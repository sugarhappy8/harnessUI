//! The path sandbox (ARCHITECTURE.md 13.4).
//!
//! For every path a file tool, the terminal's working directory or a git tool receives:
//!
//! 1. **Hard limits** – never under the app's data folder, the install folder or the
//!    agent's browser profile, whatever folder was approved.
//! 2. **Lexical check, with no file-system access** – NUL bytes, Windows device names,
//!    alternate data streams, UNC and device-namespace forms, dot segments; the path must
//!    sit lexically under an approved folder. Resolving `\\attacker\share\x` on Windows
//!    makes the machine connect and authenticate to that server, so this happens first.
//! 3. **Canonicalise**, following symlinks; stop if resolution reaches a remote path.
//!    For a path that does not exist yet, canonicalise the deepest existing ancestor
//!    and append the rest.
//! 4. **Containment** – the canonical path must be inside the canonical form of an
//!    approved folder, compared component by component, case-insensitively where the
//!    volume is.
//! 5. **Re-check at open time** ([`PathSandbox::recheck`]).
//!
//! Paths are handled as strings with an explicit [`Flavor`] so Windows rules are
//! tested on every OS. File-system access goes through [`Fs`] so tests can prove that
//! rejected paths cause no I/O.

use std::fmt;

/// Which OS's path rules apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Windows,
    Posix,
}

impl Flavor {
    pub const fn native() -> Self {
        if cfg!(windows) {
            Flavor::Windows
        } else {
            Flavor::Posix
        }
    }
    fn is_sep(self, c: char) -> bool {
        match self {
            Flavor::Windows => c == '\\' || c == '/',
            Flavor::Posix => c == '/',
        }
    }
    fn sep(self) -> char {
        match self {
            Flavor::Windows => '\\',
            Flavor::Posix => '/',
        }
    }
}

/// How names compare under a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseRule {
    Sensitive,
    Insensitive,
}

/// Why a path was refused. Shown to the user and sent back to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    Empty,
    NulByte,
    DeviceName(String),
    AlternateStream(String),
    /// `\\server\share`, `\\?\`, `\\.\` and similar forms.
    NetworkOrDevicePath,
    /// `C:foo` (drive-relative) or `\foo` (root of the current drive).
    AmbiguousRoot,
    /// `..` above the root.
    EscapesRoot,
    /// A relative path with no folder to resolve it against.
    Relative,
    OutsideApprovedFolders,
    /// Inside the app's own data, install or browser-profile folder.
    Forbidden,
    /// Resolution reached a network location.
    RemoteResolution,
    /// The path changed between preview and use (a symlink appeared or it moved).
    ChangedSincePreview,
    Io(String),
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::Empty => write!(f, "the path is empty"),
            PathError::NulByte => write!(f, "the path contains a NUL byte"),
            PathError::DeviceName(n) => write!(f, "\"{n}\" is a reserved Windows device name"),
            PathError::AlternateStream(n) => write!(f, "\"{n}\" names an alternate data stream"),
            PathError::NetworkOrDevicePath => write!(f, "network and device paths are not allowed"),
            PathError::AmbiguousRoot => {
                write!(f, "the path is relative to a drive; use a full path")
            }
            PathError::EscapesRoot => write!(f, "the path goes above the root folder"),
            PathError::Relative => write!(f, "the path is relative and there is no working folder"),
            PathError::OutsideApprovedFolders => {
                write!(f, "the path is outside the approved folders")
            }
            PathError::Forbidden => write!(f, "the app's own folders are never reachable"),
            PathError::RemoteResolution => write!(f, "the path resolves to a network location"),
            PathError::ChangedSincePreview => write!(f, "the path changed after it was approved"),
            PathError::Io(e) => write!(f, "the path could not be resolved: {e}"),
        }
    }
}

impl std::error::Error for PathError {}

/// A path after the lexical check: absolute, separators normalised, no dot segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexicalPath {
    /// `C:` / `\\server\share` on Windows; empty on POSIX.
    pub prefix: String,
    pub components: Vec<String>,
    pub flavor: Flavor,
}

impl LexicalPath {
    pub fn to_string_lossless(&self) -> String {
        let sep = self.flavor.sep();
        let mut s = self.prefix.clone();
        if self.components.is_empty() {
            s.push(sep);
        }
        for c in &self.components {
            s.push(sep);
            s.push_str(c);
        }
        s
    }
    fn is_unc(&self) -> bool {
        self.prefix.starts_with("\\\\")
    }
}

impl fmt::Display for LexicalPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_lossless())
    }
}

