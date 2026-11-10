//! Types for `llama.lock.json`, the pinned llama.cpp builds (ARCHITECTURE.md 7.1).
//!
//! The lock file is compiled into the app, so the app knows every downloadable pack's URLs,
//! sizes and SHA-256 without asking the network. `scripts/fetch_llama.py` reads the same
//! file at build time.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::backend::BackendKind;

/// The lock file as compiled into this build.
const EMBEDDED: &str = include_str!("../../../../llama.lock.json");

/// The whole lock file. Unknown fields are ignored, so the file may grow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlamaLock {
    /// Schema version of the file.
    #[serde(default)]
    pub schema: u32,
    /// The pinned upstream llama.cpp build.
    pub llama_cpp: LlamaPin,
    /// File-name patterns kept when an upstream archive is unpacked (`*` wildcards).
    #[serde(default)]
    pub keep_files: Vec<String>,
    /// Microsoft VC++ runtime DLLs placed in every Windows runtime directory.
    #[serde(default)]
    pub vc_runtime_files: Vec<String>,
    /// What the installer ships, keyed by platform (`windows-x86_64`, `macos-aarch64`).
    #[serde(default)]
    pub installer: HashMap<String, InstallerEntry>,
    /// Backends downloaded on request in Settings.
    #[serde(default)]
    pub packs: Vec<PackSpec>,
}

/// The pinned upstream llama.cpp release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlamaPin {
    /// Build tag, for example `b12110`.
    pub tag: String,
    /// Semver release the build corresponds to, for example `0.9.0`.
    pub version: String,
    /// Full commit hash.
    pub commit: String,
    /// Upstream repository URL.
    pub repository: String,
}

/// What the installer ships for one platform.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallerEntry {
    /// Directory name under the bundled `llama/` resources (`main`).
    pub dir: String,
    /// Backends served from that directory.
    #[serde(default)]
    pub backends: Vec<BackendKind>,
    /// Upstream archives unpacked into it (empty when built from source).
    #[serde(default)]
    pub files: Vec<PackFile>,
    /// Present when the runtime is built in CI from the pinned commit (macOS).
    #[serde(default)]
    pub build_from_source: Option<SourceBuild>,
}

/// How CI builds a runtime from source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBuild {
    /// Repository to clone.
    pub git: String,
    /// Commit to check out.
    pub commit: String,
    /// CMake flags.
    #[serde(default)]
    pub cmake_flags: Vec<String>,
    /// CMake targets to build.
    #[serde(default)]
    pub targets: Vec<String>,
}

/// A backend downloaded on request (ROCm, CUDA).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSpec {
    /// Stable id (`cuda`, `rocm`).
    pub id: String,
    /// Which backend the pack provides.
    pub backend: BackendKind,
    /// Platform key, as in [`LlamaLock::installer`].
    pub platform: String,
    /// The build the pack comes from; part of [`dir`](Self::dir).
    pub build: String,
    /// Directory name under `<local data>/backends/`.
    pub dir: String,
    /// Name shown in Settings.
    pub title: String,
    /// Hardware and driver requirements, shown before download.
    pub requirements: String,
    /// License shown before download.
    pub license_name: String,
    /// Where the license text lives.
    pub license_url: String,
    /// Keep every unpacked file instead of filtering by [`LlamaLock::keep_files`].
    #[serde(default)]
    pub keep_all: bool,
    /// The upstream llama.cpp commit a third-party pack was built from, when recorded.
    #[serde(default)]
    pub upstream_llama_cpp_commit: Option<String>,
    /// Archives to download, verify and unpack together.
    pub files: Vec<PackFile>,
}

impl PackSpec {
    /// Total download size in bytes.
    pub fn download_size(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }
}

/// One downloadable archive, pinned by size and SHA-256.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackFile {
    /// File name of the archive.
    pub name: String,
    /// HTTPS download URL (a GitHub release asset).
    pub url: String,
    /// Exact size in bytes.
    pub size: u64,
    /// Lower-case hex SHA-256.
    pub sha256: String,
}

impl LlamaLock {
    /// Parses a lock file.
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// The installer entry for the platform this build runs on, if any.
    pub fn installer_for_current_platform(&self) -> Option<&InstallerEntry> {
        self.installer.get(current_platform())
    }

