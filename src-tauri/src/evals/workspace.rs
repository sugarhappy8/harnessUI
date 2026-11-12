//! The folder a headless run works in.
//!
//! Made fresh for every run under the system's temporary folder, with a random name, and
//! removed when the [`Workspace`] is dropped: when the run finishes, fails, is cancelled
//! or its task is dropped. A run can never be pointed at a user folder: there is no way
//! to make a workspace from a path. The app's data folder is a forbidden folder for the
//! tools (ARCHITECTURE.md 13.4), so the temporary folder is the one place both the app
//! and the tools may use.

use std::io;
use std::path::{Component, Path, PathBuf};

/// The start of every workspace's folder name.
pub const PREFIX: &str = "harness-run-";

/// A run's folder. Removed on drop.
#[derive(Debug)]
pub struct Workspace {
    path: PathBuf,
}

impl Workspace {
    /// Makes a new, empty folder under the system's temporary folder.
    pub fn create() -> io::Result<Self> {
        Self::create_in(&std::env::temp_dir(), PREFIX)
    }

    /// Makes a new, empty folder named `<prefix><random>` in `parent`.
    fn create_in(parent: &Path, prefix: &str) -> io::Result<Self> {
        std::fs::create_dir_all(parent)?;
        let path = parent.join(format!("{prefix}{}", uuid::Uuid::new_v4().simple()));
        // `create_dir`, not `create_dir_all`: a folder that already exists is an error.
        std::fs::create_dir(&path)?;
        let path = dunce::canonicalize(&path)?;
        Ok(Self { path })
    }

    /// A second folder next to the workspaces, for what the run's model must not see
    /// (verification files). Removed on drop like a workspace.
    pub fn create_private() -> io::Result<Self> {
        Self::create_in(&std::env::temp_dir(), "harness-verify-")
    }

    /// The folder's real path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The folder as the tools and the policy engine see it.
    pub fn display(&self) -> String {
        self.path.display().to_string()
    }

    /// Writes `files` (relative path, content) into the folder. Each path must be a plain
    /// relative path that stays inside: shipped data only, checked anyway.
    pub fn write_files<'a>(
        &self,
        files: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    ) -> io::Result<()> {
        for (relative, content) in files {
            let target = self.inside(relative)?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&target, content)?;
        }
        Ok(())
    }

    /// The text of a file of the folder, if it exists and is text.
    pub fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.inside(relative).ok()?).ok()
    }

    /// Every file of the folder as a relative path with `/`, sorted. Links are not
    /// followed.
    pub fn files(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![self.path.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                if kind.is_dir() {
                    stack.push(path);
                } else if kind.is_file()
                    && let Ok(relative) = path.strip_prefix(&self.path)
                {
                    let parts: Vec<String> = relative
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect();
                    out.push(parts.join("/"));
                }
            }
        }
        out.sort();
        out
    }

    /// `relative` joined to the folder, refused unless it is a plain relative path.
    pub fn inside(&self, relative: &str) -> io::Result<PathBuf> {
        let path = Path::new(relative);
        let plain = !relative.is_empty()
            && path
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        if !plain {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{relative} is not a path inside the run's folder"),
            ));
        }
        Ok(self.path.join(path))
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if let Err(e) = remove_all(&self.path) {
            tracing::warn!(path = %self.path.display(), "could not remove a run's folder: {e}");
        }
    }
}

/// Removes a folder and everything in it, read-only files included (Windows refuses to
/// delete those otherwise). Links are removed, never followed.
fn remove_all(path: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => {
            make_writable(path);
            match std::fs::remove_dir_all(path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        }
    }
}

