//! Free disk space before a download (item5-spec.md WP9a `install.rs`: download plus
//! unpacked size plus 10 %, refused before anything is fetched) and, on Windows, the file
//! system of the reverse folder (rea's Windows support reads its runtimes only from local
//! NTFS or ReFS drives).

use std::path::Path;
use std::sync::Arc;

/// Free bytes for the user at a path; injectable so tests can fill the disk.
pub type FreeSpace = Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>;

/// The real [`FreeSpace`].
pub fn system() -> FreeSpace {
    Arc::new(free_bytes)
}

/// Free bytes available to this user on the volume holding `path` (or its nearest existing
/// ancestor). `None` when the system does not say.
pub fn free_bytes(path: &Path) -> Option<u64> {
    let existing = path.ancestors().find(|p| p.exists())?;
    imp::free_bytes(existing)
}

/// The file system name of the volume holding `path` (`NTFS`, `ReFS`, `FAT32`…), on
/// Windows only; `None` elsewhere or when unknown.
pub fn filesystem_name(path: &Path) -> Option<String> {
    let existing = path.ancestors().find(|p| p.exists())?;
    imp::filesystem_name(existing)
}

/// Whether a volume with this file system can hold rea's Windows runtimes: NTFS or ReFS, or
/// unknown (not refused on a guess).
pub fn local_ntfs_or_refs(name: Option<&str>) -> bool {
    name.is_none_or(|n| n.eq_ignore_ascii_case("NTFS") || n.eq_ignore_ascii_case("ReFS"))
}

#[cfg(unix)]
mod imp {
    use std::path::Path;

    pub fn free_bytes(path: &Path) -> Option<u64> {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: an all-zero statvfs is a valid value of this plain C structure.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c` is a valid NUL-terminated path and `st` a writable statvfs.
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        #[allow(clippy::useless_conversion)] // the field types differ between systems
        Some(u64::from(st.f_bavail).saturating_mul(u64::from(st.f_frsize)))
    }

    pub fn filesystem_name(_path: &Path) -> Option<String> {
        None
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetVolumeInformationW, GetVolumePathNameW,
    };
    use windows::core::PCWSTR;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    pub fn free_bytes(path: &Path) -> Option<u64> {
        let w = wide(path);
        let mut free = 0u64;
        // SAFETY: `w` is NUL-terminated and outlives the call; `free` is writable.
        unsafe { GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut free), None, None) }.ok()?;
        Some(free)
    }

    pub fn filesystem_name(path: &Path) -> Option<String> {
        let w = wide(path);
        let mut root = [0u16; 1024];
        // SAFETY: `w` is NUL-terminated; `root` is a writable buffer of the given length.
        unsafe { GetVolumePathNameW(PCWSTR(w.as_ptr()), &mut root) }.ok()?;
        let mut name = [0u16; 64];
        // SAFETY: `root` is NUL-terminated by the call above; `name` is writable.
        unsafe {
            GetVolumeInformationW(
                PCWSTR(root.as_ptr()),
                None,
                None,
                None,
                None,
                Some(&mut name),
            )
        }
        .ok()?;
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        Some(String::from_utf16_lossy(&name[..end]))
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::path::Path;

    pub fn free_bytes(_path: &Path) -> Option<u64> {
        None
    }
    pub fn filesystem_name(_path: &Path) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_space_of_a_missing_folder_is_its_volumes() {
        let dir = tempfile::tempdir().unwrap();
        let free = free_bytes(dir.path()).expect("the temp folder's volume reports space");
        assert!(free > 0);
        assert!(free_bytes(&dir.path().join("not/yet/created")).is_some());
    }

    #[test]
    fn only_ntfs_or_refs_or_unknown_is_accepted() {
        assert!(local_ntfs_or_refs(Some("NTFS")));
        assert!(local_ntfs_or_refs(Some("ReFS")));
        assert!(local_ntfs_or_refs(None));
        assert!(!local_ntfs_or_refs(Some("FAT32")));
        assert!(!local_ntfs_or_refs(Some("exFAT")));
    }
}
