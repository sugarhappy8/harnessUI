//! Types for `sd.lock.json`, the pinned stable-diffusion.cpp build that runs Studio's
//! local pictures and videos (ARCHITECTURE.md 7.1, as for `llama.lock.json`).
//!
//! The lock file is compiled into the app, so the app knows the Windows engine pack's URL,
//! size and SHA-256 without asking the network. `scripts/fetch_sd.py` reads the same file
//! to build `sd-cli` for macOS and Linux and to check the Windows pack in CI.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::llama::lock::{InstallerEntry, PackSpec, SourceBuild, current_platform};

/// The lock file as compiled into this build.
const EMBEDDED: &str = include_str!("../../../../sd.lock.json");

/// The whole lock file. Unknown fields are ignored, so the file may grow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdLock {
    /// Schema version of the file.
    #[serde(default)]
    pub schema: u32,
    /// The pinned upstream stable-diffusion.cpp build.
    pub sd_cpp: SdPin,
    /// The submodules a source build fetches, by path, with the commit the pinned
    /// commit records for each (`ggml`, `thirdparty/libwebp`, `thirdparty/libwebm`).
    #[serde(default)]
    pub submodules: BTreeMap<String, String>,
    /// File-name patterns kept when the Windows pack is unpacked (`*` wildcards).
    #[serde(default)]
    pub keep_files: Vec<String>,
    /// Microsoft Visual C++ runtime DLLs copied into the Windows engine folder from
    /// llama.cpp's installed folder.
    #[serde(default)]
    pub vc_runtime_files: Vec<String>,
    /// The DLLs that each program and DLL the Windows pack keeps imports, as `objdump -p`
    /// printed them at pinning (`ggml-base.dll` and `ggml-cpu-*.dll` import `VCOMP140.DLL`).
    #[serde(default)]
    pub windows_imports: BTreeMap<String, Vec<String>>,
    /// The engine downloaded with the first local model (Windows).
    #[serde(default)]
    pub packs: Vec<PackSpec>,
    /// What the app bundle ships, keyed by platform (`macos-aarch64`, `macos-x86_64`).
    #[serde(default)]
    pub installer: HashMap<String, InstallerEntry>,
    /// Development builds, keyed by platform (`linux-x86_64`).
    #[serde(default)]
    pub dev: HashMap<String, SourceBuild>,
}

/// The pinned upstream stable-diffusion.cpp release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdPin {
    /// Release tag, for example `master-1106-532d2c1`.
    pub tag: String,
    /// Build number within the tag, for example `1106`.
    pub build: u32,
    /// Full commit hash.
    pub commit: String,
    /// Upstream repository URL.
    pub repository: String,
}

impl SdLock {
    /// Parses a lock file.
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// The engine pack for the platform this build runs on, if there is one (Windows).
    pub fn pack_for_current_platform(&self) -> Option<&PackSpec> {
        self.packs
            .iter()
            .find(|pack| pack.platform == current_platform())
    }

    /// Whether unpacking the Windows pack keeps `file_name`: it matches one of
    /// [`keep_files`](Self::keep_files), ignoring case, as llama.cpp's packs are filtered.
    pub fn keeps(&self, file_name: &str) -> bool {
        self.keep_files
            .iter()
            .any(|pattern| crate::packs::glob_match(pattern, file_name))
    }
}

/// Windows DLLs every PC has (KERNEL32, USER32, ADVAPI32, SHELL32, OLE32, WS2_32, VERSION,
/// `api-ms-win-*`, …): never shipped, never checked. The Visual C++ runtime and the Vulkan
/// loader are not among them: a PC may lack either.
pub fn is_windows_system_dll(name: &str) -> bool {
    const SYSTEM: &[&str] = &[
        "advapi32.dll",
        "bcrypt.dll",
        "cfgmgr32.dll",
        "comctl32.dll",
        "comdlg32.dll",
        "crypt32.dll",
        "dbghelp.dll",
        "gdi32.dll",
        "imm32.dll",
        "iphlpapi.dll",
        "kernel32.dll",
        "ntdll.dll",
        "ole32.dll",
        "oleaut32.dll",
        "powrprof.dll",
        "psapi.dll",
        "secur32.dll",
        "setupapi.dll",
        "shell32.dll",
        "shlwapi.dll",
        "ucrtbase.dll",
        "user32.dll",
        "userenv.dll",
        "version.dll",
        "winmm.dll",
        "ws2_32.dll",
    ];
    let name = name.to_ascii_lowercase();
    name.starts_with("api-ms-win-")
        || name.starts_with("ext-ms-win-")
        || SYSTEM.contains(&name.as_str())
}

