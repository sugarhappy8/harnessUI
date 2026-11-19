//! Where the Studio engine (`sd-cli`) is on this computer, and installing it on Windows
//! (docs/dev/0.1.0/studio-local-design.md, L2).
//!
//! Three folders can hold it, looked at in this order:
//!
//! - a development folder named by [`SD_DEV_OVERRIDE_ENV`] (Linux development and CI's
//!   contract tests, with an `sd-cli` built from the pinned commit);
//! - the app's own folder (macOS, where `sd-cli` is bundled beside `llama-server`);
//! - the Windows engine pack, downloaded with the first local model and installed into
//!   `<local data>/engines/<dir>` (`sd-1106`), never into llama.cpp's folder: the pack's
//!   `ggml*.dll` share names with llama.cpp's, and Windows loads a program's DLLs from the
//!   program's own folder first.
//!
//! This module looks at files and downloads the pack; it never starts a process.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

use super::lock::SdLock;
use crate::llama::backend::{PACK_COMPLETE_MARKER, VULKAN_FALLBACK_DIR, VULKAN_RUNTIME_FILE};
use crate::llama::lock::{PackSpec, current_platform};
use crate::packs::{self, InstallSpec, PackError, PackProgress, PackState};

/// Environment variable naming a development folder that holds `sd-cli` (Linux development
/// and CI's contract tests).
pub const SD_DEV_OVERRIDE_ENV: &str = "APP_DEV_SD_DIR";

/// File name of the engine program on this platform. On macOS Tauri strips the target
/// triple from `externalBin` names when bundling, so it is plain `sd-cli` there too.
pub fn program_file_name() -> &'static str {
    if cfg!(windows) {
        "sd-cli.exe"
    } else {
        "sd-cli"
    }
}

/// Where the engine came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineSource {
    /// Shipped with the app (macOS).
    Bundled,
    /// The Windows engine pack, downloaded with the first local model.
    Pack,
    /// A development folder named by [`SD_DEV_OVERRIDE_ENV`].
    Dev,
}

/// The engine found on this computer: the program to start and the folder to start it in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Engine {
    /// The `sd-cli` program.
    pub program: PathBuf,
    /// The program's folder, always the engine's working directory, so that Windows finds
    /// the pack's DLLs beside the program.
    pub dir: PathBuf,
    /// The pinned stable-diffusion.cpp build number (`1106`), whatever the source.
    pub build: u32,
    /// Where the engine came from.
    pub source: EngineSource,
    /// Arguments passed before the engine's own. Empty except for the fake engine in tests,
    /// a test binary that needs a test filter first.
    pub leading_args: Vec<OsString>,
    /// Variables added to the engine's environment. Empty except for the fake engine in
    /// tests.
    pub extra_env: Vec<(String, String)>,
    /// On Windows, llama.cpp's bundled Vulkan runtime folder (`<llama>/vulkan`, holding
    /// `vulkan-1.dll`): the last entry of the engine's `PATH`, so a PC whose graphics driver
    /// lacks the Vulkan runtime still finds it, both for a job and for `--list-devices`.
    /// `None` elsewhere, and when llama's folder is unknown or holds no runtime. Set by
    /// [`discover`] and [`install`] from [`EngineLayout::llama_dir`].
    pub vulkan_fallback: Option<PathBuf>,
}

impl Engine {
    /// The engine whose program is [`program_file_name`] in `dir`, with `vulkan_fallback`
    /// at the end of its `PATH`.
    fn in_dir(
        dir: &Path,
        lock: &SdLock,
        source: EngineSource,
        vulkan_fallback: Option<PathBuf>,
    ) -> Self {
        Self {
            program: dir.join(program_file_name()),
            dir: dir.to_path_buf(),
            build: lock.sd_cpp.build,
            source,
            leading_args: Vec::new(),
            extra_env: Vec::new(),
            vulkan_fallback,
        }
    }
}

