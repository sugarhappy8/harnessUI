//! Unpacking the pinned archives (item5-spec.md WP9a `archive.rs`): `.tar.gz` (rea, Node.js
//! for macOS and Linux, Kuna, Temurin for Linux) and `.zip` (Ghidra, Temurin and Node.js for
//! Windows) into a new folder, with a strip prefix and a keep list.
//!
//! The same rules as `browser::own::unpack_tree`, plus Windows device names, for both
//! formats:
//! - kept: Unix permission bits, masked to `(mode & 0o755) | 0o600` (no write for the
//!   group or others); relative links whose target stays inside the folder, created last,
//!   never through another link, and checked again on disk afterwards;
//! - refused: absolute paths, `..`, `.` and empty components, backslashes, NUL and other
//!   control characters, colons (drive letters, alternate streams), names ending in a dot or
//!   a space, Windows device names (`CON`, `NUL`, `COM1`…, with any extension), entries
//!   outside the strip prefix, links that are absolute or leave the folder, hard links,
//!   devices and pipes, a second entry for the same path (compared without case, as on
//!   Windows and macOS), and more entries or bytes than the lock's numbers plus 10 %.
//!
//! What an entry is comes from the archive's bytes alone, never from the system unpacking
//! it: a tar entry's type, and a zip entry's Unix mode as the system its archive names as
//! the maker records it (an entry made on MS-DOS has none and is a plain file or folder).
//!
//! Nothing here runs anything. Links are refused on Windows (creating them needs a
//! privilege most accounts lack); none of the Windows archives holds one.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

use crate::lock::{Build, Keep};

/// What one unpack may hold.
#[derive(Clone, Debug)]
pub struct Rules<'a> {
    /// Every entry must start with this; it is removed (`package/`). Empty for none.
    pub strip_prefix: &'a str,
    /// What is written.
    pub keep: &'a Keep,
    /// At most this many entries in the archive (all of them, kept or not).
    pub max_entries: usize,
    /// At most this many bytes written.
    pub max_bytes: u64,
}

impl<'a> Rules<'a> {
    /// The rules for a pinned build: its prefix and keep list, and its entry and byte counts
    /// plus 10 %.
    pub fn for_build(build: &'a Build) -> Self {
        Self {
            strip_prefix: &build.strip_prefix,
            keep: &build.keep,
            max_entries: build.entries + build.entries / 10 + 1,
            max_bytes: build.unpacked_bytes + build.unpacked_bytes / 10,
        }
    }
}

/// What was written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unpacked {
    /// Regular files.
    pub files: usize,
    /// Links.
    pub links: usize,
    /// Bytes of the files.
    pub bytes: u64,
}

/// Why an archive was not unpacked. When this is returned, the output folder holds a
/// partial tree and must be deleted.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    /// The archive breaks one of the rules, or is damaged.
    #[error("{0}")]
    Refused(String),
    /// The user cancelled.
    #[error("cancelled")]
    Cancelled,
    /// Writing failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn refused(message: impl Into<String>) -> ArchiveError {
    ArchiveError::Refused(message.into())
}

/// Unpacks `archive` (by its name: `.tar.gz`, `.tgz` or `.zip`) into the new folder `out`.
pub fn unpack(
    archive: &Path,
    out: &Path,
    rules: &Rules<'_>,
    cancel: &CancellationToken,
    progress: &dyn Fn(u64),
) -> Result<Unpacked, ArchiveError> {
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    // The downloads folder names parts `<asset>.<hash>.part`.
    let name = name.strip_suffix(".part").map_or(name.as_str(), |n| {
        n.rsplit_once('.').map_or(n, |(asset, _hash)| asset)
    });
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        unpack_tar_gz(archive, out, rules, cancel, progress)
    } else if name.ends_with(".zip") {
        unpack_zip(archive, out, rules, cancel, progress)
    } else {
        Err(refused(format!("unknown archive type: {name}")))
    }
}

/// Windows device names, refused as any path component (with or without an extension).
fn is_device_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .to_ascii_lowercase();
    let stem = stem.trim_end_matches(' ');
    matches!(stem, "con" | "prn" | "aux" | "nul" | "conin$" | "conout$")
        || ((stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit())
}

/// The components of a `/`-separated relative path that is safe on every system, or `None`.
pub fn safe_relative(name: &str) -> Option<Vec<&str>> {
    if name.is_empty()
        || name
            .chars()
            .any(|c| c == '\\' || c == ':' || c.is_control())
    {
        return None;
    }
    let parts: Vec<&str> = name.split('/').collect();
    parts
        .iter()
        .all(|p| {
            !p.is_empty()
                && *p != "."
                && *p != ".."
                && !p.ends_with('.')
                && !p.ends_with(' ')
                && !is_device_name(p)
        })
        .then_some(parts)
}