/// The lock file compiled into this build, parsed once.
///
/// # Panics
///
/// Panics if the embedded file does not parse. The file is part of the source tree and a
/// unit test parses it, so a broken lock file cannot pass CI.
pub fn embedded() -> &'static SdLock {
    static LOCK: OnceLock<SdLock> = OnceLock::new();
    LOCK.get_or_init(|| match SdLock::parse(EMBEDDED) {
        Ok(lock) => lock,
        // The input is a compile-time constant checked by `the_embedded_lock_parses`.
        Err(error) => panic!("embedded sd.lock.json is invalid: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llama::backend::BackendKind;

    #[test]
    fn the_embedded_lock_parses() {
        assert_eq!(
            &SdLock::parse(EMBEDDED).expect("sd.lock.json parses"),
            embedded()
        );
    }

    #[test]
    fn the_embedded_lock_pins_one_build() {
        let l = embedded();
        assert_eq!(
            (l.sd_cpp.tag.as_str(), l.sd_cpp.build),
            ("master-1106-532d2c1", 1106)
        );
        assert!(
            l.sd_cpp.commit.len() == 40
                && l.sd_cpp
                    .commit
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }

    #[test]
    fn the_windows_pack_is_the_vulkan_zip_from_github() {
        let p = embedded()
            .packs
            .iter()
            .find(|p| p.id == "studio-vulkan")
            .unwrap();
        assert_eq!(
            (p.platform.as_str(), p.dir.as_str(), p.backend),
            ("windows-x86_64", "sd-1106", BackendKind::Vulkan)
        );
        for f in &p.files {
            assert!(f.url.starts_with(
                "https://github.com/leejet/stable-diffusion.cpp/releases/download/master-1106-532d2c1/"
            ));
            assert!(
                f.size > 0 && f.sha256.len() == 64,
                "{}: pin its size and SHA-256",
                f.name
            );
        }
    }

    #[test]
    fn the_engine_keeps_its_own_files_and_none_of_llamas() {
        let l = embedded();
        assert!(
            l.keeps("sd-cli.exe") && l.keeps("stable-diffusion.cpp.txt") && l.keeps("ggml.txt")
        );
        assert!(
            !l.keeps("llama-server.exe")
                && !l.keeps("sd-server.exe")
                && !l.keeps("start-server.bat")
        );
    }

    #[test]
    fn the_engine_keeps_every_dll_it_imports() {
        // every import of every program and DLL the pack keeps (ggml-base.dll and the
        // ggml-cpu-*.dll too, not only sd-cli.exe and stable-diffusion.dll) is a system DLL,
        // a Visual C++ runtime file, the Vulkan loader, or a file of the pinned zip that is kept
        let l = embedded();
        let listing = include_str!("../../tests/fixtures/sd_win_pack_listing.txt");
        let kept_programs: Vec<&str> = listing
            .lines()
            .filter(|f| {
                let f = f.to_ascii_lowercase();
                (f.ends_with(".dll") || f.ends_with(".exe")) && l.keeps(&f)
            })
            .collect();
        assert!(!kept_programs.is_empty());
        for program in &kept_programs {
            assert!(
                l.windows_imports.contains_key(*program),
                "{program} is kept, but its imports are not recorded"
            );
        }
        for (exe, dlls) in &l.windows_imports {
            assert!(
                kept_programs.contains(&exe.as_str()),
                "{exe} is not a kept file of the pinned zip"
            );
            for dll in dlls {
                let vc = l
                    .vc_runtime_files
                    .iter()
                    .any(|f| f.eq_ignore_ascii_case(dll));
                // ggml-vulkan.dll's loader comes with the graphics driver, or from llama's
                // bundled `vulkan` folder at the end of the engine's PATH
                let vulkan_loader = dll.eq_ignore_ascii_case("vulkan-1.dll");
                let shipped = listing.lines().any(|f| f.eq_ignore_ascii_case(dll)) && l.keeps(dll);
                assert!(
                    is_windows_system_dll(dll) || vc || vulkan_loader || shipped,
                    "{exe} imports {dll}, which the pack would not have"
                );
            }
        }
    }

    #[test]
    fn the_vc_runtime_covers_every_msvc_import() {
        let l = embedded();
        // vcomp140.dll is MSVC's OpenMP runtime, which ggml-base.dll and ggml-cpu-*.dll import
        for dll in l.windows_imports.values().flatten().filter(|d| {
            let d = d.to_ascii_lowercase();
            d.starts_with("msvcp") || d.starts_with("vcruntime") || d.starts_with("vcomp")
        }) {
            assert!(
                l.vc_runtime_files
                    .iter()
                    .any(|f| f.eq_ignore_ascii_case(dll)),
                "{dll}"
            );
        }
        // packs copy the runtime from llama's installed folder, so it must hold them too
        let llama = crate::llama::lock::embedded();
        for f in &l.vc_runtime_files {
            assert!(
                llama.vc_runtime_files.contains(f),
                "{f} is not in llama.lock.json"
            );
        }
    }

    #[test]
    fn both_macos_slices_build_sd_cli_from_the_pin() {
        for key in ["macos-aarch64", "macos-x86_64"] {
            let b = embedded().installer[key]
                .build_from_source
                .as_ref()
                .unwrap();
            assert_eq!(
                (b.targets.as_slice(), b.commit.as_str()),
                (
                    ["sd-cli".to_string()].as_slice(),
                    embedded().sd_cpp.commit.as_str()
                )
            );
        }
    }

    #[test]
    fn system_dlls_are_the_ones_every_pc_has() {
        for dll in [
            "KERNEL32.dll",
            "SHELL32.dll",
            "api-ms-win-crt-runtime-l1-1-0.dll",
            "ws2_32.DLL",
        ] {
            assert!(is_windows_system_dll(dll), "{dll}");
        }
        for dll in [
            "vulkan-1.dll",
            "VCOMP140.DLL",
            "MSVCP140.dll",
            "ggml.dll",
            "libwebp.dll",
        ] {
            assert!(!is_windows_system_dll(dll), "{dll}");
        }
    }

    #[test]
    fn the_llama_lock_offers_no_studio_pack() {
        // list_packs never shows it
        assert!(
            crate::llama::lock::embedded()
                .packs
                .iter()
                .all(|p| !p.id.starts_with("studio"))
        );
    }
}
