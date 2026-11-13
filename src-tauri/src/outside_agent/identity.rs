//! Who a program file is, to notice when it was replaced after the user turned Coding apps
//! on. Kept apart from the rest of the module so that the Windows code can be compile-checked
//! for a Windows target on its own (it needs only `std`, `serde` and the `windows` crate).
//!
//! - Unix: the device and inode, the size and the modified time.
//! - Windows: the volume serial number and the file index (`GetFileInformationByHandle`,
//!   the same pair the system uses to tell files apart), the size and the modified time.
//!   Replacing the file, also with a copy that has the same size and times, gives it a new
//!   file index.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Who a file is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileIdentity {
    /// The device (Unix) or the volume serial number (Windows).
    pub device: u64,
    /// The inode (Unix) or the file index (Windows).
    pub file: u64,
    pub size: u64,
    /// Modified time, in nanoseconds since 1970.
    pub modified_ns: u64,
}

impl FileIdentity {
    /// The identity of the file at `path` (links followed). Only regular files have one.
    pub fn of(path: &Path) -> std::io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        if !meta.is_file() {
            return Err(std::io::Error::other("not a file"));
        }
        let modified_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                device: meta.dev(),
                file: meta.ino(),
                size: meta.len(),
                modified_ns,
            })
        }
        #[cfg(windows)]
        {
            let (device, file) = windows_file_id(path)?;
            Ok(Self {
                device,
                file,
                size: meta.len(),
                modified_ns,
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            Ok(Self {
                device: 0,
                file: 0,
                size: meta.len(),
                modified_ns,
            })
        }
    }
}

/// The volume serial number and the 64-bit file index of the file at `path`, from
/// `GetFileInformationByHandle` on a handle opened only to read its attributes (stable Rust
/// has no portable way to read them: `MetadataExt::volume_serial_number` and `file_index`
/// are still unstable).
#[cfg(windows)]
fn windows_file_id(path: &Path) -> std::io::Result<(u64, u64)> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_READ_ATTRIBUTES, GetFileInformationByHandle,
    };

    // Reading the attributes needs no read access to the contents, and the default share
    // mode lets the program run, be updated or be deleted meanwhile.
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0)
        .open(path)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `file` stays open for the whole call, so its handle is valid, and `info` is
    // a properly sized, writable `BY_HANDLE_FILE_INFORMATION`.
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Ok((u64::from(info.dwVolumeSerialNumber), index))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same file keeps its identity; a file put in its place (also with the same size)
    /// has another one.
    #[test]
    fn a_replaced_file_has_another_identity() {
        let dir = std::env::temp_dir().join(format!("identity-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("program");
        std::fs::write(&path, b"one").unwrap();
        let first = FileIdentity::of(&path).unwrap();
        assert_eq!(FileIdentity::of(&path).unwrap(), first);
        assert_eq!(first.size, 3);
        // Keep the old file alive under another name so its inode or file index cannot be
        // reused for the new one.
        std::fs::rename(&path, dir.join("old")).unwrap();
        std::fs::write(&path, b"two").unwrap();
        let second = FileIdentity::of(&path).unwrap();
        assert_eq!(second.size, first.size);
        assert_ne!((second.device, second.file), (first.device, first.file));
        assert!(FileIdentity::of(&dir).is_err(), "a folder has no identity");
        assert!(FileIdentity::of(&dir.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