/// Where a link at `link` (components relative to the folder) lands when it points at
/// `target`, on the names alone: `None` when the target is absolute, malformed, or climbs
/// out of the folder (or is the folder itself).
fn resolve_link(link: &[String], target: &str) -> Option<Vec<String>> {
    if target.is_empty()
        || target.len() > 4096
        || target.starts_with('/')
        || target
            .chars()
            .any(|c| c == '\\' || c == ':' || c.is_control())
    {
        return None;
    }
    let mut at: Vec<String> = link[..link.len().saturating_sub(1)].to_vec();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                at.pop()?;
            }
            name => at.push(name.to_string()),
        }
    }
    (!at.is_empty()).then_some(at)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // Permission bits only (no set-user-id, set-group-id or sticky), no write for the group
    // or others (only the owner may change what the app runs), and the owner can always
    // read and replace the file, so the folder can be removed again.
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode((mode & 0o755) | 0o600),
    )
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn make_link(target: &str, at: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, at)
}

#[cfg(not(unix))]
fn make_link(_target: &str, _at: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "links in archives are not supported on this system",
    ))
}

/// The state of one unpack, shared by both formats.
struct Writer<'a> {
    out: PathBuf,
    rules: &'a Rules<'a>,
    left: u64,
    entries: usize,
    /// Kept files and links, lower-cased: a second entry for the same path is refused.
    seen: HashSet<String>,
    /// Exact relative paths written (the keep list must be complete).
    written: HashSet<String>,
    links: Vec<(Vec<String>, String)>,
    report: Unpacked,
}

impl<'a> Writer<'a> {
    fn new(out: &Path, rules: &'a Rules<'a>) -> std::io::Result<Self> {
        std::fs::create_dir_all(out)?;
        Ok(Self {
            out: out.to_path_buf(),
            rules,
            left: rules.max_bytes,
            entries: 0,
            seen: HashSet::new(),
            written: HashSet::new(),
            links: Vec::new(),
            report: Unpacked::default(),
        })
    }

    /// Counts one entry against the cap.
    fn count(&mut self) -> Result<(), ArchiveError> {
        self.entries += 1;
        if self.entries > self.rules.max_entries {
            return Err(refused(format!(
                "the archive has more than {} entries",
                self.rules.max_entries
            )));
        }
        Ok(())
    }

    /// The kept relative path of `raw`, `Ok(None)` for the prefix folder itself or an entry
    /// the keep list leaves out. Every name is checked, kept or not.
    fn relative(&self, raw: &str) -> Result<Option<(String, Vec<String>)>, ArchiveError> {
        let trimmed = raw.strip_suffix('/').unwrap_or(raw);
        let prefix = self.rules.strip_prefix;
        if !prefix.is_empty() && (trimmed == prefix.trim_end_matches('/') || raw == prefix) {
            return Ok(None);
        }
        let Some(rel) = trimmed.strip_prefix(prefix) else {
            return Err(refused(format!("{raw:?} is outside {prefix:?}")));
        };
        let Some(parts) = safe_relative(rel) else {
            return Err(refused(format!("unsafe path in archive: {raw:?}")));
        };
        let parts: Vec<String> = parts.into_iter().map(str::to_string).collect();
        Ok(Some((rel.to_string(), parts)))
    }

    fn target(&self, parts: &[String]) -> PathBuf {
        let mut t = self.out.clone();
        for p in parts {
            t.push(p);
        }
        t
    }

    fn claim(&mut self, rel: &str) -> Result<(), ArchiveError> {
        if !self.seen.insert(rel.to_lowercase()) {
            return Err(refused(format!("{rel:?} is in the archive twice")));
        }
        Ok(())
    }

    fn dir(&mut self, rel: &str, parts: &[String]) -> Result<(), ArchiveError> {
        if self.rules.keep.is_all() {
            std::fs::create_dir_all(self.target(parts))?;
        }
        let _ = rel;
        Ok(())
    }

