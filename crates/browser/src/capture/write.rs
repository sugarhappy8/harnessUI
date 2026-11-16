//! Writing a capture to disk (ARCHITECTURE.md 9.3, 13.4).
//!
//! The capture gets a folder of its own, created new inside the approved folder the card
//! named (a name already there gets `-2`, `-3`…; nothing is ever overwritten). Every file
//! is created new (`create_new`, which also refuses to follow a link left at its name),
//! under a name from [`crate::capture::names`], in one of four sub-folders the app names.
//! Before each write the capture folder must still be the real folder that was created,
//! not a link put in its place.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::capture::names::NameBook;
use crate::capture::report::FileLine;

/// The sub-folders a capture may write into.
pub const SUBFOLDERS: &[&str] = &["styles", "assets", "scripts", "screenshots"];

/// One capture folder being written.
#[derive(Debug)]
pub struct Writer {
    root: PathBuf,
    /// The folder as the system resolves it, recorded when it was created.
    canonical: PathBuf,
    books: std::collections::HashMap<String, NameBook>,
    made: Vec<String>,
    pub files: Vec<FileLine>,
}

impl Writer {
    /// Creates `parent/name` (or `name-2`, `name-3`…) as a new folder. `parent` is the
    /// approved folder the card named; it is created first when it does not exist yet.
    pub fn create(parent: &Path, name: &str) -> Result<Self, String> {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "could not create the folder {}: {}",
                parent.display(),
                plain_io(&e)
            )
        })?;
        for n in 1..=99u32 {
            let candidate = if n == 1 {
                parent.join(name)
            } else {
                parent.join(format!("{name}-{n}"))
            };
            match std::fs::create_dir(&candidate) {
                Ok(()) => {
                    let canonical = dunce::canonicalize(&candidate).map_err(|e| {
                        format!(
                            "could not read the new folder {}: {}",
                            candidate.display(),
                            plain_io(&e)
                        )
                    })?;
                    let mut books = std::collections::HashMap::new();
                    books.insert(
                        String::new(),
                        NameBook::with_taken(&["index.html", "report.md", "animations.json"]),
                    );
                    for s in SUBFOLDERS {
                        books.insert((*s).to_string(), NameBook::default());
                    }
                    return Ok(Self {
                        root: candidate,
                        canonical,
                        books,
                        made: Vec::new(),
                        files: Vec::new(),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(format!(
                        "could not create the folder {}: {}",
                        candidate.display(),
                        plain_io(&e)
                    ));
                }
            }
        }
        Err(format!(
            "could not find a free folder name for {name} in {}",
            parent.display()
        ))
    }

    /// The capture folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The capture folder as resolved when it was created.
    pub fn canonical(&self) -> &Path {
        &self.canonical
    }

    /// A free name in `sub` (`""` for the capture folder itself).
    pub fn name(&mut self, sub: &str, stem: &str, ext: &str) -> String {
        self.books.entry(sub.to_string()).or_default().take(stem, ext)
    }

    /// Writes a new file `sub/name` and returns its path inside the capture (with `/`).
    /// `name` must come from [`Self::name`] or be one of the fixed names.
    pub fn write(&mut self, sub: &str, name: &str, bytes: &[u8]) -> Result<String, String> {
        if !(sub.is_empty() || SUBFOLDERS.contains(&sub)) || !plain_name(name) {
            return Err(format!("internal error: refused the name {sub}/{name}"));
        }
        self.still_ours()?;
        let dir = if sub.is_empty() {
            self.root.clone()
        } else {
            let d = self.root.join(sub);
            if !self.made.iter().any(|m| m == sub) {
                match std::fs::create_dir(&d) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        let meta = std::fs::symlink_metadata(&d)
                            .map_err(|e| format!("could not check {}: {}", d.display(), plain_io(&e)))?;
                        if !meta.file_type().is_dir() {
                            return Err(format!("{} is not a folder any more", d.display()));
                        }
                    }
                    Err(e) => {
                        return Err(format!("could not create {}: {}", d.display(), plain_io(&e)));
                    }
                }
                self.made.push(sub.to_string());
            }
            d
        };
        let path = dir.join(name);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| format!("could not save {}: {}", path.display(), plain_io(&e)))?;
        f.write_all(bytes)
            .and_then(|()| f.flush())
            .map_err(|e| format!("could not save {}: {}", path.display(), plain_io(&e)))?;
        let rel = if sub.is_empty() {
            name.to_string()
        } else {
            format!("{sub}/{name}")
        };
        self.files.push(FileLine {
            path: rel.clone(),
            bytes: bytes.len() as u64,
        });
        Ok(rel)
    }

    /// Refuses when the capture folder was replaced (by a link, or moved) since it was made.
    fn still_ours(&self) -> Result<(), String> {
        let meta = std::fs::symlink_metadata(&self.root).map_err(|e| {
            format!(
                "the folder {} is gone: {}",
                self.root.display(),
                plain_io(&e)
            )
        })?;
        let now = dunce::canonicalize(&self.root).ok();
        if !meta.file_type().is_dir() || now.as_deref() != Some(self.canonical.as_path()) {
            return Err(format!(
                "the folder {} was changed while the copy was being saved, so nothing more was written",
                self.root.display()
            ));
        }
        Ok(())
    }
}

