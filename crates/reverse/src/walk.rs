//! Walking a folder a reverse tool names (item5-spec.md WP9c `walk.rs`, section 6): at most
//! 50,000 entries and 32 levels; each link read with `read_link` and refused without being
//! resolved when it names a network, UNC, device-namespace or (on a Mac) `/net/…` place;
//! other links resolved and refused outside the approved folders or inside a forbidden
//! folder (every mode); devices, pipes and sockets refused.
//!
//! rea follows links inside a folder it is given (an app folder, a `.app` bundle, an
//! extracted JavaScript app), so a link is checked by its own text first (no I/O for a
//! network target), then hop by hop through every further link on the way, each by its
//! text ([`follow_links`]; the system resolves nothing until no hop names a network or
//! device place), and only then where the system resolves it; a link to another folder
//! that passes is walked too, with the same limits, so a chain of links cannot lead out.
//! Links are never followed into folders by the walk itself.
//!
//! Every folder the walk enters (plain sub-folders and folders reached through a link) must
//! itself pass the sandbox: a folder holding harnessUI's own folders or a key store is
//! refused in every mode, since rea walks it for itself and cannot be told to skip them.
//! A folder that cannot be listed is refused too: rea can still open a name inside it that
//! it already knows (a `.app` bundle's `Contents/Info.plist`), which the walk never saw.
//!
//! Known limits: a link created after the walk is not seen until the next call (WP9g walks
//! again in `call`); a hard link to a file elsewhere on the same drive looks like an ordinary
//! file (the file tools share this limit).

use std::collections::{HashSet, VecDeque};
use std::path::Path;

use policy::path::{CaseRule, Flavor, Fs, LexicalPath, PathError, PathSandbox, is_within, lexical};

use crate::FileIdentity;

/// Entries walked at most before a folder is refused as too large.
pub const MAX_ENTRIES: usize = 50_000;
/// Levels walked at most.
pub const MAX_DEPTH: usize = 32;
/// Links followed at most on the way to one path (the system's own limit on Linux).
pub const MAX_HOPS: usize = 40;

/// What an entry is, without following a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A folder.
    Dir,
    /// A link (or a junction on Windows).
    Link,
    /// A character or block device.
    Device,
    /// A pipe.
    Pipe,
    /// A socket.
    Socket,
    /// Anything else.
    Other,
}

/// The file system access a folder walk needs, beyond the path sandbox's own. Tests use a
/// fake that records every access (so "no I/O for a UNC path" can be proved).
pub trait WalkFs: Fs + Send + Sync {
    /// The names inside a folder.
    fn read_dir(&self, path: &str) -> Result<Vec<String>, String>;
    /// A link's own text, never resolved; `None` when `path` is not a link; an error when
    /// it is missing or its text cannot be read (not valid text).
    fn read_link(&self, path: &str) -> Result<Option<String>, String>;
    /// What `path` is, without following a link; `None` when it does not exist.
    fn kind(&self, path: &str) -> Option<EntryKind>;
    /// The identity of the regular file at `path` (links followed); `None` when it is not
    /// one or cannot be read. (Added by WP9c, with a default for the real file system.)
    fn identity(&self, path: &str) -> Option<FileIdentity> {
        FileIdentity::of(Path::new(path)).ok()
    }
    /// How names compare on the volume holding `path`. (Added by WP9c, with a default for
    /// the real file system.)
    fn case_of(&self, path: &str) -> CaseRule {
        policy::path::volume_case(path)
    }
}

/// Why a folder was refused (each maps to a message of section 6, [`WalkRefusal::message`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalkRefusal {
    /// More than [`MAX_ENTRIES`] entries or [`MAX_DEPTH`] levels.
    TooLarge,
    /// A link that leads out of this chat's reach.
    LinkOut {
        /// The link's path.
        link: String,
    },
    /// A device, pipe or socket.
    Device {
        /// Its path.
        path: String,
    },
    /// A folder the walk could not list or check, or a name it cannot represent (not valid
    /// text): it is not checked, so it is refused. (Added by WP9c.)
    Unreadable {
        /// Its path.
        path: String,
    },
    /// A folder inside it is one of harnessUI's own folders or a key store (refused in
    /// every mode). (Added by WP9c in review round 1.)
    Forbidden {
        /// That folder's path.
        path: String,
    },
}

impl WalkRefusal {
    /// The message the model reads (section 6), with paths made safe to show.
    pub fn message(&self) -> String {
        use crate::describe::messages as m;
        match self {
            WalkRefusal::TooLarge => m::TOO_LARGE.to_string(),
            WalkRefusal::LinkOut { link } => format!("{}{}.", m::LINK_OUT, shown(link)),
            WalkRefusal::Device { path } => format!("{}{}.", m::DEVICE_IN_FOLDER, shown(path)),
            WalkRefusal::Unreadable { path } => format!("{}{}.", m::UNREADABLE, shown(path)),
            WalkRefusal::Forbidden { path } => {
                format!("{}{}.", m::HOLDS_APP_FOLDERS, shown(path))
            }
        }
    }
}

/// A path as a message or card shows it: one line, invisible characters escaped, never cut
/// short (a cut could hide which file it is).
pub(crate) fn shown(path: &str) -> String {
    policy::sanitize::one_line(path, usize::MAX)
}

/// The real file system.
pub struct RealWalkFs;

impl Fs for RealWalkFs {
    fn canonicalize(&self, path: &str) -> Result<Option<String>, String> {
        policy::path::RealFs.canonicalize(path)
    }
    fn is_symlink(&self, path: &str) -> Option<bool> {
        policy::path::RealFs.is_symlink(path)
    }
}

impl WalkFs for RealWalkFs {
    fn read_dir(&self, path: &str) -> Result<Vec<String>, String> {
        // A folder the user may enter but not list is an error too, not an empty folder:
        // rea can still open a name inside it that it knows.
        let entries = std::fs::read_dir(path).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "a name that is not valid text".to_string())?;
            out.push(name);
            // One more than the limit is enough to refuse; never list millions.
            if out.len() > MAX_ENTRIES {
                break;
            }
        }
        Ok(out)
    }

    fn read_link(&self, path: &str) -> Result<Option<String>, String> {
        let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !meta.file_type().is_symlink() {
            return Ok(None);
        }
        let target = std::fs::read_link(path).map_err(|e| e.to_string())?;
        target
            .into_os_string()
            .into_string()
            .map(Some)
            .map_err(|_| "a link whose target is not valid text".to_string())
    }

    fn kind(&self, path: &str) -> Option<EntryKind> {
        let ft = std::fs::symlink_metadata(path).ok()?.file_type();
        Some(if ft.is_symlink() {
            EntryKind::Link
        } else if ft.is_dir() {
            EntryKind::Dir
        } else if ft.is_file() {
            EntryKind::File
        } else {
            special_kind(&ft)
        })
    }
}

#[cfg(unix)]
fn special_kind(ft: &std::fs::FileType) -> EntryKind {
    use std::os::unix::fs::FileTypeExt;
    if ft.is_char_device() || ft.is_block_device() {
        EntryKind::Device
    } else if ft.is_fifo() {
        EntryKind::Pipe
    } else if ft.is_socket() {
        EntryKind::Socket
    } else {
        EntryKind::Other
    }
}

#[cfg(not(unix))]
fn special_kind(_ft: &std::fs::FileType) -> EntryKind {
    EntryKind::Other
}

/// The path flavour of a canonical path: POSIX paths start with `/`.
pub(crate) fn flavor_of(path: &str) -> Flavor {
    if path.starts_with('/') {
        Flavor::Posix
    } else {
        Flavor::Windows
    }
}

fn is_sep(c: char, flavor: Flavor) -> bool {
    c == '/' || (flavor == Flavor::Windows && c == '\\')
}

