//! Walking a folder a reverse tool names (item5-spec.md WP9c `walk.rs`, section 6): at most
//! 50,000 entries and 32 levels; each link read with `read_link` and refused without being
//! resolved when it names a network, UNC, device-namespace or (on a Mac) `/net/…` place;
//! other links resolved and refused outside the approved folders (not in Full access) or
//! inside a forbidden folder (every mode); devices, pipes and sockets refused.
//!
//! TODO(0.0.6): WP9c builds [`walk_folder`] and [`RealWalkFs`]. This is WP9a's contract
//! stub: the trait, types and signatures are frozen.

use policy::path::{Fs, PathSandbox};

/// Entries walked at most before a folder is refused as too large.
pub const MAX_ENTRIES: usize = 50_000;
/// Levels walked at most.
pub const MAX_DEPTH: usize = 32;

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
    /// A link's own text, never resolved; `None` when `path` is not a link.
    fn read_link(&self, path: &str) -> Result<Option<String>, String>;
    /// What `path` is, without following a link; `None` when it does not exist.
    fn kind(&self, path: &str) -> Option<EntryKind>;
}

/// Why a folder was refused (each maps to a message of section 6).
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
}

/// The real file system.
///
/// TODO(0.0.6): WP9c. The stub lists nothing and reports no links.
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
    fn read_dir(&self, _path: &str) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
    fn read_link(&self, _path: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn kind(&self, _path: &str) -> Option<EntryKind> {
        None
    }
}

/// Walks `folder` (a canonical path that passed the sandbox).
///
/// TODO(0.0.6): WP9c. The stub accepts every folder; nothing calls it yet.
pub fn walk_folder(
    fs: &dyn WalkFs,
    folder: &str,
    sandbox: &PathSandbox,
    full_access: bool,
) -> Result<(), WalkRefusal> {
    let _ = (fs, folder, sandbox, full_access);
    Ok(())
}