/// Where to look for the engine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EngineLayout {
    /// The folder holding the app's own `sd-cli`: the app's executable folder on macOS;
    /// `None` where the app ships none.
    pub bundled_dir: Option<PathBuf>,
    /// The folder the engine pack is installed into: `<local data>/engines`.
    pub engines_dir: PathBuf,
    /// llama.cpp's installed folder (`resources/llama/main` on Windows), which the pack's
    /// Visual C++ runtime files are copied from, and whose bundled Vulkan runtime folder ends
    /// the engine's `PATH` on Windows ([`Engine::vulkan_fallback`]). Never the engine's
    /// folder.
    pub llama_dir: Option<PathBuf>,
    /// A development folder, from [`SD_DEV_OVERRIDE_ENV`].
    pub dev_override: Option<PathBuf>,
}

impl EngineLayout {
    /// A layout with `dev_override` taken from [`SD_DEV_OVERRIDE_ENV`] when it is set and
    /// not empty.
    pub fn with_env_override(
        bundled_dir: Option<PathBuf>,
        engines_dir: PathBuf,
        llama_dir: Option<PathBuf>,
    ) -> Self {
        let dev_override = std::env::var_os(SD_DEV_OVERRIDE_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        Self {
            bundled_dir,
            engines_dir,
            llama_dir,
            dev_override,
        }
    }
}

/// Finds the engine: the development folder, then the bundled program, then the engine
/// pack installed for `platform`. `None` when no folder holds the program, or the pack's
/// folder lacks a file it needs (see [`pack_state`]). On Windows the engine found records
/// llama.cpp's bundled Vulkan runtime folder ([`Engine::vulkan_fallback`]).
pub fn discover(layout: &EngineLayout, lock: &SdLock, platform: &str) -> Option<Engine> {
    let folders = [
        (layout.dev_override.as_deref(), EngineSource::Dev),
        (layout.bundled_dir.as_deref(), EngineSource::Bundled),
    ];
    for (dir, source) in folders {
        if let Some(dir) = dir
            && dir.join(program_file_name()).is_file()
        {
            let fallback = vulkan_fallback(layout.llama_dir.as_deref(), platform);
            return Some(Engine::in_dir(dir, lock, source, fallback));
        }
    }
    installed_pack(lock, pack_for(lock, platform)?, layout)
}

/// The engine pack's state from what is on disk. `Installed` needs the `.complete` marker
/// *and* the program and every DLL it needs, the Visual C++ runtime files included:
/// antivirus quarantine can remove one after the install, and Studio then offers the
/// download again instead of failing every run with a missing library. Such a folder is
/// `NotInstalled`, or `Partial` once a new download has begun.
pub fn pack_state(lock: &SdLock, engines_dir: &Path) -> PackState {
    if lock
        .packs
        .iter()
        .any(|pack| is_complete(lock, pack, &engines_dir.join(&pack.dir)))
    {
        return PackState::Installed;
    }
    let bytes: u64 = lock
        .packs
        .iter()
        .map(|pack| packs::partial_bytes(&pack.files, engines_dir))
        .sum();
    if bytes > 0 {
        PackState::Partial { bytes }
    } else {
        PackState::NotInstalled
    }
}

/// Downloads and installs the engine pack (resuming a partial download) into
/// `layout.engines_dir`, and returns the engine in it. Its Visual C++ runtime files are
/// copied from llama.cpp's installed folder, `layout.llama_dir`; nothing is written there.
/// [`PackError::WrongPlatform`] off Windows x86-64, the one platform with a pack; an error
/// before anything is downloaded when llama.cpp's folder is unknown or lacks a runtime file
/// (see [`vc_runtime_dir`]).
pub async fn install(
    lock: &SdLock,
    layout: &EngineLayout,
    cancel: &CancellationToken,
    progress: &(dyn Fn(PackProgress) + Send + Sync),
) -> Result<Engine, PackError> {
    let pack = pack_for(lock, current_platform()).ok_or(PackError::WrongPlatform)?;
    if let Some(engine) = installed_pack(lock, pack, layout) {
        return Ok(engine);
    }
    let spec = InstallSpec {
        id: &pack.id,
        build: &pack.build,
        dir: &pack.dir,
        files: &pack.files,
        keep: &lock.keep_files,
        keep_all: false,
        vc_runtime_files: &lock.vc_runtime_files,
        vc_runtime_dir: vc_runtime_dir(lock, pack, layout)?,
    };
    packs::install_files(&spec, &layout.engines_dir, cancel, progress).await?;
    installed_pack(lock, pack, layout).ok_or_else(|| PackError::Archive {
        file: pack.id.clone(),
        message: "a file the engine needs is not in it".into(),
    })
}

/// Removes the installed engine pack, a half-unpacked one and any partial download. The
/// bundled and development engines are left alone.
pub async fn remove(lock: &SdLock, engines_dir: &Path) -> Result<(), PackError> {
    for pack in &lock.packs {
        packs::remove(pack, engines_dir).await?;
    }
    Ok(())
}

/// The lock's engine pack for `platform` (`studio-vulkan` on Windows x86-64).
fn pack_for<'a>(lock: &'a SdLock, platform: &str) -> Option<&'a PackSpec> {
    lock.packs.iter().find(|pack| pack.platform == platform)
}