/// `dir` joined with one name.
pub(crate) fn join(dir: &str, name: &str, flavor: Flavor) -> String {
    let sep = if flavor == Flavor::Windows { '\\' } else { '/' };
    if dir.ends_with(|c| is_sep(c, flavor)) {
        format!("{dir}{name}")
    } else {
        format!("{dir}{sep}{name}")
    }
}

/// The folder holding `path` (a canonical path).
pub(crate) fn parent(path: &str, flavor: Flavor) -> String {
    let trimmed = path.trim_end_matches(|c| is_sep(c, flavor));
    match trimmed.rfind(|c| is_sep(c, flavor)) {
        Some(0) => trimmed[..1].to_string(),
        // `C:\x` → `C:\`.
        Some(i) if flavor == Flavor::Windows && trimmed[..i].ends_with(':') => {
            trimmed[..=i].to_string()
        }
        Some(i) => trimmed[..i].to_string(),
        None => trimmed.to_string(),
    }
}

/// The last name of a path.
pub(crate) fn file_name(path: &str, flavor: Flavor) -> &str {
    let trimmed = path.trim_end_matches(|c| is_sep(c, flavor));
    match trimmed.rfind(|c| is_sep(c, flavor)) {
        Some(i) => &trimmed[i + 1..],
        None => trimmed,
    }
}

fn key(path: &str, flavor: Flavor) -> String {
    match flavor {
        Flavor::Windows => path.to_lowercase(),
        Flavor::Posix => path.to_string(),
    }
}

/// Whether a link's own text, or a path the model gave, names a network or device place:
/// `\\server\share`, `\\?\UNC\…`, `\\.\…`, `\\?\Volume{…}`, `//server`, `/net/…`, a URL. Such
/// a place is refused without being looked at: on Windows, resolving `\\server\share`
/// makes the computer connect and sign in to that server. The verbatim form of a local
/// drive path (`\\?\C:\…`, `\??\C:\…`, which Windows uses for absolute links) is not one.
pub fn is_network_or_device(text: &str, flavor: Flavor) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("://")
        || ["smb:", "afp:", "nfs:", "cifs:", "webdav:"]
            .iter()
            .any(|p| lower.starts_with(p))
        || lower == "/net"
        || lower.starts_with("/net/")
        || lower == "/network"
        || lower.starts_with("/network/")
    {
        return true;
    }
    let mut chars = text.chars();
    let (a, b) = (chars.next(), chars.next());
    let two_seps = matches!((a, b), (Some(a), Some(b)) if is_sep(a, flavor) && is_sep(b, flavor));
    match flavor {
        Flavor::Posix => two_seps,
        Flavor::Windows => {
            if verbatim_drive(text).is_some() {
                return false;
            }
            two_seps || lower.starts_with("\\??\\")
        }
    }
}

/// `C:\x` for `\\?\C:\x` or `\??\C:\x` (either slash), else `None`.
pub(crate) fn verbatim_drive(text: &str) -> Option<&str> {
    let rest = ["\\\\?\\", "//?/", "\\??\\"]
        .iter()
        .find_map(|p| text.strip_prefix(p))?;
    let b = rest.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
        .then_some(rest)
}

fn is_absolute(text: &str, flavor: Flavor) -> bool {
    match flavor {
        Flavor::Posix => text.starts_with('/'),
        Flavor::Windows => {
            let b = text.as_bytes();
            b.len() >= 3
                && b[0].is_ascii_alphabetic()
                && b[1] == b':'
                && (b[2] == b'\\' || b[2] == b'/')
        }
    }
}

/// The absolute path a link's own text names, with its dot segments read, or `None` when it
/// names a network or device place (by its text, or once `..` is read: `../../net/x`) or
/// cannot be a path. Nothing is looked up.
fn link_target(text: &str, link: &str, flavor: Flavor) -> Option<String> {
    if text.is_empty() || text.contains('\0') || is_network_or_device(text, flavor) {
        return None;
    }
    let text = verbatim_drive(text).unwrap_or(text);
    let joined = if is_absolute(text, flavor) {
        text.to_string()
    } else {
        // Relative to the folder holding the link (a real folder: the walk never descends
        // through a link). Where the text passes through another link before a `..`, the
        // system reads it differently; [`follow_links`] checks that route.
        join(&parent(link, flavor), text, flavor)
    };
    let normal = lexical(&joined, None, flavor).ok()?.to_string();
    (!is_network_or_device(&normal, flavor)).then_some(normal)
}

/// What following the links on the way to a path, by their text, found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hops {
    /// No link on the way names a network or device place (the path may not exist).
    Clear,
    /// The path, or a link on the way to it, names a network or device place.
    Network,
    /// More than [`MAX_HOPS`] links, or a link text this check cannot read: refuse it.
    Unclear,
}

/// How a path (or a link's text) starts.
enum Start {
    /// An absolute path: `/` (prefix empty) or `C:\` (prefix `C:`).
    Absolute(String),
    /// `\x` on Windows: the top of the current drive.
    TopOfDrive,
    /// Relative to the folder holding the link.
    Relative,
    /// `C:x` on Windows (relative to a drive's current folder): not followed.
    Odd,
}

fn start_of(text: &str, flavor: Flavor) -> (Start, &str) {
    match flavor {
        Flavor::Posix => match text.strip_prefix('/') {
            Some(rest) => (Start::Absolute(String::new()), rest),
            None => (Start::Relative, text),
        },
        Flavor::Windows => {
            let b = text.as_bytes();
            if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
                if b.len() == 2 || b[2] == b'\\' || b[2] == b'/' {
                    let drive = format!("{}:", (b[0] as char).to_ascii_uppercase());
                    (Start::Absolute(drive), text.get(3..).unwrap_or(""))
                } else {
                    (Start::Odd, text)
                }
            } else if text.starts_with(['\\', '/']) {
                (Start::TopOfDrive, &text[1..])
            } else {
                (Start::Relative, text)
            }
        }
    }
}

/// The names of a path, in order (dot segments kept: they are read as the system reads
/// them, after the links before them).
fn names(text: &str, flavor: Flavor) -> Vec<String> {
    text.split(|c| is_sep(c, flavor))
        .filter(|n| !n.is_empty() && *n != ".")
        .map(str::to_string)
        .collect()
}

fn render(prefix: &str, done: &[String], flavor: Flavor) -> String {
    let sep = if flavor == Flavor::Windows { "\\" } else { "/" };
    format!("{prefix}{sep}{}", done.join(sep))
}

/// Follows every link on the way to `path` (an absolute path) by reading its text, one
/// hop at a time, the way the system would resolve it (a `..` after a link goes to the
/// folder above the link's target), but without letting the system resolve anything:
/// every place is checked by its text before it is looked at, so a link to
/// `\\server\share` or to `/net/…` (a Mac's automounted shares) is never touched, even at
/// the end of a chain of local links. Use it before anything canonicalises the path.
pub fn follow_links(fs: &dyn WalkFs, path: &str, flavor: Flavor) -> Hops {
    if is_network_or_device(path, flavor) {
        return Hops::Network;
    }
    let path = verbatim_drive(path).unwrap_or(path);
    let (Start::Absolute(mut prefix), rest) = start_of(path, flavor) else {
        return Hops::Unclear;
    };
    let mut done: Vec<String> = Vec::new();
    let mut todo: VecDeque<String> = names(rest, flavor).into();
    let mut hops = 0usize;
    while let Some(name) = todo.pop_front() {
        if name == ".." {
            // `done` holds no link (each was replaced by its target), so this is the
            // folder above, as the system sees it.
            done.pop();
            continue;
        }
        done.push(name);
        let here = render(&prefix, &done, flavor);
        if is_network_or_device(&here, flavor) {
            return Hops::Network;
        }
        let text = match fs.read_link(&here) {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            // Something is there, but its link text cannot be read (not valid text: bytes
            // on Linux and macOS, unpaired surrogates on Windows): the system would still
            // follow it, perhaps to a network place, so it is refused, never taken for a
            // missing name. `kind` does not follow it, and nothing before it is a link.
            Err(_) if fs.kind(&here).is_some() => return Hops::Unclear,
            // Missing (or not reachable): the system stops here too.
            Err(_) => return Hops::Clear,
        };
        hops += 1;
        if hops > MAX_HOPS || text.is_empty() || text.contains('\0') {
            return Hops::Unclear;
        }
        if is_network_or_device(&text, flavor) {
            return Hops::Network;
        }
        let text = verbatim_drive(&text).unwrap_or(&text).to_string();
        done.pop();
        let (start, rest) = start_of(&text, flavor);
        match start {
            Start::Absolute(p) => {
                prefix = p;
                done.clear();
            }
            Start::TopOfDrive => done.clear(),
            Start::Relative => {}
            Start::Odd => return Hops::Unclear,
        }
        for n in names(rest, flavor).into_iter().rev() {
            todo.push_front(n);
        }
    }
    Hops::Clear
}