    /// Packs offered on the platform this build runs on.
    pub fn packs_for_current_platform(&self) -> impl Iterator<Item = &PackSpec> {
        self.packs
            .iter()
            .filter(|pack| pack.platform == current_platform())
    }

    /// The pack with this id, on any platform.
    pub fn pack(&self, id: &str) -> Option<&PackSpec> {
        self.packs.iter().find(|pack| pack.id == id)
    }
}

/// The lock file compiled into this build, parsed once.
///
/// # Panics
///
/// Panics if the embedded file does not parse. The file is part of the source tree and a
/// unit test parses it, so a broken lock file cannot pass CI.
pub fn embedded() -> &'static LlamaLock {
    static LOCK: OnceLock<LlamaLock> = OnceLock::new();
    LOCK.get_or_init(|| match LlamaLock::parse(EMBEDDED) {
        Ok(lock) => lock,
        // The input is a compile-time constant checked by `embedded_lock_parses`.
        Err(error) => panic!("embedded llama.lock.json is invalid: {error}"),
    })
}

/// The platform key of this build, as used in the lock file: `<os>-<arch>`, for example
/// `windows-x86_64` or `macos-aarch64`.
pub fn current_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "windows-x86_64",
        ("windows", "aarch64") => "windows-aarch64",
        ("macos", "aarch64") => "macos-aarch64",
        ("macos", "x86_64") => "macos-x86_64",
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        _ => "unsupported",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_hex64(s: &str) -> bool {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    fn is_github_https(url: &str) -> bool {
        url.starts_with("https://github.com/")
    }

    #[test]
    fn embedded_lock_parses() {
        let lock = LlamaLock::parse(EMBEDDED).expect("embedded lock parses");
        assert_eq!(&lock, embedded());
        assert!(lock.llama_cpp.tag.starts_with('b'));
        assert_eq!(lock.llama_cpp.commit.len(), 40);
    }

    #[test]
    fn every_file_is_pinned_and_from_github() {
        let lock = embedded();
        let installer_files = lock.installer.values().flat_map(|entry| entry.files.iter());
        let pack_files = lock.packs.iter().flat_map(|pack| pack.files.iter());
        let mut count = 0;
        for file in installer_files.chain(pack_files) {
            assert!(
                is_hex64(&file.sha256),
                "{}: sha256 {:?}",
                file.name,
                file.sha256
            );
            assert!(
                is_github_https(&file.url),
                "{}: url {}",
                file.name,
                file.url
            );
            assert!(
                file.url.ends_with(&file.name),
                "{}: url does not end with the file name",
                file.name
            );
            assert!(file.size > 0, "{}: size", file.name);
            count += 1;
        }
        assert!(
            count >= 4,
            "expected the Windows installer zip plus pack zips, got {count}"
        );
    }

    #[test]
    fn windows_installer_serves_cpu_and_vulkan_from_main() {
        let entry = embedded()
            .installer
            .get("windows-x86_64")
            .expect("windows entry");
        assert_eq!(entry.dir, "main");
        assert_eq!(entry.backends, vec![BackendKind::Cpu, BackendKind::Vulkan]);
        let mac = embedded()
            .installer
            .get("macos-aarch64")
            .expect("macos entry");
        // CPU from the same binary, so a failed Metal load can step down (7.4).
        assert_eq!(mac.backends, vec![BackendKind::Metal, BackendKind::Cpu]);
        assert!(mac.build_from_source.is_some());
    }

    #[test]
    fn packs_have_unique_ids_and_build_specific_dirs() {
        let lock = embedded();
        let mut ids: Vec<&str> = lock.packs.iter().map(|pack| pack.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), lock.packs.len());
        for pack in &lock.packs {
            assert!(
                pack.dir
                    .contains(pack.build.trim_start_matches("lemonade-")),
                "{}",
                pack.dir
            );
            assert!(pack.download_size() > 0);
        }
        let rocm = lock.pack("rocm").expect("rocm pack");
        assert_eq!(rocm.backend, BackendKind::Rocm);
        assert!(rocm.keep_all);
        assert!(
            !lock.pack("cuda").expect("cuda pack").keep_all,
            "keep_all defaults to false"
        );
    }

    #[test]
    fn current_platform_is_known_on_ci_targets() {
        assert_ne!(current_platform(), "unsupported");
    }
}