    fn file(
        &mut self,
        rel: &str,
        parts: &[String],
        data: &mut dyn Read,
        mode: Option<u32>,
        progress: &dyn Fn(u64),
    ) -> Result<(), ArchiveError> {
        if !self.rules.keep.keeps(rel) {
            return Ok(());
        }
        self.claim(rel)?;
        let target = self.target(parts);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // `create_new`: nothing already there is overwritten or written through.
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    refused(format!("{rel:?} is in the archive twice"))
                } else {
                    ArchiveError::Io(e)
                }
            })?;
        let copied = std::io::copy(&mut data.take(self.left.saturating_add(1)), &mut f)?;
        if copied > self.left {
            return Err(refused(format!(
                "the archive unpacks to more than {} bytes",
                self.rules.max_bytes
            )));
        }
        self.left -= copied;
        drop(f);
        if let Some(mode) = mode {
            set_mode(&target, mode)?;
        }
        self.report.files += 1;
        self.report.bytes += copied;
        self.written.insert(rel.to_string());
        progress(self.report.bytes);
        Ok(())
    }

    fn link(&mut self, rel: &str, parts: Vec<String>, text: String) -> Result<(), ArchiveError> {
        if !self.rules.keep.keeps(rel) {
            return Ok(());
        }
        if resolve_link(&parts, &text).is_none() {
            return Err(refused(format!(
                "the link {rel:?} points outside the archive ({:?})",
                text.chars().take(200).collect::<String>()
            )));
        }
        self.claim(rel)?;
        self.written.insert(rel.to_string());
        self.links.push((parts, text));
        Ok(())
    }

    /// Links last, then the second check on disk and the keep list's completeness.
    fn finish(mut self) -> Result<Unpacked, ArchiveError> {
        for (parts, text) in &self.links {
            let mut at = self.out.clone();
            // A link's own path must not run through a link made earlier in this loop.
            for p in &parts[..parts.len().saturating_sub(1)] {
                at.push(p);
                if at
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink())
                {
                    return Err(refused(format!(
                        "the link {:?} would be created through another link",
                        parts.join("/")
                    )));
                }
            }
            if let Some(last) = parts.last() {
                at.push(last);
            }
            if let Some(parent) = at.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if at.symlink_metadata().is_ok() {
                return Err(refused(format!(
                    "{:?} is in the archive twice",
                    parts.join("/")
                )));
            }
            make_link(text, &at).map_err(|e| {
                refused(format!(
                    "the link {:?} could not be created: {e}",
                    parts.join("/")
                ))
            })?;
            self.report.links += 1;
        }
        // Every link must lead to something inside the folder, however many links it passes.
        if !self.links.is_empty() {
            let root = std::fs::canonicalize(&self.out)?;
            for (parts, text) in &self.links {
                let at = self.target(parts);
                let inside = std::fs::canonicalize(&at).is_ok_and(|real| real.starts_with(&root));
                if !inside {
                    return Err(refused(format!(
                        "the link {:?} points outside the archive or at nothing ({text:?})",
                        parts.join("/")
                    )));
                }
            }
        }
        if let Keep::Files(files) = self.rules.keep
            && let Some(missing) = files.iter().find(|f| !self.written.contains(*f))
        {
            return Err(refused(format!("{missing:?} is missing from the archive")));
        }
        Ok(self.report)
    }
}

const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFLNK: u32 = 0o120_000;

/// Unpacks a `.tar.gz` into the new folder `out`.
pub fn unpack_tar_gz(
    archive: &Path,
    out: &Path,
    rules: &Rules<'_>,
    cancel: &CancellationToken,
    progress: &dyn Fn(u64),
) -> Result<Unpacked, ArchiveError> {
    let file = std::fs::File::open(archive)?;
    let gz = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
    let mut tar = tar::Archive::new(gz);
    let mut w = Writer::new(out, rules)?;
    let entries = tar
        .entries()
        .map_err(|e| refused(format!("damaged archive: {e}")))?;
    for entry in entries {
        if cancel.is_cancelled() {
            return Err(ArchiveError::Cancelled);
        }
        let mut entry = entry.map_err(|e| refused(format!("damaged archive: {e}")))?;
        let kind = entry.header().entry_type();
        // Extension headers carry no file of their own.
        if kind.is_pax_global_extensions()
            || kind.is_pax_local_extensions()
            || kind.is_gnu_longname()
            || kind.is_gnu_longlink()
        {
            continue;
        }
        w.count()?;
        let raw_bytes = entry.path_bytes().into_owned();
        let raw = String::from_utf8(raw_bytes)
            .map_err(|_| refused("an entry's name is not valid text"))?;
        if !(kind.is_file() || kind.is_dir() || kind.is_symlink()) {
            return Err(refused(format!("{raw:?} is not a file, folder or link")));
        }
        let Some((rel, parts)) = w.relative(&raw)? else {
            continue;
        };
        if kind.is_dir() {
            w.dir(&rel, &parts)?;
        } else if kind.is_symlink() {
            let text = entry
                .link_name_bytes()
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            w.link(&rel, parts, text)?;
        } else {
            let mode = entry.header().mode().ok();
            w.file(&rel, &parts, &mut entry, mode, progress)?;
        }
    }
    w.finish()
}