/// Whether `path` sits lexically under one of the sandbox's folders: only then does the
/// sandbox look anything up for it (it refuses the rest from the text).
fn lexically_inside(sandbox: &PathSandbox, path: &str, flavor: Flavor) -> bool {
    let Ok(lex) = lexical(path, None, flavor) else {
        return false;
    };
    sandbox
        .roots()
        .iter()
        .any(|r| is_within(&lex, &r.lexical, r.case) || is_within(&lex, &r.canonical, r.case))
}

/// Whether the system may be asked to resolve `path` for `sandbox`: either the sandbox
/// refuses it from its text alone, or no link on the way names a network or device place.
pub fn safe_to_resolve(fs: &dyn WalkFs, sandbox: &PathSandbox, path: &str, flavor: Flavor) -> Hops {
    if !lexically_inside(sandbox, path, flavor) {
        return Hops::Clear;
    }
    follow_links(fs, path, flavor)
}

/// The first of `forbidden` (as written, or where it resolves) that is `folder` or lies
/// inside it, compared with the volume's case rule. Nothing is listed: a folder holding
/// harnessUI's own folders or a key store is refused before it is walked.
pub fn forbidden_inside(
    fs: &dyn WalkFs,
    folder: &str,
    forbidden: &[String],
    flavor: Flavor,
) -> Option<String> {
    let outer = lexical(folder, None, flavor).ok()?;
    let case = fs.case_of(folder);
    let inside = |f: &LexicalPath| is_within(f, &outer, case);
    for f in forbidden {
        let Ok(lex) = lexical(f, None, flavor) else {
            continue;
        };
        if inside(&lex) {
            return Some(lex.to_string());
        }
        if let Ok(Some(c)) = fs.canonicalize(&lex.to_string())
            && let Ok(cl) = lexical(&c, None, flavor)
            && inside(&cl)
        {
            return Some(cl.to_string());
        }
    }
    None
}

/// Whether a name read from a folder is a plain name.
fn plain_name(name: &str, flavor: Flavor) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('\0')
        && !name.contains(|c| is_sep(c, flavor))
}

/// Checks one link: by its own text, then where the system resolves it. Returns the
/// folder it leads to, when it leads to one that must be walked too.
fn check_link(
    fs: &dyn WalkFs,
    link: &str,
    flavor: Flavor,
    sandbox: &PathSandbox,
) -> Result<Option<String>, WalkRefusal> {
    let out = || WalkRefusal::LinkOut {
        link: link.to_string(),
    };
    let text = match fs.read_link(link) {
        Ok(Some(text)) => text,
        _ => return Err(out()),
    };
    let target = link_target(&text, link, flavor).ok_or_else(out)?;
    // Every further link on the way, by its text, before the system resolves anything:
    // the route the system takes (through the link itself) and the target as written.
    if follow_links(fs, link, flavor) != Hops::Clear
        || safe_to_resolve(fs, sandbox, &target, flavor) != Hops::Clear
    {
        return Err(out());
    }
    // A target outside the roots is refused in every mode: in Full access the roots are
    // already the whole computer (`approvals::widen`), and a forbidden folder is refused
    // by the sandbox before containment is looked at.
    let checked = |path: &str| sandbox.check(path, None, fs).map_err(|_: PathError| out());
    checked(&target)?;
    let by_system = checked(link)?;
    if is_network_or_device(&by_system.canonical, flavor) {
        return Err(out());
    }
    if !by_system.exists {
        // A dangling link: its text was checked above; nothing to look inside.
        return Ok(None);
    }
    match fs.kind(&by_system.canonical) {
        Some(EntryKind::Dir) => Ok(Some(by_system.canonical)),
        Some(EntryKind::File) => Ok(None),
        // A link to a device or pipe is refused like the thing itself.
        Some(EntryKind::Device | EntryKind::Pipe | EntryKind::Socket | EntryKind::Other) => {
            Err(WalkRefusal::Device {
                path: link.to_string(),
            })
        }
        // Resolves to a link: the sandbox resolved it fully, so this cannot happen; refuse.
        Some(EntryKind::Link) | None => Err(out()),
    }
}

/// Checks a folder found in a folder (a real folder, not a link): it must pass the sandbox
/// itself, so one of harnessUI's own folders or a key store inside the walked folder is
/// refused (the sandbox refuses it from its text, as its parent is canonical).
fn check_dir(fs: &dyn WalkFs, path: String, sandbox: &PathSandbox) -> Result<String, WalkRefusal> {
    match sandbox.check(&path, None, fs) {
        Ok(_) => Ok(path),
        Err(PathError::Forbidden) => Err(WalkRefusal::Forbidden { path }),
        // Changed since it was listed (a link now leading out), or a name the sandbox does
        // not take: not checked, so refused.
        Err(PathError::OutsideApprovedFolders | PathError::RemoteResolution) => {
            Err(WalkRefusal::LinkOut { link: path })
        }
        Err(_) => Err(WalkRefusal::Unreadable { path }),
    }
}

/// Walks `folder` (a canonical path that passed the sandbox).
///
/// `full_access` is the chat's mode. In Full access the caller has already widened the
/// sandbox's roots to the whole computer (`approvals::widen`), which is what lets links
/// lead anywhere but the forbidden folders; a target outside even those roots (a drive
/// added since) is refused in every mode, so the flag changes nothing here.
pub fn walk_folder(
    fs: &dyn WalkFs,
    folder: &str,
    sandbox: &PathSandbox,
    full_access: bool,
) -> Result<(), WalkRefusal> {
    let _ = full_access;
    let flavor = flavor_of(folder);
    let mut entries = 0usize;
    let mut visited: HashSet<String> = HashSet::from([key(folder, flavor)]);
    let mut pending: Vec<(String, usize)> = vec![(folder.to_string(), 0)];
    while let Some((dir, depth)) = pending.pop() {
        let names = fs
            .read_dir(&dir)
            .map_err(|_| WalkRefusal::Unreadable { path: dir.clone() })?;
        if names.is_empty() {
            continue;
        }
        if depth >= MAX_DEPTH {
            return Err(WalkRefusal::TooLarge);
        }
        for name in names {
            entries += 1;
            if entries > MAX_ENTRIES {
                return Err(WalkRefusal::TooLarge);
            }
            let path = join(&dir, &name, flavor);
            if !plain_name(&name, flavor) {
                return Err(WalkRefusal::Unreadable { path });
            }
            let next = match fs.kind(&path) {
                // Gone since the listing: nothing rea could read.
                None | Some(EntryKind::File) => None,
                Some(EntryKind::Dir) => Some(check_dir(fs, path, sandbox)?),
                Some(EntryKind::Link) => check_link(fs, &path, flavor, sandbox)?,
                Some(
                    EntryKind::Device | EntryKind::Pipe | EntryKind::Socket | EntryKind::Other,
                ) => {
                    return Err(WalkRefusal::Device { path });
                }
            };
            if let Some(next) = next
                && visited.insert(key(&next, flavor))
            {
                pending.push((next, depth + 1));
            }
        }
    }
    Ok(())
}

