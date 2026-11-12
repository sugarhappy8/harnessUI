//! The identity of a file the owner pointed rea at (item5-spec.md WP9a `identity.rs`): the
//! device and inode on macOS and Linux, the volume serial and file index on Windows, plus
//! size and modified time. A session call checks it again immediately before forwarding,
//! so a file swapped after the card was shown is refused (WP9g). Folders have no identity:
//! a folder is walked again in `call` instead.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Who a file is, as of one look at it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileIdentity {
    /// Device (Unix) or volume serial number (Windows).
    pub device: u64,
    /// Inode (Unix) or file index (Windows).
    pub file: u64,
    /// Size in bytes.
    pub size: u64,
    /// Last modification, in nanoseconds since 1970 (Unix) or 100 ns units since 1601
    /// (Windows); only compared, never shown.
    pub modified: i128,
}

impl FileIdentity {
    /// The identity of the regular file at `path` (links followed). Folders, devices,
    /// pipes and sockets are refused with `InvalidInput`; opening never blocks on a pipe.
    pub fn of(path: &Path) -> io::Result<Self> {
        imp::of(path)
    }
}

fn not_a_file() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "only regular files have an identity",
    )
}

#[cfg(unix)]
mod imp {
    use std::io;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Path;

    use super::{FileIdentity, not_a_file};

    pub fn of(path: &Path) -> io::Result<FileIdentity> {
        if !std::fs::metadata(path)?.is_file() {
            return Err(not_a_file());
        }
        // O_NONBLOCK: if the path became a pipe since the look above, opening it must not
        // wait for a writer.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        let m = file.metadata()?;
        if !m.is_file() {
            return Err(not_a_file());
        }
        Ok(FileIdentity {
            device: m.dev(),
            file: m.ino(),
            size: m.len(),
            modified: i128::from(m.mtime()) * 1_000_000_000 + i128::from(m.mtime_nsec()),
        })
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, GetFileInformationByHandle,
    };

    use super::{FileIdentity, not_a_file};

    pub fn of(path: &Path) -> io::Result<FileIdentity> {
        if !std::fs::metadata(path)?.is_file() {
            return Err(not_a_file());
        }
        let file = std::fs::File::open(path)?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the handle belongs to `file`, which outlives the call; `info` is a valid,
        // writable structure of the type the function fills.
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
            .map_err(|e| io::Error::other(e.to_string()))?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0 {
            return Err(not_a_file());
        }
        let modified = (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32)
            | u64::from(info.ftLastWriteTime.dwLowDateTime);
        Ok(FileIdentity {
            device: u64::from(info.dwVolumeSerialNumber),
            file: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            size: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
            modified: i128::from(modified),
        })
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::io;
    use std::path::Path;

    use super::FileIdentity;

    pub fn of(_path: &Path) -> io::Result<FileIdentity> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "file identity is not available on this system",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_file_same_identity_and_a_replacement_differs() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("hello.exe");
        std::fs::write(&p, b"MZ first").unwrap();
        let a = FileIdentity::of(&p).unwrap();
        assert_eq!(a, FileIdentity::of(&p).unwrap());
        assert_eq!(a.size, 8);
        // A different file moved into its place.
        let q = dir.path().join("other.exe");
        std::fs::write(&q, b"MZ second!").unwrap();
        std::fs::rename(&q, &p).unwrap();
        let b = FileIdentity::of(&p).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn folders_have_no_identity() {
        let dir = tempfile::tempdir().unwrap();
        let e = FileIdentity::of(dir.path()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        assert!(FileIdentity::of(&dir.path().join("missing")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_is_refused_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: `c` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let e = FileIdentity::of(&fifo).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_followed_to_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("real");
        std::fs::write(&p, b"x").unwrap();
        let l = dir.path().join("link");
        std::os::unix::fs::symlink(&p, &l).unwrap();
        assert_eq!(FileIdentity::of(&l).unwrap(), FileIdentity::of(&p).unwrap());
    }
}