/// A name the writer accepts: one plain component.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= crate::capture::names::MAX_NAME
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
}

fn plain_io(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        std::io::ErrorKind::NotFound => "it does not exist".into(),
        std::io::ErrorKind::AlreadyExists => "a file with that name is already there".into(),
        _ => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_folder_is_new_and_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let a = Writer::create(tmp.path(), "site-2026").unwrap();
        let b = Writer::create(tmp.path(), "site-2026").unwrap();
        assert_ne!(a.root(), b.root());
        assert!(b.root().ends_with("site-2026-2"));
        // A missing approved folder is created first.
        let deeper = tmp.path().join("new").join("place");
        let c = Writer::create(&deeper, "x").unwrap();
        assert!(c.root().starts_with(&deeper));
    }

    #[test]
    fn files_get_plain_names_in_the_app_s_folders_only() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = Writer::create(tmp.path(), "cap").unwrap();
        let n = w.name("assets", "logo", "png");
        assert_eq!(w.write("assets", &n, b"png").unwrap(), "assets/logo.png");
        let again = w.name("assets", "LOGO", "png");
        assert_eq!(again, "logo-2.png");
        assert!(w.write("assets", "logo.png", b"x").is_err(), "an existing file is never replaced");
        assert!(w.write("../outside", "x.png", b"x").is_err());
        assert!(w.write("assets", "../x.png", b"x").is_err());
        assert!(w.write("assets", "a/b.png", b"x").is_err());
        assert!(w.write("assets", "C:x.png", b"x").is_err());
        assert!(w.write("", "index.html", b"<p>").is_ok());
        assert_eq!(w.files.len(), 2);
        assert!(!tmp.path().join("x.png").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_swapped_for_a_link_stops_the_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let mut w = Writer::create(tmp.path(), "cap").unwrap();
        w.write("", "index.html", b"a").unwrap();
        let root = w.root().to_path_buf();
        std::fs::remove_file(root.join("index.html")).unwrap();
        std::fs::remove_dir(&root).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), &root).unwrap();
        let err = w.write("", "report.md", b"b").unwrap_err();
        assert!(err.contains("was changed"), "{err}");
        assert!(!elsewhere.path().join("report.md").exists());
        // A link left where a file will go is not followed either.
        let tmp2 = tempfile::tempdir().unwrap();
        let mut w2 = Writer::create(tmp2.path(), "cap").unwrap();
        let target = elsewhere.path().join("victim.txt");
        std::os::unix::fs::symlink(&target, w2.root().join("report.md")).unwrap();
        assert!(w2.write("", "report.md", b"x").is_err());
        assert!(!target.exists());
    }
}