const DEVICE_NAMES: &[&str] = &["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "CLOCK$"];

fn is_device_name(component: &str) -> bool {
    // Windows ignores everything from the first dot, and trailing spaces/dots.
    let stem = component
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches([' ', '.']);
    let upper = stem.to_uppercase();
    if DEVICE_NAMES.contains(&upper.as_str()) {
        return true;
    }
    let digit_suffix = |p: &str| {
        upper.strip_prefix(p).is_some_and(|rest| {
            matches!(
                rest,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³" | "0"
            )
        })
    };
    digit_suffix("COM") || digit_suffix("LPT")
}

/// Step 2: the lexical check. `base` resolves relative paths (the working folder).
pub fn lexical(
    path: &str,
    base: Option<&LexicalPath>,
    flavor: Flavor,
) -> Result<LexicalPath, PathError> {
    if path.is_empty() {
        return Err(PathError::Empty);
    }
    if path.contains('\0') {
        return Err(PathError::NulByte);
    }
    let (prefix, rest, absolute) = match flavor {
        Flavor::Posix => {
            let abs = path.starts_with('/');
            (String::new(), path, abs)
        }
        Flavor::Windows => {
            let bytes: Vec<char> = path.chars().collect();
            let sep = |i: usize| bytes.get(i).is_some_and(|&c| flavor.is_sep(c));
            if sep(0) && sep(1) {
                // \\server\share, \\?\..., \\.\..., //server/share: refused here; an
                // approved folder on a share is matched by `lexical_unc`.
                return lexical_unc(path, flavor);
            }
            if bytes.len() >= 2 && bytes[1] == ':' && bytes[0].is_ascii_alphabetic() {
                if !sep(2) && bytes.len() > 2 {
                    return Err(PathError::AmbiguousRoot);
                }
                let drive = format!("{}:", bytes[0].to_ascii_uppercase());
                (drive, &path[2..], true)
            } else if sep(0) {
                return Err(PathError::AmbiguousRoot);
            } else {
                (String::new(), path, false)
            }
        }
    };

    let mut out: Vec<String> = Vec::new();
    let mut out_prefix = prefix;
    if !absolute {
        let base = base.ok_or(PathError::Relative)?;
        if base.flavor != flavor {
            return Err(PathError::Relative);
        }
        out_prefix = base.prefix.clone();
        out = base.components.clone();
    }
    for comp in rest.split(|c| flavor.is_sep(c)) {
        match comp {
            "" | "." => continue,
            ".." => {
                if out.pop().is_none() {
                    return Err(PathError::EscapesRoot);
                }
            }
            c => {
                if flavor == Flavor::Windows {
                    if c.contains(':') {
                        return Err(PathError::AlternateStream(c.to_string()));
                    }
                    if is_device_name(c) {
                        return Err(PathError::DeviceName(c.to_string()));
                    }
                    if c.chars().any(|ch| {
                        matches!(ch, '<' | '>' | '"' | '|' | '?' | '*') || ch.is_control()
                    }) {
                        return Err(PathError::Io(format!("\"{c}\" is not a valid file name")));
                    }
                    // Windows strips trailing dots and spaces, so "a." and "a" are the
                    // same file; refuse the ambiguous spelling.
                    if c.ends_with('.') || c.ends_with(' ') {
                        return Err(PathError::Io(format!("\"{c}\" ends with a dot or space")));
                    }
                }
                out.push(c.to_string());
            }
        }
    }
    Ok(LexicalPath {
        prefix: out_prefix,
        components: out,
        flavor,
    })
}

/// `\\server\share\rest` → prefix `\\server\share`. Device namespaces (`\\?\`, `\\.\`)
/// are always refused.
fn lexical_unc(path: &str, flavor: Flavor) -> Result<LexicalPath, PathError> {
    let parts: Vec<&str> = path.split(|c| flavor.is_sep(c)).collect();
    // parts: ["", "", server, share, rest...]
    let server = parts.get(2).copied().unwrap_or("");
    let share = parts.get(3).copied().unwrap_or("");
    if server.is_empty()
        || share.is_empty()
        || server == "?"
        || server == "."
        || server.contains(':')
    {
        return Err(PathError::NetworkOrDevicePath);
    }
    let rest = parts[4..].join("\\");
    let mut lp = lexical(&format!("C:\\{rest}"), None, flavor)?;
    lp.prefix = format!("\\\\{}\\{}", server.to_lowercase(), share.to_lowercase());
    Ok(lp)
}

fn eq_component(a: &str, b: &str, case: CaseRule) -> bool {
    match case {
        CaseRule::Sensitive => a == b,
        CaseRule::Insensitive => a.to_lowercase() == b.to_lowercase(),
    }
}

/// Whether `inner` is `outer` or inside it, component by component.
pub fn is_within(inner: &LexicalPath, outer: &LexicalPath, case: CaseRule) -> bool {
    let prefix_case = if inner.flavor == Flavor::Windows {
        CaseRule::Insensitive
    } else {
        case
    };
    inner.flavor == outer.flavor
        && eq_component(&inner.prefix, &outer.prefix, prefix_case)
        && inner.components.len() >= outer.components.len()
        && outer
            .components
            .iter()
            .zip(&inner.components)
            .all(|(o, i)| eq_component(o, i, case))
}

/// File-system access used by the sandbox. Tests substitute a fake.
pub trait Fs {
    /// Fully resolved path (symlinks followed). `Ok(None)` if it does not exist.
    fn canonicalize(&self, path: &str) -> Result<Option<String>, String>;
    /// Whether the final component is itself a symlink (or junction). `None` if missing.
    fn is_symlink(&self, path: &str) -> Option<bool>;
}

/// The real file system.
pub struct RealFs;

impl Fs for RealFs {
    fn canonicalize(&self, path: &str) -> Result<Option<String>, String> {
        match std::fs::canonicalize(path) {
            Ok(p) => {
                // On Windows std returns the \\?\ verbatim form; dunce strips it when it is
                // safe to, and leaves \\?\UNC\ (remote) forms intact so they are refused.
                let p = dunce::simplified(&p).to_string_lossy().into_owned();
                Ok(Some(p))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn is_symlink(&self, path: &str) -> Option<bool> {
        std::fs::symlink_metadata(path)
            .ok()
            .map(|m| m.file_type().is_symlink())
    }
}

/// An approved folder, with its canonical form and case rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub lexical: LexicalPath,
    pub canonical: LexicalPath,
    pub case: CaseRule,
}

/// A path that passed every check, ready to show on a card and to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedPath {
    /// The canonical path the tool must use.
    pub canonical: String,
    /// Index of the approved folder that contains it (the grant scope).
    pub root: usize,
    pub exists: bool,
    pub was_symlink: bool,
}

/// The sandbox for one chat: approved folders plus folders that are never reachable.
#[derive(Debug, Clone)]
pub struct PathSandbox {
    flavor: Flavor,
    roots: Vec<Root>,
    forbidden: Vec<(LexicalPath, CaseRule)>,
}

/// Default case rule for a folder on this OS when the volume cannot be asked.
pub fn default_case(flavor: Flavor) -> CaseRule {
    match flavor {
        Flavor::Windows => CaseRule::Insensitive,
        Flavor::Posix => {
            if cfg!(target_os = "macos") {
                CaseRule::Insensitive
            } else {
                CaseRule::Sensitive
            }
        }
    }
}

/// Whether the volume holding `path` compares names case-insensitively
/// (`pathconf(_PC_CASE_SENSITIVE)` on macOS; Windows is always insensitive).
pub fn volume_case(path: &str) -> CaseRule {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        if let Ok(c) = CString::new(path) {
            // SAFETY: `c` is a valid NUL-terminated string; pathconf only reads it.
            let r = unsafe { libc::pathconf(c.as_ptr(), libc::_PC_CASE_SENSITIVE) };
            return if r == 0 {
                CaseRule::Insensitive
            } else {
                CaseRule::Sensitive
            };
        }
        CaseRule::Insensitive
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        default_case(Flavor::native())
    }
}

impl PathSandbox {
    /// Builds a sandbox. Approved folders must exist; ones that cannot be resolved are
    /// skipped (and reported). `forbidden` lists the app's data, install and
    /// browser-profile folders.
    pub fn new(
        flavor: Flavor,
        approved: &[String],
        forbidden: &[String],
        fs: &dyn Fs,
        case_of: &dyn Fn(&str) -> CaseRule,
    ) -> (Self, Vec<(String, PathError)>) {
        let mut roots = Vec::new();
        let mut skipped = Vec::new();
        for a in approved {
            let result = (|| {
                let lex = lexical(a, None, flavor)?;
                if flavor == Flavor::Windows && lex.is_unc() {
                    // An approved share is allowed; its canonical form is itself.
                    return Ok(Root {
                        canonical: lex.clone(),
                        lexical: lex,
                        case: CaseRule::Insensitive,
                    });
                }
                let canon = fs
                    .canonicalize(&lex.to_string())
                    .map_err(PathError::Io)?
                    .ok_or_else(|| PathError::Io("the folder does not exist".into()))?;
                let canonical = lexical(&canon, None, flavor)?;
                Ok(Root {
                    case: case_of(&canon),
                    lexical: lex,
                    canonical,
                })
            })();
            match result {
                Ok(r) => roots.push(r),
                Err(e) => skipped.push((a.clone(), e)),
            }
        }
        let mut forb = Vec::new();
        for f in forbidden {
            if let Ok(lex) = lexical(f, None, flavor) {
                forb.push((lex.clone(), default_case(flavor)));
                if let Ok(Some(c)) = fs.canonicalize(&lex.to_string())
                    && let Ok(cl) = lexical(&c, None, flavor)
                {
                    forb.push((cl, default_case(flavor)));
                }
            }
        }
        (
            Self {
                flavor,
                roots,
                forbidden: forb,
            },
            skipped,
        )
    }

    pub fn roots(&self) -> &[Root] {
        &self.roots
    }

    fn forbidden_hit(&self, p: &LexicalPath) -> bool {
        // Refused if the path is inside a forbidden folder, or if a forbidden folder is
        // inside the path (a working directory of the app's parent would expose it).
        self.forbidden.iter().any(|(f, case)| {
            let case = if self.flavor == Flavor::Windows {
                CaseRule::Insensitive
            } else {
                *case
            };
            is_within(p, f, case)
        })
    }

    /// Steps 1–4. `base` is the folder relative paths resolve against.
    pub fn check(
        &self,
        path: &str,
        base: Option<&str>,
        fs: &dyn Fs,
    ) -> Result<CheckedPath, PathError> {
        let base_lex = match base {
            Some(b) => Some(lexical(b, None, self.flavor)?),
            None => None,
        };
        let lex = lexical(path, base_lex.as_ref(), self.flavor)?;
        // Step 1 (lexical part) and step 2.
        if self.forbidden_hit(&lex) {
            return Err(PathError::Forbidden);
        }
        let lexical_root = self.roots.iter().position(|r| {
            is_within(&lex, &r.lexical, r.case) || is_within(&lex, &r.canonical, r.case)
        });
        let Some(_) = lexical_root else {
            return Err(if lex.is_unc() {
                PathError::NetworkOrDevicePath
            } else {
                PathError::OutsideApprovedFolders
            });
        };

        // Step 3: canonicalise, or the deepest existing ancestor plus the remainder.
        let (canon, exists) = self.canonicalize_with_ancestors(&lex, fs)?;
        if self.flavor == Flavor::Windows {
            let s = canon.to_string();
            let approved_share = canon.is_unc()
                && self.roots.iter().any(|r| {
                    r.canonical.is_unc() && is_within(&canon, &r.canonical, CaseRule::Insensitive)
                });
            if (s.starts_with("\\\\") && !approved_share) || s.starts_with("\\\\?\\") {
                return Err(PathError::RemoteResolution);
            }
        }
        // Step 1 again on the resolved path, then step 4.
        if self.forbidden_hit(&canon) {
            return Err(PathError::Forbidden);
        }
        let root = self
            .roots
            .iter()
            .position(|r| is_within(&canon, &r.canonical, r.case))
            .ok_or(PathError::OutsideApprovedFolders)?;
        let canonical = canon.to_string();
        let was_symlink = fs.is_symlink(&lex.to_string()).unwrap_or(false);
        Ok(CheckedPath {
            canonical,
            root,
            exists,
            was_symlink,
        })
    }

    fn canonicalize_with_ancestors(
        &self,
        lex: &LexicalPath,
        fs: &dyn Fs,
    ) -> Result<(LexicalPath, bool), PathError> {
        let mut existing = lex.clone();
        let mut remainder: Vec<String> = Vec::new();
        loop {
            match fs
                .canonicalize(&existing.to_string())
                .map_err(PathError::Io)?
            {
                Some(c) => {
                    let mut canon =
                        lexical(&c, None, self.flavor).map_err(|_| PathError::RemoteResolution)?;
                    let exists = remainder.is_empty();
                    remainder.reverse();
                    canon.components.extend(remainder);
                    return Ok((canon, exists));
                }
                None => {
                    let Some(last) = existing.components.pop() else {
                        return Err(PathError::Io("no part of the path exists".into()));
                    };
                    remainder.push(last);
                }
            }
        }
    }

    /// Step 5: immediately before the side effect, resolve again and refuse if the
    /// result differs from what was approved or a symlink appeared at the end.
    pub fn recheck(&self, approved: &CheckedPath, fs: &dyn Fs) -> Result<(), PathError> {
        let again = self.check(&approved.canonical, None, fs)?;
        let same = {
            let a = lexical(&again.canonical, None, self.flavor)?;
            let b = lexical(&approved.canonical, None, self.flavor)?;
            let case = self
                .roots
                .get(again.root)
                .map_or(CaseRule::Sensitive, |r| r.case);
            a.components.len() == b.components.len() && is_within(&a, &b, case)
        };
        if !same || (again.was_symlink && !approved.was_symlink) {
            return Err(PathError::ChangedSincePreview);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::BTreeMap;

    /// An in-memory file system: existing paths map to their resolved target.
    #[derive(Default)]
    pub struct FakeFs {
        pub entries: BTreeMap<String, String>,
        pub symlinks: Vec<String>,
        pub calls: Cell<usize>,
        pub flavor: Option<Flavor>,
    }

    impl FakeFs {
        pub fn posix(paths: &[&str]) -> Self {
            let mut fs = FakeFs {
                flavor: Some(Flavor::Posix),
                ..Default::default()
            };
            for p in paths {
                fs.entries.insert(p.to_string(), p.to_string());
            }
            fs
        }
        pub fn windows(paths: &[&str]) -> Self {
            let mut fs = FakeFs {
                flavor: Some(Flavor::Windows),
                ..Default::default()
            };
            for p in paths {
                fs.entries.insert(p.to_lowercase(), p.to_string());
            }
            fs
        }
        pub fn link(mut self, from: &str, to: &str) -> Self {
            let key = if self.flavor == Some(Flavor::Windows) {
                from.to_lowercase()
            } else {
                from.to_string()
            };
            self.entries.insert(key, to.to_string());
            self.symlinks.push(from.to_string());
            self
        }
        fn key(&self, path: &str) -> String {
            if self.flavor == Some(Flavor::Windows) {
                path.to_lowercase()
            } else {
                path.to_string()
            }
        }
        /// Resolves `path` by resolving each prefix (so links in the middle work).
        fn resolve(&self, path: &str) -> Option<String> {
            let flavor = self.flavor.unwrap_or(Flavor::Posix);
            let lex = lexical(path, None, flavor).ok()?;
            let mut cur = LexicalPath {
                prefix: lex.prefix.clone(),
                components: vec![],
                flavor,
            };
            for c in &lex.components {
                cur.components.push(c.clone());
                let target = self.entries.get(&self.key(&cur.to_string()))?;
                cur = lexical(target, None, flavor).ok()?;
            }
            Some(cur.to_string())
        }
    }

    impl Fs for FakeFs {
        fn canonicalize(&self, path: &str) -> Result<Option<String>, String> {
            self.calls.set(self.calls.get() + 1);
            Ok(self.resolve(path))
        }
        fn is_symlink(&self, path: &str) -> Option<bool> {
            self.calls.set(self.calls.get() + 1);
            let k = self.key(path);
            Some(self.symlinks.iter().any(|s| self.key(s) == k))
        }
    }

    pub fn posix_sandbox(fs: &FakeFs, approved: &[&str], forbidden: &[&str]) -> PathSandbox {
        let a: Vec<String> = approved.iter().map(|s| s.to_string()).collect();
        let f: Vec<String> = forbidden.iter().map(|s| s.to_string()).collect();
        let (sb, skipped) = PathSandbox::new(Flavor::Posix, &a, &f, fs, &|_| CaseRule::Sensitive);
        assert!(skipped.is_empty(), "{skipped:?}");
        sb
    }

    fn win_sandbox(fs: &FakeFs, approved: &[&str], forbidden: &[&str]) -> PathSandbox {
        let a: Vec<String> = approved.iter().map(|s| s.to_string()).collect();
        let f: Vec<String> = forbidden.iter().map(|s| s.to_string()).collect();
        let (sb, skipped) =
            PathSandbox::new(Flavor::Windows, &a, &f, fs, &|_| CaseRule::Insensitive);
        assert!(skipped.is_empty(), "{skipped:?}");
        sb
    }

    #[test]
    fn lexical_normalises_posix() {
        let p = lexical("/home/u/proj/./src/../lib//x.rs", None, Flavor::Posix).unwrap();
        assert_eq!(p.to_string(), "/home/u/proj/lib/x.rs");
        assert_eq!(
            lexical("/..", None, Flavor::Posix),
            Err(PathError::EscapesRoot)
        );
        assert_eq!(
            lexical("a/b", None, Flavor::Posix),
            Err(PathError::Relative)
        );
        let base = lexical("/w", None, Flavor::Posix).unwrap();
        assert_eq!(
            lexical("a/../b", Some(&base), Flavor::Posix)
                .unwrap()
                .to_string(),
            "/w/b"
        );
        assert_eq!(
            lexical("../../x", Some(&base), Flavor::Posix),
            Err(PathError::EscapesRoot)
        );
        assert_eq!(
            lexical("a\0b", Some(&base), Flavor::Posix),
            Err(PathError::NulByte)
        );
        assert_eq!(lexical("", None, Flavor::Posix), Err(PathError::Empty));
        // Backslash is an ordinary character on POSIX.
        assert_eq!(
            lexical("/a\\..\\b", None, Flavor::Posix)
                .unwrap()
                .components,
            vec!["a\\..\\b"]
        );
    }

    #[test]
    fn lexical_windows_rules() {
        let w = Flavor::Windows;
        assert_eq!(
            lexical("c:/Users/me/x", None, w).unwrap().to_string(),
            "C:\\Users\\me\\x"
        );
        assert_eq!(
            lexical("C:\\a\\..\\b", None, w).unwrap().to_string(),
            "C:\\b"
        );
        assert_eq!(lexical("C:foo", None, w), Err(PathError::AmbiguousRoot));
        assert_eq!(lexical("\\foo", None, w), Err(PathError::AmbiguousRoot));
        for dev in [
            "C:\\x\\CON",
            "C:\\x\\nul.txt",
            "C:\\x\\COM1",
            "C:\\x\\lpt9.log",
            "C:\\x\\com¹",
            "C:\\x\\AUX .txt",
            "C:\\x\\CONIN$",
        ] {
            assert!(
                matches!(lexical(dev, None, w), Err(PathError::DeviceName(_))),
                "{dev}"
            );
        }
        assert!(lexical("C:\\x\\CONSOLE.txt", None, w).is_ok());
        assert!(lexical("C:\\x\\COM10", None, w).is_ok());
        assert!(matches!(
            lexical("C:\\x\\file.txt:secret", None, w),
            Err(PathError::AlternateStream(_))
        ));
        assert!(matches!(
            lexical("C:\\x\\file.txt::$DATA", None, w),
            Err(PathError::AlternateStream(_))
        ));
        assert!(lexical("C:\\x\\a.", None, w).is_err());
        assert!(lexical("C:\\x\\a ", None, w).is_err());
        assert!(lexical("C:\\x\\a*b", None, w).is_err());
        for dev in [
            "\\\\?\\C:\\x",
            "\\\\.\\PhysicalDrive0",
            "//?/C:/x",
            "\\\\",
            "\\\\server",
        ] {
            assert_eq!(
                lexical(dev, None, w),
                Err(PathError::NetworkOrDevicePath),
                "{dev}"
            );
        }
        let unc = lexical("\\\\Server\\Share\\a\\..\\b", None, w).unwrap();
        assert_eq!(unc.to_string(), "\\\\server\\share\\b");
    }

    #[test]
    fn dotdot_escape_is_refused() {
        let fs = FakeFs::posix(&["/home", "/home/u", "/home/u/proj", "/home/u/secret"]);
        let sb = posix_sandbox(&fs, &["/home/u/proj"], &[]);
        assert_eq!(
            sb.check("/home/u/proj/../secret", None, &fs),
            Err(PathError::OutsideApprovedFolders)
        );
        assert_eq!(
            sb.check("../secret", Some("/home/u/proj"), &fs),
            Err(PathError::OutsideApprovedFolders)
        );
        assert!(sb.check("/home/u/proj/new.txt", None, &fs).is_ok());
        // Prefix that is not a component boundary.
        let fs2 = FakeFs::posix(&["/home", "/home/u", "/home/u/proj", "/home/u/project2"]);
        let sb2 = posix_sandbox(&fs2, &["/home/u/proj"], &[]);
        assert_eq!(
            sb2.check("/home/u/project2/x", None, &fs2),
            Err(PathError::OutsideApprovedFolders)
        );
    }

    #[test]
    fn symlink_pointing_out_is_refused() {
        let fs = FakeFs::posix(&["/home", "/home/u", "/home/u/proj", "/etc", "/etc/passwd"])
            .link("/home/u/proj/link", "/etc");
        let sb = posix_sandbox(&fs, &["/home/u/proj"], &[]);
        assert_eq!(
            sb.check("/home/u/proj/link/passwd", None, &fs),
            Err(PathError::OutsideApprovedFolders)
        );
        // Writing a new file through the link is refused too.
        assert_eq!(
            sb.check("/home/u/proj/link/new", None, &fs),
            Err(PathError::OutsideApprovedFolders)
        );
    }

    #[test]
    fn symlink_staying_inside_is_allowed() {
        let fs = FakeFs::posix(&["/p", "/p/real", "/p/real/f"]).link("/p/alias", "/p/real");
        let sb = posix_sandbox(&fs, &["/p"], &[]);
        let c = sb.check("/p/alias/f", None, &fs).unwrap();
        assert_eq!(c.canonical, "/p/real/f");
    }

    #[test]
    fn approved_folder_given_through_a_link() {
        // macOS: /tmp -> /private/tmp. Paths spelled either way are inside.
        let fs = FakeFs::posix(&[
            "/private",
            "/private/tmp",
            "/private/tmp/w",
            "/private/tmp/w/a",
        ])
        .link("/tmp", "/private/tmp");
        let sb = posix_sandbox(&fs, &["/tmp/w"], &[]);
        assert!(sb.check("/tmp/w/a", None, &fs).is_ok());
        assert!(sb.check("/private/tmp/w/a", None, &fs).is_ok());
    }

    #[test]
    fn windows_junction_out_is_refused() {
        let fs = FakeFs::windows(&[
            "C:\\Users",
            "C:\\Users\\me",
            "C:\\Users\\me\\proj",
            "C:\\Windows",
            "C:\\Windows\\System32",
        ])
        .link("C:\\Users\\me\\proj\\j", "C:\\Windows");
        let sb = win_sandbox(&fs, &["C:\\Users\\me\\proj"], &[]);
        assert_eq!(
            sb.check("C:\\Users\\me\\proj\\j\\System32", None, &fs),
            Err(PathError::OutsideApprovedFolders)
        );
    }

    #[test]
    fn windows_case_tricks() {
        let fs = FakeFs::windows(&[
            "C:\\Users",
            "C:\\Users\\me",
            "C:\\Users\\me\\Proj",
            "C:\\Users\\me\\Proj\\a.txt",
        ]);
        let sb = win_sandbox(&fs, &["C:\\Users\\me\\Proj"], &[]);
        assert!(sb.check("c:\\users\\ME\\proj\\A.TXT", None, &fs).is_ok());
        assert!(sb.check("C:/USERS/me/PROJ/new.txt", None, &fs).is_ok());
    }

    #[test]
    fn posix_case_sensitive_volume() {
        let fs = FakeFs::posix(&["/v", "/v/Proj", "/v/proj", "/v/proj/x"]);
        let sb = posix_sandbox(&fs, &["/v/Proj"], &[]);
        assert_eq!(
            sb.check("/v/proj/x", None, &fs),
            Err(PathError::OutsideApprovedFolders)
        );
    }

    #[test]
    fn short_names_outside_lexically_are_refused() {
        // 8.3 names are only accepted where they resolve inside; a short name that is not
        // lexically under the approved folder is refused before any I/O.
        let fs = FakeFs::windows(&[
            "C:\\Program Files",
            "C:\\Users",
            "C:\\Users\\me",
            "C:\\Users\\me\\proj",
        ]);
        let sb = win_sandbox(&fs, &["C:\\Users\\me\\proj"], &[]);
        let before = fs.calls.get();
        assert_eq!(
            sb.check("C:\\PROGRA~1\\x", None, &fs),
            Err(PathError::OutsideApprovedFolders)
        );
        assert_eq!(fs.calls.get(), before);
        // A short-name spelling inside the approved folder that resolves elsewhere.
        let fs = FakeFs::windows(&[
            "C:\\Users",
            "C:\\Users\\me",
            "C:\\Users\\me\\proj",
            "C:\\Secret",
        ])
        .link("C:\\Users\\me\\proj\\SECRET~1", "C:\\Secret");
        let sb = win_sandbox(&fs, &["C:\\Users\\me\\proj"], &[]);
        assert_eq!(
            sb.check("C:\\Users\\me\\proj\\SECRET~1\\k", None, &fs),
            Err(PathError::OutsideApprovedFolders)
        );
    }

    #[test]
    fn unc_argument_causes_no_io() {
        let fs = FakeFs::windows(&["C:\\Users", "C:\\Users\\me", "C:\\Users\\me\\proj"]);
        let sb = win_sandbox(&fs, &["C:\\Users\\me\\proj"], &[]);
        let before = fs.calls.get();
        for p in [
            "\\\\attacker\\share\\x",
            "//attacker/share/x",
            "\\\\?\\UNC\\attacker\\share\\x",
            "\\\\?\\C:\\Users\\me\\proj\\x",
            "\\\\.\\pipe\\x",
        ] {
            assert!(sb.check(p, None, &fs).is_err(), "{p}");
        }
        // Relative path with a UNC working folder.
        assert!(sb.check("x", Some("\\\\attacker\\share"), &fs).is_err());
        assert_eq!(
            fs.calls.get(),
            before,
            "a UNC path must not touch the file system"
        );
    }

    #[test]
    fn approved_share_is_usable() {
        let fs = FakeFs::windows(&[]);
        let sb = win_sandbox(&fs, &["\\\\nas\\team"], &[]);
        let mut fs2 = FakeFs::windows(&[]);
        fs2.entries.insert(
            "\\\\nas\\team\\doc.txt".into(),
            "\\\\nas\\team\\doc.txt".into(),
        );
        fs2.entries
            .insert("\\\\nas\\team".into(), "\\\\nas\\team".into());
        let c = sb.check("\\\\NAS\\Team\\doc.txt", None, &fs2).unwrap();
        assert_eq!(c.canonical, "\\\\nas\\team\\doc.txt");
        assert!(sb.check("\\\\other\\team\\doc.txt", None, &fs2).is_err());
    }

    #[test]
    fn resolution_to_a_share_is_refused() {
        let fs = FakeFs::windows(&["C:\\p"]).link("C:\\p\\net", "\\\\evil\\s");
        let sb = win_sandbox(&fs, &["C:\\p"], &[]);
        assert!(matches!(
            sb.check("C:\\p\\net\\x", None, &fs),
            Err(PathError::RemoteResolution | PathError::OutsideApprovedFolders)
        ));
    }

    #[test]
    fn forbidden_folders_beat_approval() {
        let fs = FakeFs::posix(&[
            "/home",
            "/home/u",
            "/home/u/.local",
            "/home/u/.local/share",
            "/home/u/.local/share/app",
            "/home/u/.local/share/app/app.db",
        ]);
        let sb = posix_sandbox(&fs, &["/home/u"], &["/home/u/.local/share/app"]);
        assert_eq!(
            sb.check("/home/u/.local/share/app/app.db", None, &fs),
            Err(PathError::Forbidden)
        );
        // A link to the data folder from an approved place.
        let fs = FakeFs::posix(&["/home", "/home/u", "/data", "/data/app.db"])
            .link("/home/u/d", "/data");
        let sb = posix_sandbox(&fs, &["/home/u"], &["/data"]);
        assert_eq!(
            sb.check("/home/u/d/app.db", None, &fs),
            Err(PathError::Forbidden)
        );
    }

    #[test]
    fn new_file_under_missing_folders() {
        let fs = FakeFs::posix(&["/p"]);
        let sb = posix_sandbox(&fs, &["/p"], &[]);
        let c = sb.check("/p/a/b/c.txt", None, &fs).unwrap();
        assert_eq!(c.canonical, "/p/a/b/c.txt");
        assert!(!c.exists);
    }

    #[test]
    fn recheck_detects_a_new_symlink() {
        let fs = FakeFs::posix(&["/p", "/p/f", "/etc", "/etc/x"]);
        let sb = posix_sandbox(&fs, &["/p"], &[]);
        let approved = sb.check("/p/f", None, &fs).unwrap();
        assert!(sb.recheck(&approved, &fs).is_ok());
        // Between preview and write, f becomes a link to a file outside.
        let fs2 = FakeFs::posix(&["/p", "/etc", "/etc/x"]).link("/p/f", "/etc/x");
        assert!(sb.recheck(&approved, &fs2).is_err());
        // Or a link that stays inside but was not there at preview.
        let fs3 = FakeFs::posix(&["/p", "/p/g"]).link("/p/f", "/p/g");
        assert!(sb.recheck(&approved, &fs3).is_err());
    }

    #[test]
    fn missing_approved_folder_is_skipped() {
        let fs = FakeFs::posix(&[]);
        let (sb, skipped) = PathSandbox::new(Flavor::Posix, &["/gone".into()], &[], &fs, &|_| {
            CaseRule::Sensitive
        });
        assert_eq!(skipped.len(), 1);
        assert!(sb.roots().is_empty());
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        fn component() -> impl Strategy<Value = String> {
            prop_oneof![
                Just("..".to_string()),
                Just(".".to_string()),
                Just(String::new()),
                Just("proj".to_string()),
                Just("secret".to_string()),
                Just("CON".to_string()),
                Just("a:b".to_string()),
                Just("link".to_string()),
                "[a-z]{1,3}".prop_map(|s| s),
            ]
        }

        fn sep() -> impl Strategy<Value = &'static str> {
            prop_oneof![Just("/"), Just("\\"), Just("//")]
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(2000))]

            /// Whatever the shape, an accepted path resolves inside the approved folder.
            #[test]
            fn accepted_paths_stay_inside_posix(parts in prop::collection::vec((component(), sep()), 0..8), abs in any::<bool>()) {
                let fs = FakeFs::posix(&["/r", "/r/proj", "/r/secret", "/r/proj/x"]).link("/r/proj/link", "/r/secret");
                let sb = posix_sandbox(&fs, &["/r/proj"], &[]);
                let mut p = if abs { "/r/proj/".to_string() } else { String::new() };
                for (c, s) in &parts { p.push_str(c); p.push_str(s); }
                if let Ok(c) = sb.check(&p, Some("/r/proj"), &fs) {
                    prop_assert!(c.canonical == "/r/proj" || c.canonical.starts_with("/r/proj/"), "{} -> {}", p, c.canonical);
                }
            }

            #[test]
            fn accepted_paths_stay_inside_windows(parts in prop::collection::vec((component(), sep()), 0..8), lead in prop_oneof![Just(""), Just("C:\\R\\proj\\"), Just("\\\\"), Just("c:/r/PROJ/"), Just("\\")]) {
                let fs = FakeFs::windows(&["C:\\R", "C:\\R\\proj", "C:\\R\\secret"]).link("C:\\R\\proj\\link", "C:\\R\\secret");
                let sb = win_sandbox(&fs, &["C:\\R\\proj"], &[]);
                let mut p = lead.to_string();
                for (c, s) in &parts { p.push_str(c); p.push_str(s); }
                if let Ok(c) = sb.check(&p, Some("C:\\R\\proj"), &fs) {
                    let lower = c.canonical.to_lowercase();
                    prop_assert!(lower == "c:\\r\\proj" || lower.starts_with("c:\\r\\proj\\"), "{} -> {}", p, c.canonical);
                }
            }

            /// UNC-looking input never reaches the file system.
            #[test]
            fn unc_never_does_io(server in "[a-z]{1,8}", share in "[a-z]{1,8}", rest in "[a-z/\\\\.]{0,20}", fwd in any::<bool>()) {
                let fs = FakeFs::windows(&["C:\\R", "C:\\R\\proj"]);
                let sb = win_sandbox(&fs, &["C:\\R\\proj"], &[]);
                let before = fs.calls.get();
                let p = if fwd { format!("//{server}/{share}/{rest}") } else { format!("\\\\{server}\\{share}\\{rest}") };
                prop_assert!(sb.check(&p, None, &fs).is_err());
                prop_assert_eq!(fs.calls.get(), before);
            }
        }
    }
}
