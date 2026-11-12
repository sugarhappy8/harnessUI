//! What this computer offers (item5-spec.md section 2, the owner's answers Q1 to Q3): which
//! Settings rows can be installed and why not, whether a Mac has Apple's command line tools
//! (R7), and how much memory the computer has (the Ghidra row's 16 GB line).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::lock::ReverseLock;
use crate::{AppleTools, Component};

/// A platform, as the lock files name it (`runtime::llama::lock::current_platform`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Platform {
    /// `windows-x86_64` (the owner's PC).
    WindowsX64,
    /// `windows-aarch64`.
    WindowsArm64,
    /// `macos-aarch64` (the owner's Mac).
    MacosArm64,
    /// `macos-x86_64`.
    MacosX64,
    /// `linux-x86_64` (development).
    LinuxX64,
    /// `linux-aarch64`.
    LinuxArm64,
    /// Anything else.
    Unsupported,
}

/// The operating system family of a [`Platform`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    /// Windows.
    Windows,
    /// macOS.
    Macos,
    /// Linux.
    Linux,
    /// Anything else.
    Other,
}

impl Platform {
    /// The platforms the app ships for, each of which the lock must cover with rea and Node.
    pub const SHIPPED: [Platform; 4] = [
        Platform::WindowsX64,
        Platform::MacosArm64,
        Platform::MacosX64,
        Platform::LinuxX64,
    ];

    /// The platform this build runs on.
    pub fn current() -> Self {
        Self::from_key(runtime::llama::lock::current_platform()).unwrap_or(Platform::Unsupported)
    }

    /// The platform for a lock key.
    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "windows-x86_64" => Platform::WindowsX64,
            "windows-aarch64" => Platform::WindowsArm64,
            "macos-aarch64" => Platform::MacosArm64,
            "macos-x86_64" => Platform::MacosX64,
            "linux-x86_64" => Platform::LinuxX64,
            "linux-aarch64" => Platform::LinuxArm64,
            "unsupported" => Platform::Unsupported,
            _ => return None,
        })
    }

    /// The lock key.
    pub fn key(self) -> &'static str {
        match self {
            Platform::WindowsX64 => "windows-x86_64",
            Platform::WindowsArm64 => "windows-aarch64",
            Platform::MacosArm64 => "macos-aarch64",
            Platform::MacosX64 => "macos-x86_64",
            Platform::LinuxX64 => "linux-x86_64",
            Platform::LinuxArm64 => "linux-aarch64",
            Platform::Unsupported => "unsupported",
        }
    }

    /// The operating system family.
    pub fn os(self) -> Os {
        match self {
            Platform::WindowsX64 | Platform::WindowsArm64 => Os::Windows,
            Platform::MacosArm64 | Platform::MacosX64 => Os::Macos,
            Platform::LinuxX64 | Platform::LinuxArm64 => Os::Linux,
            Platform::Unsupported => Os::Other,
        }
    }
}

/// Exact reasons shown in Settings (item5-spec.md WP9a `platform.rs` and the owner's
/// answers, which override the spec's Mac Ghidra line).
pub const KUNA_NOT_ON_WINDOWS: &str = "rea can't use Kuna on Windows yet.";
/// Q2 = B: no Ghidra on a Mac in 0.0.6.
pub const GHIDRA_NOT_ON_MAC: &str = "Not available on Mac in this version.";
/// Any other part missing for this computer.
pub const NOT_FOR_THIS_COMPUTER: &str = "Not available for this computer.";
/// A release build whose lock lacks a publisher check for one of the row's parts (never
/// meant to ship: `scripts/pin_reverse.py --check --release` fails on such a lock).
pub const NOT_IN_THIS_VERSION: &str = "Not available in this version of harnessUI.";

/// Whether this build follows the release rules: only builds with their publishers' checks
/// are offered ([`crate::lock::release_checks`]). Release builds do; debug builds also offer
/// a part pinned on first download only, so development can go on before the lead pins it
/// where GitHub's API answers.
pub const RELEASE_RULES: bool = cfg!(not(debug_assertions));

/// Whether `component` can be installed here, with the reason shown when it cannot.
///
/// A row is offered exactly when the lock has a build of every one of its parts for this
/// platform, so Kuna on Windows turns on by itself once a later pinned rea supports it.
pub fn availability(
    lock: &ReverseLock,
    component: Component,
    platform: Platform,
) -> (bool, Option<String>) {
    availability_with(lock, component, platform, RELEASE_RULES)
}