/// The engine in `pack`'s folder under `layout.engines_dir`, when that folder is completely
/// installed.
fn installed_pack(lock: &SdLock, pack: &PackSpec, layout: &EngineLayout) -> Option<Engine> {
    let dir = layout.engines_dir.join(&pack.dir);
    is_complete(lock, pack, &dir).then(|| {
        let fallback = vulkan_fallback(layout.llama_dir.as_deref(), &pack.platform);
        Engine::in_dir(&dir, lock, EngineSource::Pack, fallback)
    })
}

/// llama.cpp's bundled Vulkan runtime folder in `llama_dir` (`<llama>/vulkan`), when
/// `platform` is Windows and the folder holds `vulkan-1.dll`. It is the last entry of the
/// engine's `PATH`, as it is of llama-server's
/// ([`path_with_vulkan_fallback`](crate::llama::backend::path_with_vulkan_fallback)).
fn vulkan_fallback(llama_dir: Option<&Path>, platform: &str) -> Option<PathBuf> {
    if !platform.starts_with("windows") {
        return None;
    }
    let folder = llama_dir?.join(VULKAN_FALLBACK_DIR);
    folder.join(VULKAN_RUNTIME_FILE).is_file().then_some(folder)
}

/// The folder `install` copies `pack`'s Visual C++ runtime files from: llama.cpp's
/// installed folder. For a Windows pack that folder must be known and hold every one of the
/// lock's `vc_runtime_files`, or the engine would not start on a PC without the Visual C++
/// redistributable (ruling L1-vcomp); the error comes before anything is downloaded. Off
/// Windows there is no runtime to copy.
fn vc_runtime_dir<'a>(
    lock: &SdLock,
    pack: &PackSpec,
    layout: &'a EngineLayout,
) -> Result<Option<&'a Path>, PackError> {
    if !pack.platform.starts_with("windows") {
        return Ok(None);
    }
    let dir = layout.llama_dir.as_deref().ok_or_else(|| {
        PackError::Io(
            "llama.cpp's folder is unknown, so the engine's Visual C++ runtime files cannot be \
             copied from it"
                .into(),
        )
    })?;
    if let Some(missing) = lock
        .vc_runtime_files
        .iter()
        .find(|name| !dir.join(name).is_file())
    {
        return Err(PackError::Io(format!(
            "{missing} is not in llama.cpp's folder ({})",
            dir.display()
        )));
    }
    Ok(Some(dir))
}

