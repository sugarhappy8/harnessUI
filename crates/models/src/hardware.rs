//! System memory, for the fit check when a model runs from RAM (CPU, or layers
//! spilled from a discrete GPU). GPU memory comes from llama-server's own device
//! listing (`runtime::probe`), which reports what the backend can actually use.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMemory {
    pub total_bytes: u64,
    /// Memory the OS reports as available to a new process without swapping.
    pub available_bytes: u64,
}

/// The machine's memory, or `None` if the OS call fails.
pub fn system_memory() -> Option<SystemMemory> {
    imp::system_memory()
}

#[cfg(windows)]
mod imp {
    use super::SystemMemory;
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    pub fn system_memory() -> Option<SystemMemory> {
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // SAFETY: `status` is a properly sized MEMORYSTATUSEX with dwLength set, as the
        // API requires; the call only writes into it.
        unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
        Some(SystemMemory {
            total_bytes: status.ullTotalPhys,
            available_bytes: status.ullAvailPhys,
        })
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::SystemMemory;

    fn sysctl_u64(name: &std::ffi::CStr) -> Option<u64> {
        let mut value: u64 = 0;
        let mut size = std::mem::size_of::<u64>();
        // SAFETY: `name` is NUL-terminated; `value`/`size` describe a writable u64.
        let rc = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&mut value as *mut u64).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        (rc == 0 && size == std::mem::size_of::<u64>()).then_some(value)
    }

    pub fn system_memory() -> Option<SystemMemory> {
        let total = sysctl_u64(c"hw.memsize")?;
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if page <= 0 {
            return None;
        }
        let mut stats: libc::vm_statistics64 = unsafe {
            // SAFETY: vm_statistics64 is a plain C struct of integers; all-zero is valid.
            std::mem::zeroed()
        };
        let mut count = libc::HOST_VM_INFO64_COUNT;
        // SAFETY: `stats` and `count` describe a buffer of HOST_VM_INFO64_COUNT
        // integers, as host_statistics64 requires for HOST_VM_INFO64.
        #[allow(deprecated)]
        let rc = unsafe {
            libc::host_statistics64(
                libc::mach_host_self(),
                libc::HOST_VM_INFO64,
                (&mut stats as *mut libc::vm_statistics64).cast(),
                &mut count,
            )
        };
        let available = if rc == 0 {
            let pages = u64::from(stats.free_count)
                + u64::from(stats.inactive_count)
                + u64::from(stats.purgeable_count)
                + u64::from(stats.speculative_count);
            (pages * page as u64).min(total)
        } else {
            total / 2
        };
        Some(SystemMemory {
            total_bytes: total,
            available_bytes: available,
        })
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::SystemMemory;

    /// Linux, for development and CI.
    pub fn system_memory() -> Option<SystemMemory> {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        parse_meminfo(&text)
    }

    pub(super) fn parse_meminfo(text: &str) -> Option<SystemMemory> {
        let field = |name: &str| {
            text.lines().find_map(|l| {
                let rest = l.strip_prefix(name)?.strip_prefix(':')?;
                rest.trim()
                    .trim_end_matches("kB")
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .map(|kb| kb * 1024)
            })
        };
        Some(SystemMemory {
            total_bytes: field("MemTotal")?,
            available_bytes: field("MemAvailable")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_plausible_memory() {
        let m = system_memory().expect("memory");
        assert!(m.total_bytes > 256 * 1024 * 1024);
        assert!(m.available_bytes <= m.total_bytes);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn parses_meminfo() {
        let m = imp::parse_meminfo(
            "MemTotal:       16384000 kB\nMemFree: 1 kB\nMemAvailable:    8192000 kB\n",
        )
        .unwrap();
        assert_eq!(m.total_bytes, 16_384_000 * 1024);
        assert_eq!(m.available_bytes, 8_192_000 * 1024);
        assert!(imp::parse_meminfo("nothing").is_none());
    }
}