/// Makes a folder tree removable. On Unix only folders matter (deleting a file needs
/// write access to its folder, not to the file), so every folder gets the owner-only mode
/// 0o700 and files are left alone: nothing becomes writable for others, a folder set to
/// mode 000 can be listed again, and a hard link to a file elsewhere keeps its mode. On
/// Windows a read-only file cannot be deleted, so files lose that flag. Links are never
/// followed.
fn make_writable(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            make_writable(&path);
            continue;
        }
        #[cfg(windows)]
        if let Ok(meta) = entry.metadata() {
            let mut perms = meta.permissions();
            if perms.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                let _ = std::fs::set_permissions(&path, perms);
            }
        }
        #[cfg(not(windows))]
        let _ = path;
    }
}

/// Left-over folders of runs older than this are removed when a run starts.
pub const LEFTOVER_AGE: std::time::Duration = std::time::Duration::from_secs(3600);

/// Removes the folders of earlier runs that ended with the app (a crash, a forced quit):
/// only real folders, not links, directly in the system's temporary folder, named exactly
/// `harness-run-<32 hex digits>` or `harness-verify-<32 hex digits>`, and older than
/// [`LEFTOVER_AGE`] (so a run in progress elsewhere, such as a test, is never touched).
/// Returns how many were removed.
pub fn remove_leftovers() -> usize {
    remove_leftovers_in(&std::env::temp_dir(), LEFTOVER_AGE)
}