/// Whether the pack folder `dir` holds its `.complete` marker, the program, and every DLL
/// the lock's `windows_imports` names (a recorded file or one of its imports) that the pack
/// keeps. A Windows pack also needs every one of the lock's `vc_runtime_files`, copied from
/// llama.cpp's folder: without them the engine does not start on a PC lacking the Visual
/// C++ redistributable. System DLLs are not checked.
fn is_complete(lock: &SdLock, pack: &PackSpec, dir: &Path) -> bool {
    let needed: BTreeSet<&str> = lock
        .windows_imports
        .iter()
        .flat_map(|(file, imports)| std::iter::once(file).chain(imports))
        .map(String::as_str)
        .filter(|name| name.to_ascii_lowercase().ends_with(".dll") && lock.keeps(name))
        .collect();
    let vc_runtime: &[String] = if pack.platform.starts_with("windows") {
        &lock.vc_runtime_files
    } else {
        &[]
    };
    dir.join(PACK_COMPLETE_MARKER).is_file()
        && dir.join(program_file_name()).is_file()
        && needed.iter().all(|dll| dir.join(dll).is_file())
        && vc_runtime.iter().all(|dll| dir.join(dll).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sd::lock::embedded;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"x").unwrap();
    }

    /// What an install leaves in `<engines>/sd-1106`: every file of the pinned zip the lock
    /// keeps, the Visual C++ runtime files copied from llama's folder, this platform's
    /// program name (for tests off Windows) and the marker.
    fn fake_pack(engines_dir: &Path) -> PathBuf {
        let dir = engines_dir.join("sd-1106");
        let listing = include_str!("../../tests/fixtures/sd_win_pack_listing.txt");
        for name in listing.lines().filter(|n| embedded().keeps(n)) {
            touch(&dir.join(name));
        }
        for name in &embedded().vc_runtime_files {
            touch(&dir.join(name));
        }
        touch(&dir.join(program_file_name()));
        touch(&dir.join(PACK_COMPLETE_MARKER));
        dir
    }

    struct Folders {
        _tmp: tempfile::TempDir,
        layout: EngineLayout,
    }

    /// A dev folder and a bundled folder each holding the program, an installed pack, and
    /// llama's installed folder.
    fn folders() -> Folders {
        let tmp = tempfile::tempdir().unwrap();
        let dev = tmp.path().join("dev");
        let bundled = tmp.path().join("bundled");
        touch(&dev.join(program_file_name()));
        touch(&bundled.join(program_file_name()));
        let engines_dir = tmp.path().join("engines");
        fake_pack(&engines_dir);
        let llama_dir = tmp.path().join("llama").join("main");
        std::fs::create_dir_all(&llama_dir).unwrap();
        let layout = EngineLayout {
            bundled_dir: Some(bundled),
            engines_dir,
            llama_dir: Some(llama_dir),
            dev_override: Some(dev),
        };
        Folders { _tmp: tmp, layout }
    }

    #[test]
    fn program_names() {
        let expected = if cfg!(windows) {
            "sd-cli.exe"
        } else {
            "sd-cli"
        };
        assert_eq!(program_file_name(), expected);
    }

    #[test]
    fn discover_prefers_the_dev_folder_then_the_bundled_program_then_the_pack() {
        let mut f = folders();
        let found = |layout: &EngineLayout| discover(layout, embedded(), "windows-x86_64").unwrap();

        let dev = f.layout.dev_override.clone().unwrap();
        let engine = found(&f.layout);
        assert_eq!(engine.source, EngineSource::Dev);
        assert_eq!(
            (engine.program, engine.dir),
            (dev.join(program_file_name()), dev)
        );

        f.layout.dev_override = None;
        let bundled = f.layout.bundled_dir.clone().unwrap();
        let engine = found(&f.layout);
        assert_eq!(engine.source, EngineSource::Bundled);
        assert_eq!(
            (engine.program, engine.dir),
            (bundled.join(program_file_name()), bundled)
        );

        f.layout.bundled_dir = None;
        let pack = f.layout.engines_dir.join("sd-1106");
        let engine = found(&f.layout);
        assert_eq!(engine.source, EngineSource::Pack);
        assert_eq!(
            (engine.program.clone(), engine.dir.clone()),
            (pack.join(program_file_name()), pack)
        );
        // every source runs the pinned build, with nothing added to its arguments or environment
        assert_eq!(engine.build, 1106);
        assert!(engine.leading_args.is_empty() && engine.extra_env.is_empty());

        // the pack is Windows' engine: another platform does not find it
        assert_eq!(discover(&f.layout, embedded(), "macos-aarch64"), None);
    }

    #[test]
    fn a_pack_whose_program_was_removed_is_not_found() {
        for removed in [program_file_name(), "libwebp.dll"] {
            let mut f = folders();
            f.layout.dev_override = None;
            f.layout.bundled_dir = None;
            assert!(discover(&f.layout, embedded(), "windows-x86_64").is_some());
            assert_eq!(
                pack_state(embedded(), &f.layout.engines_dir),
                PackState::Installed
            );
            // as antivirus quarantine does
            let pack = f.layout.engines_dir.join("sd-1106");
            std::fs::remove_file(pack.join(removed)).unwrap();
            assert!(pack.join(PACK_COMPLETE_MARKER).is_file());
            assert_eq!(
                discover(&f.layout, embedded(), "windows-x86_64"),
                None,
                "{removed}"
            );
            assert_eq!(
                pack_state(embedded(), &f.layout.engines_dir),
                PackState::NotInstalled,
                "{removed}"
            );
        }
    }

    /// The engine starts on a PC without the Visual C++ redistributable only with the
    /// runtime files beside it (ruling L1-vcomp), so a Windows pack missing one is neither
    /// complete nor found, and Studio offers the download again.
    #[test]
    fn a_windows_pack_missing_a_visual_cpp_runtime_file_is_not_complete() {
        let lock = embedded();
        let pack = &lock.packs[0];
        assert!(pack.platform.starts_with("windows"));
        let mut elsewhere = pack.clone();
        elsewhere.platform = "linux-x86_64".into();
        assert!(!lock.vc_runtime_files.is_empty());
        for missing in &lock.vc_runtime_files {
            let tmp = tempfile::tempdir().unwrap();
            let layout = EngineLayout {
                engines_dir: tmp.path().to_path_buf(),
                ..EngineLayout::default()
            };
            let dir = fake_pack(tmp.path());
            assert!(is_complete(lock, pack, &dir), "{missing}");
            assert_eq!(pack_state(lock, tmp.path()), PackState::Installed);
            assert!(discover(&layout, lock, &pack.platform).is_some());

            std::fs::remove_file(dir.join(missing)).unwrap();
            assert!(!is_complete(lock, pack, &dir), "{missing}");
            assert_eq!(
                pack_state(lock, tmp.path()),
                PackState::NotInstalled,
                "{missing}"
            );
            assert_eq!(discover(&layout, lock, &pack.platform), None, "{missing}");
            // the Visual C++ runtime is Windows' own: a pack for another platform needs none
            assert!(is_complete(lock, &elsewhere, &dir), "{missing}");
        }
    }

    /// The Visual C++ runtime files come from llama's folder. For a Windows pack an unknown
    /// folder, or one without a runtime file, is an error before anything is downloaded,
    /// not an engine installed without it.
    #[test]
    fn a_windows_pack_takes_its_visual_cpp_runtime_from_llamas_folder() {
        let lock = embedded();
        let pack = &lock.packs[0];
        let mut f = folders();
        let llama = f.layout.llama_dir.clone().unwrap();
        for name in &lock.vc_runtime_files {
            touch(&llama.join(name));
        }
        assert_eq!(
            vc_runtime_dir(lock, pack, &f.layout).unwrap(),
            Some(llama.as_path())
        );
        for missing in &lock.vc_runtime_files {
            std::fs::remove_file(llama.join(missing)).unwrap();
            let err = vc_runtime_dir(lock, pack, &f.layout).unwrap_err();
            assert!(err.to_string().contains(missing.as_str()), "{err}");
            touch(&llama.join(missing));
        }

        f.layout.llama_dir = None;
        let err = vc_runtime_dir(lock, pack, &f.layout).unwrap_err();
        assert!(err.to_string().contains("llama.cpp"), "{err}");
        // off Windows there is no runtime to copy, and no folder is needed
        let mut elsewhere = pack.clone();
        elsewhere.platform = "linux-x86_64".into();
        assert_eq!(vc_runtime_dir(lock, &elsewhere, &f.layout).unwrap(), None);
    }

    /// On Windows itself, `install` refuses without llama's folder and downloads nothing.
    #[tokio::test]
    async fn on_windows_installing_without_llamas_folder_downloads_nothing() {
        if !cfg!(windows) || current_platform() != "windows-x86_64" {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let layout = EngineLayout {
            engines_dir: tmp.path().join("engines"),
            ..EngineLayout::default()
        };
        let result = install(embedded(), &layout, &CancellationToken::new(), &|_| {}).await;
        assert!(matches!(result, Err(PackError::Io(_))), "{result:?}");
        assert!(!layout.engines_dir.exists());
    }

    #[test]
    fn the_engine_folder_is_never_the_llama_folder() {
        let mut f = folders();
        f.layout.dev_override = None;
        f.layout.bundled_dir = None;
        let engine = discover(&f.layout, embedded(), "windows-x86_64").unwrap();
        assert_eq!(engine.dir, f.layout.engines_dir.join("sd-1106"));
        assert_ne!(Some(&engine.dir), f.layout.llama_dir.as_ref());
    }

    /// The engine carries llama's bundled Vulkan runtime folder, so its `PATH` ends with it
    /// for a job and for `--list-devices` alike (`run::engine_env`).
    #[test]
    fn on_windows_the_engine_records_llamas_vulkan_runtime_folder() {
        let mut f = folders();
        let fallback = f
            .layout
            .llama_dir
            .clone()
            .unwrap()
            .join(VULKAN_FALLBACK_DIR);
        let found = |layout: &EngineLayout, platform| discover(layout, embedded(), platform);

        // llama's folder holds no runtime: nothing to add
        std::fs::create_dir_all(&fallback).unwrap();
        let engine = found(&f.layout, "windows-x86_64").unwrap();
        assert_eq!(engine.vulkan_fallback, None);

        touch(&fallback.join(VULKAN_RUNTIME_FILE));
        let engine = found(&f.layout, "windows-x86_64").unwrap();
        assert_eq!(engine.source, EngineSource::Dev);
        assert_eq!(engine.vulkan_fallback.as_ref(), Some(&fallback));
        // never off Windows
        assert_eq!(
            found(&f.layout, "linux-x86_64").unwrap().vulkan_fallback,
            None
        );
        f.layout.dev_override = None;
        let engine = found(&f.layout, "macos-aarch64").unwrap();
        assert_eq!(engine.source, EngineSource::Bundled);
        assert_eq!(engine.vulkan_fallback, None);

        // the pack, Windows' own engine
        f.layout.bundled_dir = None;
        let engine = found(&f.layout, "windows-x86_64").unwrap();
        assert_eq!(engine.source, EngineSource::Pack);
        assert_eq!(engine.vulkan_fallback.as_ref(), Some(&fallback));
        // llama's folder unknown
        f.layout.llama_dir = None;
        assert_eq!(
            found(&f.layout, "windows-x86_64").unwrap().vulkan_fallback,
            None
        );
    }

    #[tokio::test]
    async fn removing_the_pack_deletes_its_folder_and_its_partial_download() {
        let f = folders();
        let engines_dir = &f.layout.engines_dir;
        let file = &embedded().packs[0].files[0];
        let part = engines_dir
            .join(".downloads")
            .join(crate::packs::part_name(file));
        std::fs::create_dir_all(part.parent().unwrap()).unwrap();
        std::fs::write(&part, b"12345").unwrap();
        assert_eq!(pack_state(embedded(), engines_dir), PackState::Installed);
        std::fs::remove_file(engines_dir.join("sd-1106").join(PACK_COMPLETE_MARKER)).unwrap();
        assert_eq!(
            pack_state(embedded(), engines_dir),
            PackState::Partial { bytes: 5 }
        );
        remove(embedded(), engines_dir).await.unwrap();
        assert!(!engines_dir.join("sd-1106").exists() && !part.exists());
        assert_eq!(pack_state(embedded(), engines_dir), PackState::NotInstalled);
    }

    #[tokio::test]
    async fn installing_the_pack_is_for_windows_only() {
        if cfg!(windows) {
            return;
        }
        let f = folders();
        assert!(matches!(
            install(embedded(), &f.layout, &CancellationToken::new(), &|_| {}).await,
            Err(PackError::WrongPlatform)
        ));
    }
}