/// [`availability`], with the release rules on or off (tests, and the installer's own
/// setting).
pub fn availability_with(
    lock: &ReverseLock,
    component: Component,
    platform: Platform,
    release_rules: bool,
) -> (bool, Option<String>) {
    let complete = lock.parts_of(component).next().is_some()
        && lock
            .parts_of(component)
            .all(|p| p.build_for(platform).is_some());
    if complete && release_rules {
        let ready = lock
            .parts_of(component)
            .filter_map(|p| p.build_for(platform).map(|b| (p, b)))
            .all(|(p, b)| b.release_ready(&p.name));
        if !ready {
            return (false, Some(NOT_IN_THIS_VERSION.to_string()));
        }
    }
    if complete {
        return (true, None);
    }
    let reason = match (component, platform.os()) {
        (Component::Kuna, Os::Windows) => KUNA_NOT_ON_WINDOWS,
        (Component::Ghidra, Os::Macos) => GHIDRA_NOT_ON_MAC,
        _ => NOT_FOR_THIS_COMPUTER,
    };
    (false, Some(reason.to_string()))
}

/// Apple's command line tools inside the Developer folder Xcode or the tools install.
const CLT_SWIFTC: &str = "Library/Developer/CommandLineTools/usr/bin/swiftc";
/// The link `xcode-select` keeps to the chosen developer folder.
const XCODE_SELECT_LINK: &str = "var/db/xcode_select_link";

/// Whether this Mac has Apple's command line tools (R7), checked fresh by looking at files
/// only, never by running a program (`/usr/bin/xcrun` without the tools is Apple's stub,
/// which opens Apple's install window). `NotNeeded` on other systems.
pub fn apple_tools(platform: Platform) -> AppleTools {
    apple_tools_at(platform, Path::new("/"))
}

/// [`apple_tools`] with every absolute path read under `root` (tests use a fake tree).
pub fn apple_tools_at(platform: Platform, root: &Path) -> AppleTools {
    if platform.os() != Os::Macos {
        return AppleTools::NotNeeded;
    }
    if root.join(CLT_SWIFTC).is_file() {
        return AppleTools::Present;
    }
    let link = root.join(XCODE_SELECT_LINK);
    let Ok(target) = std::fs::read_link(&link) else {
        return AppleTools::Missing;
    };
    let text = target.to_string_lossy();
    if is_network_target(&text) {
        // Never resolved: a developer folder on a share would make every status check
        // wait on the network.
        return AppleTools::Missing;
    }
    let developer: PathBuf = if target.is_absolute() {
        root.join(target.strip_prefix("/").unwrap_or(&target))
    } else {
        root.join("var/db").join(&target)
    };
    let found = [
        "usr/bin/swiftc",
        "Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc",
    ]
    .iter()
    .any(|rel| developer.join(rel).is_file());
    if found {
        AppleTools::Present
    } else {
        AppleTools::Missing
    }
}

/// A link target that names a network location (never followed).
fn is_network_target(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    lower.starts_with("//")
        || lower.starts_with("\\\\")
        || lower.starts_with("/net/")
        || lower == "/net"
        || lower.starts_with("/network/")
        || lower.contains("://")
        || lower.starts_with("smb:")
        || lower.starts_with("afp:")
        || lower.starts_with("nfs:")
}

/// This computer's memory in GiB, rounded to the nearest whole number (a "16 GB" Mac reads
/// 16). `None` when the system does not say.
pub fn memory_gb() -> Option<u32> {
    models::hardware::system_memory().map(|m| gib(m.total_bytes))
}