/// Unpacks a `.zip` into the new folder `out`.
pub fn unpack_zip(
    archive: &Path,
    out: &Path,
    rules: &Rules<'_>,
    cancel: &CancellationToken,
    progress: &dyn Fn(u64),
) -> Result<Unpacked, ArchiveError> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| refused(format!("damaged archive: {e}")))?;
    if zip.len() > rules.max_entries {
        return Err(refused(format!(
            "the archive has more than {} entries",
            rules.max_entries
        )));
    }
    let mut w = Writer::new(out, rules)?;
    for i in 0..zip.len() {
        if cancel.is_cancelled() {
            return Err(ArchiveError::Cancelled);
        }
        let mut entry = zip
            .by_index(i)
            .map_err(|e| refused(format!("damaged archive: {e}")))?;
        w.count()?;
        let raw = entry
            .name()
            .map_err(|e| refused(format!("an entry's name is not valid text: {e}")))?
            .into_owned();
        let mode = entry.unix_mode();
        let kind = mode.map(|m| m & S_IFMT);
        if let Some(k) = kind
            && ![0, S_IFREG, S_IFDIR, S_IFLNK].contains(&k)
        {
            return Err(refused(format!("{raw:?} is not a file, folder or link")));
        }
        let Some((rel, parts)) = w.relative(&raw)? else {
            continue;
        };
        if entry.is_dir() || kind == Some(S_IFDIR) {
            w.dir(&rel, &parts)?;
        } else if kind == Some(S_IFLNK) {
            let mut text = String::new();
            (&mut entry)
                .take(4097)
                .read_to_string(&mut text)
                .map_err(|e| refused(format!("the link {raw:?} could not be read: {e}")))?;
            w.link(&rel, parts, text)?;
        } else {
            w.file(&rel, &parts, &mut entry, mode, progress)?;
        }
    }
    w.finish()
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::lock::KeepAll;

    const ALL: Keep = Keep::All(KeepAll::All);

    fn rules<'a>(strip: &'a str, keep: &'a Keep) -> Rules<'a> {
        Rules {
            strip_prefix: strip,
            keep,
            max_entries: 100,
            max_bytes: 10_000,
        }
    }

    /// One archive entry: name, kind and content (the link target for links).
    #[derive(Clone, Copy)]
    enum E<'a> {
        File(&'a str, &'a [u8], u32),
        Dir(&'a str),
        Link(&'a str, &'a str),
        Hard(&'a str, &'a str),
        Fifo(&'a str),
    }

    fn tar_gz(path: &Path, entries: &[E]) {
        let f = std::fs::File::create(path).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        for e in entries {
            let mut h = tar::Header::new_gnu();
            match *e {
                E::File(name, data, mode) => {
                    h.set_entry_type(tar::EntryType::Regular);
                    h.set_size(data.len() as u64);
                    h.set_mode(mode);
                    // `append_data` would refuse `..`; write the name bytes ourselves.
                    set_raw_name(&mut h, name);
                    h.set_cksum();
                    b.append(&h, data).unwrap();
                }
                E::Dir(name) => {
                    h.set_entry_type(tar::EntryType::Directory);
                    h.set_size(0);
                    h.set_mode(0o755);
                    set_raw_name(&mut h, name);
                    h.set_cksum();
                    b.append(&h, &[][..]).unwrap();
                }
                E::Link(name, target) | E::Hard(name, target) => {
                    h.set_entry_type(if matches!(e, E::Link(..)) {
                        tar::EntryType::Symlink
                    } else {
                        tar::EntryType::Link
                    });
                    h.set_size(0);
                    h.set_mode(0o777);
                    set_raw_name(&mut h, name);
                    set_raw_link(&mut h, target);
                    h.set_cksum();
                    b.append(&h, &[][..]).unwrap();
                }
                E::Fifo(name) => {
                    h.set_entry_type(tar::EntryType::Fifo);
                    h.set_size(0);
                    h.set_mode(0o644);
                    set_raw_name(&mut h, name);
                    h.set_cksum();
                    b.append(&h, &[][..]).unwrap();
                }
            }
        }
        b.into_inner().unwrap().finish().unwrap();
    }

    fn set_raw_name(h: &mut tar::Header, name: &str) {
        let field = &mut h.as_old_mut().name;
        field.fill(0);
        field[..name.len()].copy_from_slice(name.as_bytes());
    }

    fn set_raw_link(h: &mut tar::Header, target: &str) {
        let field = &mut h.as_old_mut().linkname;
        field.fill(0);
        field[..target.len()].copy_from_slice(target.as_bytes());
    }

    fn zip_file(path: &Path, entries: &[E]) {
        let f = std::fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        for e in entries {
            match *e {
                E::File(name, data, mode) => {
                    let o = zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated)
                        .unix_permissions(mode);
                    w.start_file(name, o).unwrap();
                    w.write_all(data).unwrap();
                }
                E::Dir(name) => {
                    w.add_directory(name, zip::write::SimpleFileOptions::default())
                        .unwrap();
                }
                E::Link(name, target) => {
                    w.add_symlink(name, target, zip::write::SimpleFileOptions::default())
                        .unwrap();
                }
                E::Hard(..) | E::Fifo(..) => {
                    // A pipe's file type in the Unix mode of the external attributes, in an
                    // entry that says Unix made it: left alone, the writer names the system
                    // the test runs on (MS-DOS on Windows), whose entries carry no Unix type.
                    let o = zip::write::SimpleFileOptions::default()
                        .system(zip::System::Unix)
                        .external_attributes(0o010_644 << 16);
                    let name = match *e {
                        E::Hard(n, _) | E::Fifo(n) => n,
                        _ => unreachable!(),
                    };
                    w.start_file(name, o).unwrap();
                }
            }
        }
        w.finish().unwrap();
    }

    enum Format {
        TarGz,
        Zip,
    }

    fn unpack_with(
        format: &Format,
        entries: &[E],
        rules: &Rules<'_>,
    ) -> (tempfile::TempDir, Result<Unpacked, ArchiveError>) {
        let dir = tempfile::tempdir().unwrap();
        let (name, make): (&str, fn(&Path, &[E])) = match format {
            Format::TarGz => ("a.tar.gz", tar_gz),
            Format::Zip => ("a.zip", zip_file),
        };
        let archive = dir.path().join(name);
        make(&archive, entries);
        let out = dir.path().join("out");
        let r = unpack(&archive, &out, rules, &CancellationToken::new(), &|_| {});
        (dir, r)
    }

    fn both(
        entries: &[E],
        rules: &Rules<'_>,
    ) -> Vec<(tempfile::TempDir, Result<Unpacked, ArchiveError>)> {
        vec![
            unpack_with(&Format::TarGz, entries, rules),
            unpack_with(&Format::Zip, entries, rules),
        ]
    }

    fn assert_refused(entries: &[E], rules: &Rules<'_>, why: &str) {
        for (format, (dir, r)) in ["tar.gz", "zip"].iter().zip(both(entries, rules)) {
            match r {
                Err(ArchiveError::Refused(m)) => {
                    assert!(m.contains(why), "{format}: {m:?} does not say {why:?}")
                }
                other => panic!("{format}: expected a refusal ({why}), got {other:?}"),
            }
            assert!(
                !dir.path().join("escape").exists() && !dir.path().join("x").exists(),
                "{format}: something was written outside"
            );
        }
    }

    #[test]
    fn the_strip_prefix_is_removed_and_structure_kept() {
        for (format, (dir, r)) in ["tar.gz", "zip"].iter().zip(both(
            &[
                E::Dir("package/"),
                E::File("package/scripts/rea.mjs", b"run", 0o755),
                E::File("package/dist/a.js", b"a", 0o644),
                E::Dir("package/empty/"),
            ],
            &rules("package/", &ALL),
        )) {
            let r = r.unwrap_or_else(|e| panic!("{format}: {e}"));
            assert_eq!(r.files, 2, "{format}");
            assert_eq!(r.bytes, 4, "{format}");
            let out = dir.path().join("out");
            assert_eq!(std::fs::read(out.join("scripts/rea.mjs")).unwrap(), b"run");
            assert!(out.join("dist/a.js").is_file());
            assert!(out.join("empty").is_dir());
            assert!(!out.join("package").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_modes_are_kept_and_masked() {
        use std::os::unix::fs::PermissionsExt;
        for (format, (dir, r)) in ["tar.gz", "zip"].iter().zip(both(
            &[
                E::File("p/bin/node", b"elf", 0o4755),
                E::File("p/LICENSE", b"mit", 0o444),
                // Ghidra's zip holds Windows programs at 0o666 and Temurin's Windows zip
                // files at 0o770: nobody but the owner may change what the app runs.
                E::File("p/bin/world", b"w", 0o4777),
                E::File("p/os/win/decompile.exe", b"mz", 0o666),
                E::File("p/lib/group", b"g", 0o770),
            ],
            &rules("p/", &ALL),
        )) {
            r.unwrap_or_else(|e| panic!("{format}: {e}"));
            let mode = |rel: &str| {
                std::fs::metadata(dir.path().join("out").join(rel))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777
            };
            assert_eq!(mode("bin/node"), 0o755, "{format}: set-user-id dropped");
            assert_eq!(mode("LICENSE"), 0o644, "{format}: owner can write");
            assert_eq!(mode("bin/world"), 0o755, "{format}: no write for others");
            assert_eq!(
                mode("os/win/decompile.exe"),
                0o644,
                "{format}: no write for others"
            );
            assert_eq!(mode("lib/group"), 0o750, "{format}: no write for the group");
        }
    }

    #[test]
    fn the_keep_list_keeps_exactly_its_files() {
        let keep = Keep::Files(vec!["bin/node".into(), "LICENSE".into()]);
        for (format, (dir, r)) in ["tar.gz", "zip"].iter().zip(both(
            &[
                E::File("node-v1/bin/node", b"elf", 0o755),
                E::File("node-v1/bin/npm", b"js", 0o755),
                E::File("node-v1/lib/node_modules/npm/x.js", b"js", 0o644),
                E::File("node-v1/LICENSE", b"mit", 0o644),
            ],
            &rules("node-v1/", &keep),
        )) {
            let r = r.unwrap_or_else(|e| panic!("{format}: {e}"));
            assert_eq!(r.files, 2, "{format}");
            let out = dir.path().join("out");
            assert!(out.join("bin/node").is_file() && out.join("LICENSE").is_file());
            assert!(!out.join("bin/npm").exists() && !out.join("lib").exists());
        }
        // A kept file missing from the archive is refused.
        assert_refused(
            &[E::File("node-v1/bin/node", b"elf", 0o755)],
            &rules("node-v1/", &keep),
            "missing from the archive",
        );
    }

    #[test]
    fn unsafe_names_are_refused() {
        for name in [
            "p/../escape",
            "p/./x",
            "p//x",
            "p/a\\b",
            "p/C:x",
            "p/a:stream",
            "p/CON",
            "p/sub/nul.txt",
            "p/Com1.log",
            "p/lpt9",
            "p/trailing.",
            "p/trailing ",
            "p/tab\tname",
        ] {
            assert_refused(
                &[E::File(name, b"x", 0o644)],
                &rules("p/", &ALL),
                "unsafe path",
            );
        }
    }

    #[test]
    fn absolute_entries_and_entries_outside_the_prefix_are_refused() {
        assert_refused(
            &[E::File("/escape", b"x", 0o644)],
            &rules("", &ALL),
            "unsafe path",
        );
        assert_refused(
            &[E::File("other/x", b"x", 0o644)],
            &rules("p/", &ALL),
            "outside",
        );
    }

    /// Names are checked even for entries the keep list leaves out.
    #[test]
    fn a_bad_name_is_refused_even_when_not_kept() {
        let keep = Keep::Files(vec!["bin/node".into()]);
        assert_refused(
            &[
                E::File("p/bin/node", b"x", 0o755),
                E::File("p/../escape", b"x", 0o644),
            ],
            &rules("p/", &keep),
            "unsafe path",
        );
    }

    #[test]
    fn a_second_entry_for_the_same_path_is_refused() {
        // (A zip writer refuses to write the same name twice; tar does not.)
        let (_d, r) = unpack_with(
            &Format::TarGz,
            &[E::File("p/a", b"1", 0o644), E::File("p/a", b"2", 0o644)],
            &rules("p/", &ALL),
        );
        assert!(
            matches!(&r, Err(ArchiveError::Refused(m)) if m.contains("twice")),
            "{r:?}"
        );
        // Differing only in case: one file on Windows and macOS.
        assert_refused(
            &[
                E::File("p/License", b"1", 0o644),
                E::File("p/LICENSE", b"2", 0o644),
            ],
            &rules("p/", &ALL),
            "twice",
        );
    }

    #[test]
    fn the_entry_and_byte_caps_hold() {
        let many: Vec<String> = (0..12).map(|i| format!("p/f{i}")).collect();
        let entries: Vec<E> = many.iter().map(|n| E::File(n, b"x", 0o644)).collect();
        let mut r = rules("p/", &ALL);
        r.max_entries = 10;
        assert_refused(&entries, &r, "more than 10 entries");
        let mut r = rules("p/", &ALL);
        r.max_bytes = 5;
        assert_refused(
            &[E::File("p/big", b"123456", 0o644)],
            &r,
            "more than 5 bytes",
        );
    }

    #[test]
    fn hard_links_devices_and_pipes_are_refused() {
        for entries in [
            vec![E::File("p/a", b"x", 0o644), E::Hard("p/b", "p/a")],
            vec![E::Fifo("p/pipe")],
        ] {
            let (_d, r) = unpack_with(&Format::TarGz, &entries, &rules("p/", &ALL));
            assert!(
                matches!(&r, Err(ArchiveError::Refused(m)) if m.contains("not a file, folder or link")),
                "{r:?}"
            );
        }
        let (_d, r) = unpack_with(&Format::Zip, &[E::Fifo("p/pipe")], &rules("p/", &ALL));
        assert!(
            matches!(&r, Err(ArchiveError::Refused(m)) if m.contains("not a file, folder or link")),
            "{r:?}"
        );
    }

    /// A zip of one empty stored entry `name`, written byte by byte: its central directory
    /// says which system made it (`made_by`: 0 MS-DOS, 3 Unix, 10 Windows NTFS) and carries
    /// `unix_mode` in the high half of the external attributes, as Info-ZIP writes it.
    fn hand_made_zip(path: &Path, name: &str, made_by: u8, unix_mode: u32) {
        let len = u16::try_from(name.len()).unwrap();
        // Version needed (2.0), flags, stored, time, date (1980-01-01), then the CRC-32 and
        // both sizes of no bytes.
        let common = |out: &mut Vec<u8>| {
            for v in [20u16, 0, 0, 0, 0x21] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&[0; 12]);
        };
        let mut local = 0x0403_4b50u32.to_le_bytes().to_vec();
        common(&mut local);
        for v in [len, 0] {
            local.extend_from_slice(&v.to_le_bytes());
        }
        local.extend_from_slice(name.as_bytes());

        let mut central = 0x0201_4b50u32.to_le_bytes().to_vec();
        // Version made by: 2.0, and the system that made the entry.
        central.extend_from_slice(&[20, made_by]);
        common(&mut central);
        // Name, extra and comment lengths, first disk, internal attributes.
        for v in [len, 0, 0, 0, 0] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        central.extend_from_slice(&(unix_mode << 16).to_le_bytes());
        // The local header is at the start.
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(name.as_bytes());

        let mut end = 0x0605_4b50u32.to_le_bytes().to_vec();
        for v in [0u16, 0, 1, 1] {
            end.extend_from_slice(&v.to_le_bytes());
        }
        for v in [central.len(), local.len()] {
            end.extend_from_slice(&u32::try_from(v).unwrap().to_le_bytes());
        }
        end.extend_from_slice(&0u16.to_le_bytes());
        std::fs::write(path, [local, central, end].concat()).unwrap();
    }

    /// A zip entry's type comes from the system its archive says made it, never from the
    /// system unpacking it: the same bytes give the same answer on Linux, macOS and Windows.
    #[test]
    fn a_zip_entry_is_judged_by_the_system_that_made_it_on_every_system() {
        const DOS: u8 = 0;
        const UNIX: u8 = 3;
        const NTFS: u8 = 10;
        let unpack_one = |made_by: u8, mode: u32| {
            let dir = tempfile::tempdir().unwrap();
            let archive = dir.path().join("a.zip");
            hand_made_zip(&archive, "p/entry", made_by, mode);
            let out = dir.path().join("out");
            let r = unpack(
                &archive,
                &out,
                &rules("p/", &ALL),
                &CancellationToken::new(),
                &|_| {},
            );
            (r, out.join("entry").is_file())
        };
        // Made on Unix: a pipe, a character device, a block device or a socket is refused
        // here as on any other system; so is a pipe from a maker that keeps Unix modes too.
        for (made_by, mode) in [
            (UNIX, 0o010_644),
            (UNIX, 0o020_644),
            (UNIX, 0o060_644),
            (UNIX, 0o140_755),
            (NTFS, 0o010_644),
        ] {
            let (r, written) = unpack_one(made_by, mode);
            assert!(
                matches!(&r, Err(ArchiveError::Refused(m)) if m.contains("not a file, folder or link")),
                "made by {made_by}, mode {mode:o}: {r:?}"
            );
            assert!(!written, "made by {made_by}, mode {mode:o}");
        }
        // A plain file made on Unix unpacks (the hand-made archive is sound).
        let one_empty_file = Unpacked {
            files: 1,
            links: 0,
            bytes: 0,
        };
        let (r, written) = unpack_one(UNIX, 0o100_644);
        assert_eq!(r.unwrap(), one_empty_file);
        assert!(written);
        // MS-DOS has no Unix types: whatever its high bits say, the entry is a plain file, and
        // nothing but a plain file is written.
        let (r, written) = unpack_one(DOS, 0o010_644);
        assert_eq!(r.unwrap(), one_empty_file);
        assert!(written);
    }

    #[test]
    fn links_that_leave_are_refused() {
        for target in ["../../escape", "/etc/passwd", "a\\b", "C:/x"] {
            assert_refused(
                &[E::File("p/a", b"x", 0o644), E::Link("p/l", target)],
                &rules("p/", &ALL),
                "points outside",
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn links_inside_are_kept_and_checked_on_disk() {
        for (format, (dir, r)) in ["tar.gz", "zip"].iter().zip(both(
            &[
                E::File("jdk/legal/java.base/LICENSE", b"gpl", 0o644),
                E::Link("jdk/legal/java.xml/LICENSE", "../java.base/LICENSE"),
            ],
            &rules("jdk/", &ALL),
        )) {
            let r = r.unwrap_or_else(|e| panic!("{format}: {e}"));
            assert_eq!((r.files, r.links), (1, 1), "{format}");
            let link = dir.path().join("out/legal/java.xml/LICENSE");
            assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
            assert_eq!(std::fs::read(&link).unwrap(), b"gpl");
        }
        // A link to nothing fails the check on disk.
        assert_refused(
            &[E::Link("p/l", "missing")],
            &rules("p/", &ALL),
            "at nothing",
        );
        // A link that leaves through another link fails it too.
        assert_refused(
            &[
                E::Dir("p/d/"),
                E::Link("p/up", ".."),
                E::Link("p/d/l", "../up/x"),
            ],
            &rules("p/", &ALL),
            "points outside",
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_never_created_through_another_link() {
        let (dir, r) = unpack_with(
            &Format::TarGz,
            &[
                E::Dir("p/real/"),
                E::Link("p/a", "real"),
                E::Link("p/a/b", "x"),
            ],
            &rules("p/", &ALL),
        );
        assert!(
            matches!(&r, Err(ArchiveError::Refused(m)) if m.contains("through another link")),
            "{r:?}"
        );
        assert!(!dir.path().join("out/real/b").exists());
    }

    #[cfg(windows)]
    #[test]
    fn links_are_refused_on_windows() {
        let (_d, r) = unpack_with(
            &Format::Zip,
            &[E::File("p/a", b"x", 0o644), E::Link("p/l", "a")],
            &rules("p/", &ALL),
        );
        assert!(matches!(r, Err(ArchiveError::Refused(_))), "{r:?}");
    }

    #[test]
    fn cancel_stops_the_unpack() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.tar.gz");
        tar_gz(&archive, &[E::File("p/a", b"x", 0o644)]);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let r = unpack(
            &archive,
            &dir.path().join("out"),
            &rules("p/", &ALL),
            &cancel,
            &|_| {},
        );
        assert!(matches!(r, Err(ArchiveError::Cancelled)), "{r:?}");
    }

    #[test]
    fn a_part_file_is_recognised_by_its_asset_name() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir
            .path()
            .join("kuna-v1.990-linux-x86_64.tar.gz.4860603084da39dc.part");
        tar_gz(&archive, &[E::File("k/kuna", b"elf", 0o755)]);
        let r = unpack(
            &archive,
            &dir.path().join("out"),
            &rules("k/", &ALL),
            &CancellationToken::new(),
            &|_| {},
        )
        .unwrap();
        assert_eq!(r.files, 1);
    }

    #[test]
    fn device_names() {
        for n in [
            "CON", "con.txt", "Nul", "COM1", "lpt9.dat", "AUX", "PRN.x", "CONIN$",
        ] {
            assert!(is_device_name(n), "{n}");
        }
        for n in [
            "console",
            "com",
            "com10",
            "nuls",
            "lpt",
            "auxiliary",
            "CONTRIBUTING.md",
        ] {
            assert!(!is_device_name(n), "{n}");
        }
    }
}