/// An in-memory file system for tests, recording every access.
#[cfg(test)]
pub(crate) mod fake {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Mutex;

    use policy::path::{CaseRule, Flavor, Fs, LexicalPath, lexical};

    use super::{EntryKind, WalkFs, join};
    use crate::FileIdentity;

    /// One entry.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Node {
        File(u64),
        Dir,
        /// A folder that can be entered but not listed (`d-wx--x--x`).
        Locked,
        Link(String),
        /// A link the system follows to the text it holds, but whose text is not valid
        /// text (bytes on Linux and macOS, unpaired surrogates on Windows), so `read_link`
        /// fails, as it does on the real file system.
        LinkNotText(String),
        Device,
        Pipe,
        Socket,
    }

    /// Paths map to nodes; keys are lower case in the Windows flavour.
    pub struct FakeFs {
        pub flavor: Flavor,
        pub nodes: BTreeMap<String, (String, Node)>,
        /// Folders holding this many generated files `f0`, `f1`, ….
        pub big: HashMap<String, usize>,
        pub calls: Mutex<Vec<String>>,
    }

    impl FakeFs {
        pub fn posix() -> Self {
            Self::new(Flavor::Posix)
        }
        pub fn windows() -> Self {
            Self::new(Flavor::Windows)
        }
        fn new(flavor: Flavor) -> Self {
            let mut fs = Self {
                flavor,
                nodes: BTreeMap::new(),
                big: HashMap::new(),
                calls: Mutex::new(Vec::new()),
            };
            match flavor {
                Flavor::Posix => fs.put("/", Node::Dir),
                Flavor::Windows => fs.put("C:\\", Node::Dir),
            }
            fs
        }
        fn key(&self, path: &str) -> String {
            match self.flavor {
                Flavor::Windows => path.to_lowercase().replace('/', "\\"),
                Flavor::Posix => path.to_string(),
            }
        }
        fn put(&mut self, path: &str, node: Node) {
            let k = self.key(path);
            self.nodes.insert(k, (path.to_string(), node));
        }
        /// Adds folders, creating the folders above them.
        pub fn dir(mut self, path: &str) -> Self {
            let lex = lexical(path, None, self.flavor).expect("absolute");
            let mut cur = LexicalPath {
                prefix: lex.prefix.clone(),
                components: vec![],
                flavor: self.flavor,
            };
            for c in &lex.components {
                cur.components.push(c.clone());
                let p = cur.to_string();
                if !self.nodes.contains_key(&self.key(&p)) {
                    self.put(&p, Node::Dir);
                }
            }
            self
        }
        fn above(self, path: &str) -> Self {
            let parent = super::parent(path, self.flavor);
            self.dir(&parent)
        }
        pub fn file(self, path: &str) -> Self {
            let mut fs = self.above(path);
            let n = fs.nodes.len() as u64;
            fs.put(path, Node::File(n));
            fs
        }
        pub fn link(self, path: &str, target: &str) -> Self {
            let mut fs = self.above(path);
            fs.put(path, Node::Link(target.to_string()));
            fs
        }
        pub fn special(self, path: &str, node: Node) -> Self {
            let mut fs = self.above(path);
            fs.put(path, node);
            fs
        }
        pub fn many(mut self, path: &str, files: usize) -> Self {
            self = self.dir(path);
            let k = self.key(path);
            self.big.insert(k, files);
            self
        }
        pub fn accessed(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        pub fn clear(&self) {
            self.calls.lock().unwrap().clear();
        }
        fn note(&self, what: &str, path: &str) {
            self.calls.lock().unwrap().push(format!("{what} {path}"));
        }
        fn node(&self, path: &str) -> Option<Node> {
            let k = self.key(path);
            if let Some((_, n)) = self.nodes.get(&k) {
                return Some(n.clone());
            }
            let parent = super::parent(path, self.flavor);
            let name = super::file_name(path, self.flavor);
            let count = *self.big.get(&self.key(&parent))?;
            let i: usize = name.strip_prefix('f')?.parse().ok()?;
            (i < count).then_some(Node::File(1_000_000 + i as u64))
        }
        /// Resolves every component, following links (at most 40 hops).
        fn resolve(&self, path: &str) -> Result<Option<String>, String> {
            let lex = lexical(path, None, self.flavor).map_err(|e| e.to_string())?;
            let mut cur = LexicalPath {
                prefix: lex.prefix.clone(),
                components: vec![],
                flavor: self.flavor,
            };
            let mut rest: Vec<String> = lex.components.clone();
            rest.reverse();
            let mut hops = 0;
            while let Some(c) = rest.pop() {
                cur.components.push(c);
                let here = cur.to_string();
                match self.node(&here) {
                    None => return Ok(None),
                    Some(Node::Link(target) | Node::LinkNotText(target)) => {
                        hops += 1;
                        if hops > 40 {
                            return Err("too many levels of links".into());
                        }
                        cur.components.pop();
                        let base = cur.to_string();
                        // Windows reports absolute links in the verbatim form.
                        let target =
                            super::verbatim_drive(&target).map_or(target.clone(), str::to_string);
                        let t = if super::is_absolute(&target, self.flavor) {
                            target
                        } else {
                            join(&base, &target, self.flavor)
                        };
                        let t = lexical(&t, None, self.flavor).map_err(|e| e.to_string())?;
                        cur.prefix = t.prefix;
                        cur.components = vec![];
                        let mut more = t.components;
                        more.reverse();
                        rest.extend(more);
                    }
                    Some(_) => {}
                }
            }
            // Report the stored spelling, as a real file system does.
            let out = cur.to_string();
            Ok(Some(match self.nodes.get(&self.key(&out)) {
                Some((spelled, _)) => spelled.clone(),
                None => out,
            }))
        }
    }

    impl Fs for FakeFs {
        fn canonicalize(&self, path: &str) -> Result<Option<String>, String> {
            self.note("canonicalize", path);
            self.resolve(path)
        }
        fn is_symlink(&self, path: &str) -> Option<bool> {
            self.note("is_symlink", path);
            Some(matches!(
                self.node(path)?,
                Node::Link(_) | Node::LinkNotText(_)
            ))
        }
    }

    impl WalkFs for FakeFs {
        fn read_dir(&self, path: &str) -> Result<Vec<String>, String> {
            self.note("read_dir", path);
            if let Some(n) = self.big.get(&self.key(path)) {
                return Ok((0..*n).map(|i| format!("f{i}")).collect());
            }
            match self.node(path) {
                Some(Node::Dir) => {}
                Some(Node::Locked) => return Err("permission denied".into()),
                _ => return Err("not a folder".into()),
            }
            let prefix = self.key(&join(path, "", self.flavor));
            Ok(self
                .nodes
                .iter()
                .filter(|(k, _)| {
                    k.len() > prefix.len()
                        && k.starts_with(&prefix)
                        && !k[prefix.len()..].contains(['/', '\\'])
                })
                .map(|(_, (spelled, _))| super::file_name(spelled, self.flavor).to_string())
                .collect())
        }
        fn read_link(&self, path: &str) -> Result<Option<String>, String> {
            self.note("read_link", path);
            match self.node(path) {
                Some(Node::Link(t)) => Ok(Some(t)),
                Some(Node::LinkNotText(_)) => Err("a link whose target is not valid text".into()),
                Some(_) => Ok(None),
                None => Err("missing".into()),
            }
        }
        fn kind(&self, path: &str) -> Option<EntryKind> {
            self.note("kind", path);
            Some(match self.node(path)? {
                Node::File(_) => EntryKind::File,
                Node::Dir | Node::Locked => EntryKind::Dir,
                Node::Link(_) | Node::LinkNotText(_) => EntryKind::Link,
                Node::Device => EntryKind::Device,
                Node::Pipe => EntryKind::Pipe,
                Node::Socket => EntryKind::Socket,
            })
        }
        fn identity(&self, path: &str) -> Option<FileIdentity> {
            self.note("identity", path);
            let real = self.resolve(path).ok()??;
            match self.node(&real)? {
                Node::File(n) => Some(FileIdentity {
                    device: 1,
                    file: n,
                    size: 100 + n,
                    modified: i128::from(n),
                }),
                _ => None,
            }
        }
        fn case_of(&self, _path: &str) -> CaseRule {
            match self.flavor {
                Flavor::Windows => CaseRule::Insensitive,
                Flavor::Posix => CaseRule::Sensitive,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{FakeFs, Node};
    use super::*;

    fn sandbox(fs: &FakeFs, approved: &[&str], forbidden: &[&str]) -> PathSandbox {
        let a: Vec<String> = approved.iter().map(|s| s.to_string()).collect();
        let f: Vec<String> = forbidden.iter().map(|s| s.to_string()).collect();
        let (sb, skipped) = PathSandbox::new(fs.flavor, &a, &f, fs, &|p| fs.case_of(p));
        assert!(skipped.is_empty(), "{skipped:?}");
        sb
    }

    fn app() -> FakeFs {
        FakeFs::posix()
            .file("/w/app/main.js")
            .file("/w/app/lib/a.js")
            .dir("/w/app/empty")
            .file("/w/other/x.js")
            .file("/etc/passwd")
            .dir("/data/harnessUI/reverse")
    }

    #[test]
    fn a_plain_folder_passes() {
        let fs = app().link("/w/app/lib/rel", "../main.js");
        let sb = sandbox(&fs, &["/w"], &["/data/harnessUI"]);
        assert_eq!(walk_folder(&fs, "/w/app", &sb, false), Ok(()));
    }

    #[test]
    fn a_link_out_is_refused_except_into_another_approved_folder() {
        for target in ["/etc/passwd", "/etc", "../../../etc/passwd", "/missing/x"] {
            let fs = app().link("/w/app/lib/l", target);
            let sb = sandbox(&fs, &["/w/app"], &[]);
            assert_eq!(
                walk_folder(&fs, "/w/app", &sb, false),
                Err(WalkRefusal::LinkOut {
                    link: "/w/app/lib/l".into()
                }),
                "{target}"
            );
        }
        // Into another approved folder: fine.
        let fs = app().link("/w/app/lib/l", "/w/other/x.js");
        let sb = sandbox(&fs, &["/w/app", "/w/other"], &[]);
        assert_eq!(walk_folder(&fs, "/w/app", &sb, false), Ok(()));
    }

    /// A link into a forbidden folder is refused in every mode, Full access (whole-computer
    /// roots) included; a link elsewhere passes in Full access.
    #[test]
    fn full_access_reaches_everything_but_the_forbidden_folders() {
        let fs = app().link("/w/app/l", "/data/harnessUI/reverse");
        let full = sandbox(&fs, &["/w/app", "/"], &["/data/harnessUI"]);
        assert_eq!(
            walk_folder(&fs, "/w/app", &full, true),
            Err(WalkRefusal::LinkOut {
                link: "/w/app/l".into()
            })
        );
        let fs = app().link("/w/app/l", "/etc/passwd");
        let full = sandbox(&fs, &["/w/app", "/"], &["/data/harnessUI"]);
        assert_eq!(walk_folder(&fs, "/w/app", &full, true), Ok(()));
    }

    /// A chain of links cannot lead out: a link to a folder that passes is walked too.
    #[test]
    fn a_folder_reached_through_a_link_is_walked_too() {
        let fs = app()
            .link("/w/app/to-other", "/w/other")
            .link("/w/other/out", "/etc");
        let sb = sandbox(&fs, &["/w"], &[]);
        assert_eq!(
            walk_folder(&fs, "/w/app", &sb, false),
            Err(WalkRefusal::LinkOut {
                link: "/w/other/out".into()
            })
        );
        // A loop of links ends.
        let fs = app()
            .link("/w/app/a", "/w/app/b")
            .link("/w/app/b", "/w/app/a");
        let sb = sandbox(&fs, &["/w"], &[]);
        assert!(matches!(
            walk_folder(&fs, "/w/app", &sb, false),
            Err(WalkRefusal::LinkOut { .. })
        ));
        // A link back to the folder itself is walked once.
        let fs = app().link("/w/app/lib/up", "/w/app");
        let sb = sandbox(&fs, &["/w"], &[]);
        assert_eq!(walk_folder(&fs, "/w/app", &sb, false), Ok(()));
    }

    /// A link to a network or device place is refused from its text alone: nothing is
    /// looked up for it.
    #[test]
    fn network_and_device_links_cause_no_lookup() {
        for target in ["//server/share/x", "/net/server/x", "smb://server/x"] {
            let fs = app().link("/w/app/n", target);
            let sb = sandbox(&fs, &["/"], &[]);
            fs.clear();
            assert_eq!(
                walk_folder(&fs, "/w/app", &sb, true),
                Err(WalkRefusal::LinkOut {
                    link: "/w/app/n".into()
                }),
                "{target}"
            );
            let looked: Vec<String> = fs
                .accessed()
                .into_iter()
                .filter(|a| a.contains("server"))
                .collect();
            assert!(looked.is_empty(), "{target}: {looked:?}");
        }
    }

    #[test]
    fn windows_junctions_and_shares() {
        let base = || {
            FakeFs::windows()
                .file("C:\\Users\\me\\app\\app.exe")
                .file("C:\\Windows\\System32\\cmd.exe")
                .dir("C:\\Users\\me\\AppData\\Local\\harnessUI")
        };
        // A junction (absolute, in the verbatim form Windows reports) out of the folder.
        let fs = base().link("C:\\Users\\me\\app\\j", "\\\\?\\C:\\Windows");
        let sb = sandbox(&fs, &["C:\\Users\\me\\app"], &[]);
        assert_eq!(
            walk_folder(&fs, "C:\\Users\\me\\app", &sb, false),
            Err(WalkRefusal::LinkOut {
                link: "C:\\Users\\me\\app\\j".into()
            })
        );
        // The same in Full access passes; into the app's data it does not.
        let full = sandbox(&fs, &["C:\\Users\\me\\app", "C:\\"], &[]);
        assert_eq!(walk_folder(&fs, "C:\\Users\\me\\app", &full, true), Ok(()));
        let fs = base().link(
            "C:\\Users\\me\\app\\j",
            "\\??\\C:\\Users\\me\\AppData\\Local\\harnessUI",
        );
        let full = sandbox(
            &fs,
            &["C:\\Users\\me\\app", "C:\\"],
            &["C:\\Users\\me\\AppData\\Local\\harnessUI"],
        );
        assert!(matches!(
            walk_folder(&fs, "C:\\Users\\me\\app", &full, true),
            Err(WalkRefusal::LinkOut { .. })
        ));
        // Shares and device namespaces: refused from the text, nothing looked up.
        for target in [
            "\\\\server\\share\\x",
            "\\\\?\\UNC\\server\\share",
            "\\\\.\\PhysicalDrive0",
            "\\\\?\\Volume{0000}\\x",
            "//server/share",
        ] {
            let fs = base().link("C:\\Users\\me\\app\\n", target);
            let full = sandbox(&fs, &["C:\\"], &[]);
            fs.clear();
            assert!(
                matches!(
                    walk_folder(&fs, "C:\\Users\\me\\app", &full, true),
                    Err(WalkRefusal::LinkOut { .. })
                ),
                "{target}"
            );
            assert!(
                !fs.accessed()
                    .iter()
                    .any(|a| a.to_lowercase().contains("server")
                        || a.contains("PhysicalDrive")
                        || a.contains("Volume")),
                "{target}: {:?}",
                fs.accessed()
            );
        }
    }

    #[test]
    fn devices_pipes_and_sockets_are_refused() {
        for node in [Node::Device, Node::Pipe, Node::Socket] {
            let fs = app().special("/w/app/lib/odd", node.clone());
            let sb = sandbox(&fs, &["/"], &[]);
            assert_eq!(
                walk_folder(&fs, "/w/app", &sb, true),
                Err(WalkRefusal::Device {
                    path: "/w/app/lib/odd".into()
                }),
                "{node:?}"
            );
        }
        // A link to a device, even where the link itself may lead.
        let fs = app()
            .special("/dev/zero", Node::Device)
            .link("/w/app/z", "/dev/zero");
        let sb = sandbox(&fs, &["/"], &[]);
        assert_eq!(
            walk_folder(&fs, "/w/app", &sb, true),
            Err(WalkRefusal::Device {
                path: "/w/app/z".into()
            })
        );
    }

    #[test]
    fn the_limits() {
        let fs = FakeFs::posix().many("/w/big", MAX_ENTRIES + 1);
        let sb = sandbox(&fs, &["/w"], &[]);
        assert_eq!(
            walk_folder(&fs, "/w/big", &sb, false),
            Err(WalkRefusal::TooLarge)
        );
        let fs = FakeFs::posix().many("/w/big", MAX_ENTRIES);
        let sb = sandbox(&fs, &["/w"], &[]);
        assert_eq!(walk_folder(&fs, "/w/big", &sb, false), Ok(()));
        // 32 levels below the folder are walked; a 33rd is refused.
        let deep = |levels: usize| {
            let mut p = "/w/deep".to_string();
            for i in 0..levels {
                p.push_str(&format!("/{i}"));
            }
            p
        };
        let fs = FakeFs::posix().file(&format!("{}/f", deep(MAX_DEPTH - 1)));
        let sb = sandbox(&fs, &["/w"], &[]);
        assert_eq!(walk_folder(&fs, "/w/deep", &sb, false), Ok(()));
        let fs = FakeFs::posix().file(&format!("{}/f", deep(MAX_DEPTH)));
        let sb = sandbox(&fs, &["/w"], &[]);
        assert_eq!(
            walk_folder(&fs, "/w/deep", &sb, false),
            Err(WalkRefusal::TooLarge)
        );
    }

    #[test]
    fn link_texts() {
        let w = Flavor::Windows;
        let p = Flavor::Posix;
        assert!(is_network_or_device("\\\\srv\\s", w));
        assert!(is_network_or_device("//srv/s", w));
        assert!(is_network_or_device("\\\\?\\UNC\\srv\\s", w));
        assert!(is_network_or_device("\\\\.\\pipe\\x", w));
        assert!(is_network_or_device("\\??\\Volume{1}\\x", w));
        assert!(!is_network_or_device("\\\\?\\C:\\x", w));
        assert!(!is_network_or_device("\\??\\D:\\x", w));
        assert!(!is_network_or_device("C:\\x", w));
        assert!(!is_network_or_device("..\\x", w));
        assert!(is_network_or_device("//srv/s", p));
        assert!(is_network_or_device("/net/srv/s", p));
        assert!(is_network_or_device("/Network/Servers/x", p));
        assert!(is_network_or_device("file:///etc/passwd", p));
        assert!(!is_network_or_device("/netbook/x", p));
        assert!(!is_network_or_device("../x", p));
        assert_eq!(
            link_target("\\\\?\\C:\\Windows", "C:\\a\\l", w).as_deref(),
            Some("C:\\Windows")
        );
        // Dot segments are read, so `..` cannot hide a network place.
        assert_eq!(
            link_target("..\\b", "C:\\a\\l", w).as_deref(),
            Some("C:\\b")
        );
        assert_eq!(link_target("x", "/a/l", p).as_deref(), Some("/a/x"));
        assert_eq!(link_target("", "/a/l", p), None);
        assert_eq!(link_target("../../net/srv/s", "/a/b/l", p), None);
        assert_eq!(link_target("../../Network/x", "/a/b/l", p), None);
        assert_eq!(link_target("../../../x", "/a/l", p), None);
        assert_eq!(parent("/a", p), "/");
        assert_eq!(parent("C:\\a", w), "C:\\");
        assert_eq!(file_name("C:\\a\\b.exe", w), "b.exe");
        assert_eq!(
            WalkRefusal::LinkOut {
                link: "/w/a\u{202E}b".into()
            }
            .message(),
            "That folder contains a link to a place this chat can't reach: /w/a\\u{202E}b."
        );
    }

    /// A folder holding one of harnessUI's own folders or a key store is refused in every
    /// mode (Full access's whole-computer roots included), as a plain sub-folder or inside a
    /// folder a link leads to; nothing below the forbidden folder is listed.
    #[test]
    fn a_folder_holding_a_forbidden_folder_is_refused_in_every_mode() {
        let tree = || {
            app()
                .file("/home/u/.local/share/com.harnessui.app/harness.db")
                .dir("/home/u/.local/share/com.harnessui.app/reverse")
                .file("/home/u/.local/share/keyrings/login.keyring")
                .file("/home/u/.local/share/other/a.js")
        };
        let forbidden = [
            "/home/u/.local/share/com.harnessui.app",
            "/home/u/.local/share/keyrings",
        ];
        for approved in [&["/home/u"][..], &["/home/u", "/"][..]] {
            let fs = tree();
            let sb = sandbox(&fs, approved, &forbidden);
            fs.clear();
            for folder in ["/home/u/.local/share", "/home/u/.local", "/home/u"] {
                assert!(
                    matches!(
                        walk_folder(&fs, folder, &sb, approved.len() > 1),
                        Err(WalkRefusal::Forbidden { .. })
                    ),
                    "{folder} {approved:?}"
                );
            }
            assert!(
                !fs.accessed().iter().any(|a| a
                    .starts_with("read_dir /home/u/.local/share/com.harnessui.app")
                    || a.starts_with("read_dir /home/u/.local/share/keyrings")),
                "{:?}",
                fs.accessed()
            );
            assert_eq!(
                walk_folder(&fs, "/home/u/.local/share/other", &sb, false),
                Ok(())
            );
        }
        // Behind a link: the folder it leads to is walked, and refused.
        let fs = tree().link("/w/app/share", "/home/u/.local/share");
        let sb = sandbox(&fs, &["/w", "/home/u", "/"], &forbidden);
        assert_eq!(
            walk_folder(&fs, "/w/app", &sb, true),
            Err(WalkRefusal::Forbidden {
                path: "/home/u/.local/share/com.harnessui.app".into()
            })
        );
        assert_eq!(
            WalkRefusal::Forbidden {
                path: "/home/u/.local/share/keyrings".into()
            }
            .message(),
            "That folder contains harnessUI's own folders, which tools can't reach: /home/u/.local/share/keyrings."
        );
        // Found without listing anything, as written and where it resolves.
        let fs = tree().link(
            "/home/u/data-link",
            "/home/u/.local/share/com.harnessui.app",
        );
        let f = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            forbidden_inside(&fs, "/home/u/.local", &f(&forbidden), Flavor::Posix).as_deref(),
            Some("/home/u/.local/share/com.harnessui.app")
        );
        assert_eq!(
            forbidden_inside(
                &fs,
                "/home/u/.local/share",
                &f(&["/home/u/data-link"]),
                Flavor::Posix
            )
            .as_deref(),
            Some("/home/u/.local/share/com.harnessui.app")
        );
        assert_eq!(
            forbidden_inside(
                &fs,
                "/home/u/.local/share/other",
                &f(&forbidden),
                Flavor::Posix
            ),
            None
        );
        assert_eq!(
            forbidden_inside(
                &FakeFs::windows(),
                "c:\\users\\u\\appdata",
                &f(&["C:\\Users\\u\\AppData\\Local\\harnessUI"]),
                Flavor::Windows
            )
            .as_deref(),
            Some("C:\\Users\\u\\AppData\\Local\\harnessUI")
        );
    }

    /// A folder that can be entered but not listed is refused: rea could still open a name
    /// inside it that it knows, which the walk never saw.
    #[test]
    fn an_unlistable_folder_is_refused() {
        let fs = app()
            .special("/w/x.app/Contents", Node::Locked)
            .link("/w/x.app/Contents/Info.plist", "/etc/passwd");
        let sb = sandbox(&fs, &["/w"], &[]);
        assert_eq!(
            walk_folder(&fs, "/w/x.app", &sb, false),
            Err(WalkRefusal::Unreadable {
                path: "/w/x.app/Contents".into()
            })
        );
    }

    /// Links are followed by their text, hop by hop, the way the system resolves them, and
    /// nothing named by a network or device place is looked at.
    #[test]
    fn follow_links_reads_every_hop_by_its_text() {
        let p = Flavor::Posix;
        let fs = app()
            .link("/w/a", "/w/b")
            .link("/w/b", "/net/srv/share")
            .link("/w/c", "../w/d/../e")
            .link("/w/d", "/net/srv/x/y")
            .link("/w/ok", "app/lib")
            .link("/w/loop1", "/w/loop2")
            .link("/w/loop2", "/w/loop1")
            .link("/w/rel", "../../../../net/srv/s");
        for path in [
            "/w/a",
            "/w/a/deeper",
            "/w/c",
            "/w/rel",
            "/w/rel/x",
            "/w/app/../a",
        ] {
            fs.clear();
            assert_eq!(follow_links(&fs, path, p), Hops::Network, "{path}");
            assert!(
                !fs.accessed()
                    .iter()
                    .any(|a| a.contains(" /net") || a.starts_with("canonicalize")),
                "{path}: {:?}",
                fs.accessed()
            );
        }
        assert_eq!(follow_links(&fs, "/w/ok/a.js", p), Hops::Clear);
        assert_eq!(follow_links(&fs, "/w/missing/x", p), Hops::Clear);
        assert_eq!(follow_links(&fs, "/w/loop1", p), Hops::Unclear);
        assert_eq!(follow_links(&fs, "relative", p), Hops::Unclear);
        // Windows: a junction chain to a share, the top of the drive, a drive-relative text.
        let w = Flavor::Windows;
        let fs = FakeFs::windows()
            .file("C:\\Windows\\notepad.exe")
            .link("C:\\Users\\me\\j1", "\\\\?\\C:\\Users\\me\\j2")
            .link("C:\\Users\\me\\j2", "\\\\server\\share")
            .link("C:\\Users\\me\\top", "\\Windows")
            .link("C:\\Users\\me\\odd", "D:x");
        fs.clear();
        assert_eq!(
            follow_links(&fs, "C:\\Users\\me\\j1\\x.exe", w),
            Hops::Network
        );
        assert!(
            !fs.accessed()
                .iter()
                .any(|a| a.to_lowercase().contains("server"))
        );
        assert_eq!(
            follow_links(&fs, "C:\\Users\\me\\top\\notepad.exe", w),
            Hops::Clear
        );
        assert_eq!(follow_links(&fs, "C:\\Users\\me\\odd", w), Hops::Unclear);
        assert_eq!(follow_links(&fs, "\\\\server\\share\\x", w), Hops::Network);
    }

    /// The real file system (Unix): a folder holding the app's folders, a chain of links to
    /// `/net`, and a folder that cannot be listed (the last only when not run as root, which
    /// lists every folder).
    #[cfg(unix)]
    #[test]
    fn the_real_file_system_round_one() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let s = |p: &Path| p.display().to_string();
        let share = root.join("share");
        std::fs::create_dir_all(share.join("com.harnessui.app/reverse")).unwrap();
        std::fs::create_dir_all(share.join("keyrings")).unwrap();
        std::fs::write(share.join("com.harnessui.app/harness.db"), "x").unwrap();
        std::fs::write(share.join("keyrings/login.keyring"), "k").unwrap();
        let forbidden = [
            s(&share.join("com.harnessui.app")),
            s(&share.join("keyrings")),
        ];
        let sb = |approved: &[String], forbidden: &[String]| {
            PathSandbox::new(Flavor::Posix, approved, forbidden, &RealWalkFs, &|p| {
                RealWalkFs.case_of(p)
            })
            .0
        };
        for approved in [vec![s(&root)], vec![s(&root), "/".to_string()]] {
            let full = approved.len() > 1;
            assert!(
                matches!(
                    walk_folder(&RealWalkFs, &s(&share), &sb(&approved, &forbidden), full),
                    Err(WalkRefusal::Forbidden { .. })
                ),
                "{approved:?}"
            );
            assert!(forbidden_inside(&RealWalkFs, &s(&share), &forbidden, Flavor::Posix).is_some());
        }
        std::fs::remove_dir_all(&share).unwrap();

        // a -> b -> /net/…: refused; before review round 1 it passed as a dangling link.
        let app = root.join("app");
        std::fs::create_dir_all(&app).unwrap();
        symlink(root.join("b"), app.join("a")).unwrap();
        symlink("/net/server/share", root.join("b")).unwrap();
        assert_eq!(
            walk_folder(&RealWalkFs, &s(&app), &sb(&[s(&root)], &[]), false),
            Err(WalkRefusal::LinkOut {
                link: s(&app.join("a"))
            })
        );
        assert_eq!(
            follow_links(&RealWalkFs, &s(&app.join("a")), Flavor::Posix),
            Hops::Network
        );
        std::fs::remove_file(app.join("a")).unwrap();

        // Contents can be entered but not listed; Info.plist inside leads out.
        let bundle = root.join("x.app");
        let contents = bundle.join("Contents");
        std::fs::create_dir_all(&contents).unwrap();
        std::fs::create_dir_all(root.join("outside")).unwrap();
        std::fs::write(root.join("outside/secret.plist"), "s").unwrap();
        symlink(
            root.join("outside/secret.plist"),
            contents.join("Info.plist"),
        )
        .unwrap();
        std::fs::set_permissions(&contents, std::fs::Permissions::from_mode(0o311)).unwrap();
        // SAFETY: geteuid has no preconditions.
        let root_user = unsafe { libc::geteuid() } == 0;
        let result = walk_folder(&RealWalkFs, &s(&bundle), &sb(&[s(&bundle)], &[]), false);
        std::fs::set_permissions(&contents, std::fs::Permissions::from_mode(0o755)).unwrap();
        if root_user {
            // Root lists it anyway, and so meets the link out.
            assert!(
                matches!(result, Err(WalkRefusal::LinkOut { .. })),
                "{result:?}"
            );
        } else {
            assert_eq!(result, Err(WalkRefusal::Unreadable { path: s(&contents) }));
        }
    }

    /// The real file system (Unix: links, a pipe and a socket made in a temporary folder).
    #[cfg(unix)]
    #[test]
    fn the_real_file_system() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let app = root.join("app");
        let outside = root.join("outside");
        std::fs::create_dir_all(app.join("lib")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(app.join("main.js"), "x").unwrap();
        std::fs::write(outside.join("secret"), "s").unwrap();
        symlink("../main.js", app.join("lib/rel")).unwrap();
        let s = |p: &Path| p.display().to_string();
        let sb = |approved: &[&Path]| {
            let a: Vec<String> = approved.iter().map(|p| s(p)).collect();
            PathSandbox::new(Flavor::Posix, &a, &[], &RealWalkFs, &|p| {
                RealWalkFs.case_of(p)
            })
            .0
        };
        assert_eq!(
            walk_folder(&RealWalkFs, &s(&app), &sb(&[&app]), false),
            Ok(())
        );
        assert_eq!(
            RealWalkFs.kind(&s(&app.join("lib/rel"))),
            Some(EntryKind::Link)
        );
        assert_eq!(
            RealWalkFs
                .read_link(&s(&app.join("lib/rel")))
                .unwrap()
                .as_deref(),
            Some("../main.js")
        );
        assert_eq!(
            RealWalkFs.read_link(&s(&app.join("main.js"))).unwrap(),
            None
        );
        assert!(RealWalkFs.identity(&s(&app.join("main.js"))).is_some());
        assert!(RealWalkFs.identity(&s(&app)).is_none());

        symlink(&outside, app.join("lib/out")).unwrap();
        assert_eq!(
            walk_folder(&RealWalkFs, &s(&app), &sb(&[&app]), false),
            Err(WalkRefusal::LinkOut {
                link: s(&app.join("lib/out"))
            })
        );
        assert_eq!(
            walk_folder(&RealWalkFs, &s(&app), &sb(&[&root]), false),
            Ok(())
        );
        std::fs::remove_file(app.join("lib/out")).unwrap();

        let fifo = app.join("pipe");
        let c = std::ffi::CString::new(s(&fifo)).unwrap();
        // SAFETY: `c` is a valid NUL-terminated path; mkfifo only reads it.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert_eq!(RealWalkFs.kind(&s(&fifo)), Some(EntryKind::Pipe));
        assert_eq!(
            walk_folder(&RealWalkFs, &s(&app), &sb(&[&app]), false),
            Err(WalkRefusal::Device { path: s(&fifo) })
        );
        std::fs::remove_file(&fifo).unwrap();
        let sock = app.join("sock");
        let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        assert_eq!(RealWalkFs.kind(&s(&sock)), Some(EntryKind::Socket));
        assert!(matches!(
            walk_folder(&RealWalkFs, &s(&app), &sb(&[&app]), false),
            Err(WalkRefusal::Device { .. })
        ));
    }

    /// A link whose text cannot be read (not valid text: bytes on Linux and macOS, unpaired
    /// surrogates on Windows) is refused, never taken for a missing name: the system would
    /// still follow it, perhaps to a network place. Nothing is resolved through it, whether
    /// it is the path itself, at the end of a chain of links, or inside a walked folder.
    #[test]
    fn a_link_whose_text_is_not_valid_text_is_refused() {
        let p = Flavor::Posix;
        let bad = || {
            app()
                .special("/w/bad", Node::LinkNotText("//server/share".into()))
                .link("/w/a", "/w/bad")
        };
        let resolved = |fs: &FakeFs, also: &str| -> Vec<String> {
            fs.accessed()
                .into_iter()
                .filter(|a| {
                    a.contains("server")
                        || a == "canonicalize /w/bad"
                        || a.starts_with("canonicalize /w/bad/")
                        || a == "canonicalize /w/a"
                        || a.starts_with("canonicalize /w/a/")
                        || a == &format!("canonicalize {also}")
                })
                .collect()
        };
        let fs = bad().link("/w/app/to-bad", "../bad/x");
        for path in ["/w/bad", "/w/bad/x", "/w/a", "/w/a/x", "/w/app/to-bad"] {
            fs.clear();
            assert_eq!(follow_links(&fs, path, p), Hops::Unclear, "{path}");
            let looked = resolved(&fs, path);
            assert!(looked.is_empty(), "{path}: {looked:?}");
        }
        // A missing name is still missing.
        assert_eq!(follow_links(&fs, "/w/missing/x", p), Hops::Clear);
        // In a walked folder, in every mode: such a link itself, one link to it, and a chain
        // of two links to it are each refused as a link to a place the chat can't reach.
        for (link, target) in [
            ("/w/app/lib/direct", None),
            ("/w/app/lib/one", Some("../../bad/x")),
            ("/w/app/lib/two", Some("/w/a")),
        ] {
            for approved in [&["/w"][..], &["/w", "/"][..]] {
                let fs = match target {
                    None => bad().special(link, Node::LinkNotText("//server/share".into())),
                    Some(t) => bad().link(link, t),
                };
                let sb = sandbox(&fs, approved, &[]);
                fs.clear();
                assert_eq!(
                    walk_folder(&fs, "/w/app", &sb, approved.len() > 1),
                    Err(WalkRefusal::LinkOut { link: link.into() }),
                    "{link} {approved:?}"
                );
                let looked = resolved(&fs, link);
                assert!(looked.is_empty(), "{link} {approved:?}: {looked:?}");
            }
        }
        // Windows: a junction to a link whose text holds an unpaired surrogate.
        let w = Flavor::Windows;
        let fs = FakeFs::windows()
            .special(
                "C:\\Users\\me\\bad",
                Node::LinkNotText("\\\\server\\share".into()),
            )
            .link("C:\\Users\\me\\j", "\\\\?\\C:\\Users\\me\\bad");
        for path in ["C:\\Users\\me\\bad\\x.exe", "C:\\Users\\me\\j\\x.exe"] {
            fs.clear();
            assert_eq!(follow_links(&fs, path, w), Hops::Unclear, "{path}");
            assert!(
                !fs.accessed()
                    .iter()
                    .any(|a| a.starts_with("canonicalize") || a.to_lowercase().contains("server")),
                "{path}: {:?}",
                fs.accessed()
            );
        }
    }

    /// The real file system (Unix): a link whose text is raw bytes that are not valid text,
    /// as the path itself, at the end of a chain of links and inside a walked folder, is
    /// refused (before the fix the folder passed as if the link were missing). Skipped where
    /// the system cannot make such a link.
    #[cfg(unix)]
    #[test]
    fn the_real_file_system_a_link_whose_text_is_not_valid_text() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let s = |p: &Path| p.display().to_string();
        let not_text = OsStr::from_bytes(b"//server/share/\xff");
        let bad = root.join("bad");
        if let Err(e) = symlink(not_text, &bad) {
            eprintln!("skipped: this system cannot make a link whose text is not valid text: {e}");
            return;
        }
        if RealWalkFs.read_link(&s(&bad)).is_ok() {
            eprintln!("skipped: this system keeps the link's text as valid text");
            return;
        }
        symlink(&bad, root.join("a")).unwrap();
        let p = Flavor::Posix;
        for path in [bad.clone(), bad.join("x"), root.join("a"), root.join("a/x")] {
            assert_eq!(
                follow_links(&RealWalkFs, &s(&path), p),
                Hops::Unclear,
                "{}",
                path.display()
            );
        }
        assert_eq!(
            follow_links(&RealWalkFs, &s(&root.join("missing/x")), p),
            Hops::Clear
        );
        let app = root.join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("main.js"), "x").unwrap();
        let sb = |approved: &[String]| {
            PathSandbox::new(Flavor::Posix, approved, &[], &RealWalkFs, &|p| {
                RealWalkFs.case_of(p)
            })
            .0
        };
        assert_eq!(
            walk_folder(&RealWalkFs, &s(&app), &sb(&[s(&root)]), false),
            Ok(())
        );
        for (name, target) in [
            ("direct", not_text),
            ("one", OsStr::new("../bad/x")),
            ("two", OsStr::new("../a")),
        ] {
            let link = app.join(name);
            symlink(target, &link).unwrap();
            for approved in [vec![s(&root)], vec![s(&root), "/".to_string()]] {
                let full = approved.len() > 1;
                assert_eq!(
                    walk_folder(&RealWalkFs, &s(&app), &sb(&approved), full),
                    Err(WalkRefusal::LinkOut { link: s(&link) }),
                    "{name} {approved:?}"
                );
            }
            std::fs::remove_file(&link).unwrap();
        }
    }
}