fn ours(name: &str) -> bool {
    [PREFIX, "harness-verify-"].iter().any(|prefix| {
        name.strip_prefix(prefix).is_some_and(|rest| {
            rest.len() == 32
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    })
}

fn remove_leftovers_in(parent: &Path, age: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if !ours(&entry.file_name().to_string_lossy()) {
            continue;
        }
        // `symlink_metadata`: a link with our name is not followed and not removed.
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|elapsed| elapsed >= age);
        if meta.is_dir() && old && remove_all(&entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_is_fresh_under_the_temporary_folder_and_removed_on_drop() {
        let a = Workspace::create().unwrap();
        let b = Workspace::create().unwrap();
        assert_ne!(a.path(), b.path());
        let temp = dunce::canonicalize(std::env::temp_dir()).unwrap();
        assert!(a.path().starts_with(&temp), "{:?}", a.path());
        let name = a.path().file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with(PREFIX) && name.len() > PREFIX.len() + 20,
            "{name}"
        );
        assert_eq!(std::fs::read_dir(a.path()).unwrap().count(), 0);
        a.write_files([("src/deep/x.py", b"x = 1\n".as_slice()), ("r.md", b"# r\n")])
            .unwrap();
        assert_eq!(
            a.files(),
            vec!["r.md".to_owned(), "src/deep/x.py".to_owned()]
        );
        assert_eq!(a.read("src/deep/x.py").as_deref(), Some("x = 1\n"));
        let path = a.path().to_path_buf();
        drop(a);
        assert!(!path.exists(), "the run's folder was left behind");
        drop(b);
    }

    #[test]
    fn read_only_files_do_not_keep_the_folder_alive() {
        let ws = Workspace::create().unwrap();
        ws.write_files([("locked.txt", b"x".as_slice())]).unwrap();
        let file = ws.path().join("locked.txt");
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&file, perms).unwrap();
        let path = ws.path().to_path_buf();
        drop(ws);
        assert!(!path.exists());
    }

    #[test]
    fn a_workspace_is_removed_when_the_run_panics() {
        let mut kept = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let ws = Workspace::create().unwrap();
            kept = Some(ws.path().to_path_buf());
            panic!("the run failed");
        }));
        assert!(result.is_err());
        assert!(!kept.unwrap().exists());
    }

    #[test]
    fn shipped_paths_must_stay_inside() {
        let ws = Workspace::create().unwrap();
        for bad in ["", "../x", "a/../../x", "/etc/passwd", "a/../b"] {
            assert!(ws.inside(bad).is_err(), "{bad}");
            assert!(ws.write_files([(bad, b"x".as_slice())]).is_err(), "{bad}");
        }
        assert!(ws.inside("a/b.txt").is_ok());
    }

    /// A folder the model set to mode 000 is still removed, and clean-up makes nothing
    /// writable for others. (Run as root, the mode alone does not stop the removal; the
    /// modes left on files are checked either way.)
    #[cfg(unix)]
    #[test]
    fn a_locked_folder_is_removed_and_nothing_becomes_world_writable() {
        use std::os::unix::fs::PermissionsExt;
        let outside = Workspace::create_private().unwrap();
        outside
            .write_files([("kept.txt", b"x".as_slice())])
            .unwrap();
        let kept = outside.path().join("kept.txt");
        std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o644)).unwrap();
        let ws = Workspace::create().unwrap();
        ws.write_files([("locked/inner/f.txt", b"x".as_slice())])
            .unwrap();
        // A hard link to a file outside: clean-up must not change that file's mode.
        std::fs::hard_link(&kept, ws.path().join("locked/hard.txt")).unwrap();
        std::fs::set_permissions(
            ws.path().join("locked/inner"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        std::fs::set_permissions(
            ws.path().join("locked"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        let path = ws.path().to_path_buf();
        make_writable(&path);
        let mode = std::fs::metadata(path.join("locked"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
        drop(ws);
        assert!(!path.exists(), "the locked folder was left behind");
        let mode = std::fs::metadata(&kept).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "the outside file's mode changed");
    }

    #[test]
    fn only_old_folders_with_exactly_our_names_are_left_overs() {
        let parent = Workspace::create_private().unwrap();
        let hex = "0123456789abcdef0123456789abcdef";
        let mine = [
            format!("harness-run-{hex}"),
            format!("harness-verify-{hex}"),
        ];
        let others = [
            format!("harness-run-{hex}x"),
            format!("harness-run-{}", &hex[..31]),
            // Capitals, and other digits than `hex`: on macOS and Windows a name that
            // differs only in case would be the same folder as one of ours.
            "harness-run-FEDCBA9876543210FEDCBA9876543210".to_owned(),
            format!("other-{hex}"),
        ];
        for name in mine.iter().chain(&others) {
            std::fs::create_dir(parent.path().join(name)).unwrap();
            std::fs::write(parent.path().join(name).join("f"), "x").unwrap();
        }
        // A file with our name is not a folder of ours.
        std::fs::write(
            parent
                .path()
                .join(format!("harness-run-{}", "f".repeat(32))),
            "x",
        )
        .unwrap();
        // A link with our name, pointing at a folder that must survive.
        #[cfg(unix)]
        {
            let target = Workspace::create_private().unwrap();
            target
                .write_files([("keep.txt", b"keep".as_slice())])
                .unwrap();
            std::os::unix::fs::symlink(
                target.path(),
                parent
                    .path()
                    .join(format!("harness-verify-{}", "a".repeat(32))),
            )
            .unwrap();
            // Too new: nothing is removed.
            assert_eq!(remove_leftovers_in(parent.path(), LEFTOVER_AGE), 0);
            assert_eq!(
                remove_leftovers_in(parent.path(), std::time::Duration::ZERO),
                2
            );
            assert_eq!(target.read("keep.txt").as_deref(), Some("keep"));
        }
        #[cfg(not(unix))]
        assert_eq!(
            remove_leftovers_in(parent.path(), std::time::Duration::ZERO),
            2
        );
        for name in &mine {
            assert!(!parent.path().join(name).exists(), "{name}");
        }
        for name in &others {
            assert!(parent.path().join(name).exists(), "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn removing_a_workspace_never_follows_a_link_out_of_it() {
        let outside = Workspace::create_private().unwrap();
        outside
            .write_files([("keep.txt", b"keep".as_slice())])
            .unwrap();
        let ws = Workspace::create().unwrap();
        std::os::unix::fs::symlink(outside.path(), ws.path().join("link")).unwrap();
        drop(ws);
        assert_eq!(outside.read("keep.txt").as_deref(), Some("keep"));
    }
}