fn gib(bytes: u64) -> u32 {
    u32::try_from((bytes + (1 << 29)) >> 30).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::embedded;

    #[test]
    fn keys_round_trip() {
        for p in [
            Platform::WindowsX64,
            Platform::WindowsArm64,
            Platform::MacosArm64,
            Platform::MacosX64,
            Platform::LinuxX64,
            Platform::LinuxArm64,
            Platform::Unsupported,
        ] {
            assert_eq!(Platform::from_key(p.key()), Some(p));
        }
        assert_eq!(Platform::from_key("plan9-mips"), None);
        assert_ne!(Platform::current(), Platform::Unsupported);
    }

    #[test]
    fn availability_with_the_exact_reasons() {
        let lock = embedded();
        let a = |c, p| availability(lock, c, p);
        for p in Platform::SHIPPED {
            assert_eq!(a(Component::Rea, p), (true, None), "{}", p.key());
        }
        assert_eq!(
            a(Component::Kuna, Platform::WindowsX64),
            (false, Some("rea can't use Kuna on Windows yet.".into()))
        );
        assert_eq!(a(Component::Ghidra, Platform::WindowsX64), (true, None));
        for mac in [Platform::MacosArm64, Platform::MacosX64] {
            assert_eq!(a(Component::Kuna, mac), (true, None));
            assert_eq!(
                a(Component::Ghidra, mac),
                (false, Some("Not available on Mac in this version.".into()))
            );
        }
        assert_eq!(a(Component::Kuna, Platform::LinuxX64), (true, None));
        assert_eq!(a(Component::Ghidra, Platform::LinuxX64), (true, None));
        assert_eq!(
            a(Component::Rea, Platform::LinuxArm64),
            (false, Some("Not available for this computer.".into()))
        );
    }

    /// A release build offers a row only when every part has its publishers' checks: Ghidra
    /// pinned on first download only is not offered, and nothing else changes.
    #[test]
    fn release_rules_need_each_publishers_checks() {
        let mut lock = embedded().clone();
        for b in &mut lock
            .parts
            .iter_mut()
            .find(|p| p.name == "ghidra")
            .unwrap()
            .builds
        {
            b.verified_by = vec!["first-download-only".into()];
        }
        let r = |c, p| availability_with(&lock, c, p, true);
        assert_eq!(
            r(Component::Ghidra, Platform::WindowsX64),
            (false, Some(NOT_IN_THIS_VERSION.into()))
        );
        assert_eq!(
            availability_with(&lock, Component::Ghidra, Platform::WindowsX64, false),
            (true, None),
            "a debug build still offers it"
        );
        for p in Platform::SHIPPED {
            assert_eq!(r(Component::Rea, p), (true, None), "{}", p.key());
        }
        assert_eq!(r(Component::Kuna, Platform::MacosArm64), (true, None));
        assert_eq!(
            r(Component::Ghidra, Platform::MacosArm64),
            (false, Some(GHIDRA_NOT_ON_MAC.into())),
            "the platform's own reason comes first"
        );
        for b in &mut lock
            .parts
            .iter_mut()
            .find(|p| p.name == "ghidra")
            .unwrap()
            .builds
        {
            b.verified_by = vec!["github-digest".into()];
        }
        assert_eq!(
            availability_with(&lock, Component::Ghidra, Platform::WindowsX64, true),
            (true, None)
        );
    }

    /// Q1 = A: the Kuna row turns on by itself when a later pinned rea offers it.
    #[test]
    fn kuna_on_windows_turns_on_when_the_lock_offers_it() {
        let mut lock = embedded().clone();
        let kuna = lock.parts.iter_mut().find(|p| p.name == "kuna").unwrap();
        let mut b = kuna.builds[0].clone();
        b.platform = "windows-x86_64".into();
        kuna.builds.push(b);
        assert_eq!(
            availability(&lock, Component::Kuna, Platform::WindowsX64),
            (true, None)
        );
    }

    #[test]
    fn apple_tools_are_not_needed_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        for p in [Platform::WindowsX64, Platform::LinuxX64] {
            assert_eq!(apple_tools_at(p, dir.path()), AppleTools::NotNeeded);
        }
        assert_eq!(
            apple_tools_at(Platform::MacosArm64, dir.path()),
            AppleTools::Missing
        );
    }

    #[test]
    fn apple_tools_through_the_command_line_tools_folder() {
        let dir = tempfile::tempdir().unwrap();
        let swiftc = dir.path().join(CLT_SWIFTC);
        std::fs::create_dir_all(swiftc.parent().unwrap()).unwrap();
        std::fs::write(&swiftc, b"").unwrap();
        assert_eq!(
            apple_tools_at(Platform::MacosArm64, dir.path()),
            AppleTools::Present
        );
        assert_eq!(
            apple_tools_at(Platform::MacosX64, dir.path()),
            AppleTools::Present
        );
    }

    #[cfg(unix)]
    #[test]
    fn apple_tools_through_the_xcode_select_link() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("var/db")).unwrap();
        let dev = "Applications/Xcode.app/Contents/Developer";
        let link = root.join(XCODE_SELECT_LINK);
        std::os::unix::fs::symlink(format!("/{dev}"), &link).unwrap();
        // The link exists but the developer folder has no swiftc.
        assert_eq!(
            apple_tools_at(Platform::MacosArm64, root),
            AppleTools::Missing
        );
        let swiftc = root
            .join(dev)
            .join("Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc");
        std::fs::create_dir_all(swiftc.parent().unwrap()).unwrap();
        std::fs::write(&swiftc, b"").unwrap();
        assert_eq!(
            apple_tools_at(Platform::MacosArm64, root),
            AppleTools::Present
        );
    }

    /// A link to a network path is read, never followed: even when the share's folder
    /// exists under the fake root, the tools count as missing.
    #[cfg(unix)]
    #[test]
    fn a_link_to_a_network_path_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("var/db")).unwrap();
        let swiftc = root.join("net/server/Developer/usr/bin/swiftc");
        std::fs::create_dir_all(swiftc.parent().unwrap()).unwrap();
        std::fs::write(&swiftc, b"").unwrap();
        for target in [
            "/net/server/Developer",
            "//server/Developer",
            "smb://server/Dev",
        ] {
            let link = root.join(XCODE_SELECT_LINK);
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(target, &link).unwrap();
            assert_eq!(
                apple_tools_at(Platform::MacosArm64, root),
                AppleTools::Missing,
                "{target}"
            );
        }
    }

    #[test]
    fn memory_rounds_to_whole_gib() {
        assert_eq!(gib(17_179_869_184), 16);
        assert_eq!(gib(16_000_000_000), 15);
        assert_eq!(gib(0), 0);
    }
}
