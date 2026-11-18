//! The Studio engine's commands: its state on this computer, and the Windows engine pack's
//! download, cancel and remove (docs/dev/0.1.0/studio-local-design.md, L2), on the pattern of
//! the backend downloads (`packs_cmd.rs`).
//!
//! The engine is built in on a Mac (`sd-cli` beside `llama-server`) and comes from a
//! development folder on Linux (`APP_DEV_SD_DIR`). On Windows it is the `studio-vulkan` pack,
//! downloaded together with the first local model ([`start_engine_download`]) or from its own
//! button ([`studio_engine_download`], once its licence is accepted), one download at a time
//! for the whole app ([`EngineDownloads`]). The download's progress goes to the main window as
//! [`ENGINE_PROGRESS_EVENT`], at most every 300 ms ([`Throttle`]), and its end as one last
//! event with `finished` set and the error, if any, in Studio's words
//! ([`errors::engine_not_installed`]). Each command checks that the main window called it.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use runtime::llama::lock::current_platform;
use runtime::packs::{PackError, PackProgress, PackState};
use runtime::sd::engine::{self, Engine, EngineLayout, EngineSource};
use runtime::sd::lock::{self as sd_lock, SdLock};
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

use super::errors;
use crate::commands::only_main_window;
use crate::dto::{EngineProgressDto, EngineStatusDto};
use crate::error::AppResult;

/// The event that carries the engine pack's download progress to the main window.
pub const ENGINE_PROGRESS_EVENT: &str = "studio-engine-progress";

/// The least time between two progress events of the download.
const PROGRESS_EVERY: Duration = Duration::from_millis(300);

/// The engine pack's download in progress, if any: one at a time for the whole app.
#[derive(Default)]
pub struct EngineDownloads(Mutex<Option<CancellationToken>>);

impl EngineDownloads {
    /// The slot. Nothing panics while holding it, and an `Option` cannot be left half
    /// written, so a poisoned lock is used as it is.
    fn slot(&self) -> MutexGuard<'_, Option<CancellationToken>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Claims the download: the token that cancels it, or `None` when one already runs.
    fn begin(&self) -> Option<CancellationToken> {
        let mut slot = self.slot();
        if slot.is_some() {
            return None;
        }
        let token = CancellationToken::new();
        *slot = Some(token.clone());
        Some(token)
    }

    /// Whether a download runs. A cancelled one runs until its task ends.
    fn running(&self) -> bool {
        self.slot().is_some()
    }

    /// Asks the running download, if any, to stop.
    fn cancel(&self) {
        if let Some(token) = self.slot().as_ref() {
            token.cancel();
        }
    }

    /// The download's task has ended.
    fn end(&self) {
        *self.slot() = None;
    }
}

/// Lets the download's progress through at most every [`PROGRESS_EVERY`], except the one
/// event that says unpacking has begun: no byte count follows it, so it goes at once.
#[derive(Default)]
struct Throttle(Mutex<Option<Instant>>);

impl Throttle {
    /// Whether an event at `now` goes out.
    fn pass(&self, now: Instant, unpacking: bool) -> bool {
        let mut last = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !unpacking && last.is_some_and(|t| now.saturating_duration_since(t) < PROGRESS_EVERY) {
            return false;
        }
        *last = Some(now);
        true
    }
}

/// Where the engine can be on this computer: the app's own folder on macOS (`sd-cli` is
/// bundled beside `llama-server`), `<local data>/engines` for the Windows pack, llama.cpp's
/// installed folder (the pack's Visual C++ runtime files are copied from it, and its Vulkan
/// runtime folder ends the engine's `PATH`) and the development folder named by
/// `APP_DEV_SD_DIR`.
pub fn engine_layout(app: &AppHandle) -> EngineLayout {
    let bundled_dir = if cfg!(target_os = "macos") {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
    } else {
        None
    };
    let engines_dir = app
        .path()
        .app_local_data_dir()
        .map(|d| d.join("engines"))
        .unwrap_or_else(|_| PathBuf::from("engines"));
    EngineLayout::with_env_override(
        bundled_dir,
        engines_dir,
        crate::runtime_manager::layout(app).installed_dir,
    )
}

/// The engine on this computer now, or `None`: Windows before the pack is installed (or after
/// antivirus quarantine took one of its files), Linux without a development folder.
#[allow(
    dead_code,
    reason = "Task L14's local runs find the engine through it, again after a download"
)]
pub fn current_engine(app: &AppHandle) -> Option<Engine> {
    engine::discover(&engine_layout(app), sd_lock::embedded(), current_platform())
}

/// The engine's state for `platform` from what is on disk; `downloading` holds the bytes done
/// while the pack's download runs.
///
/// - `builtIn`: the app's own program (macOS) or a development folder; nothing to download.
/// - `installed`: the pack, complete.
/// - `downloading`, `partial` (part of it downloaded; a new download continues it) and
///   `notInstalled`: the pack, not complete. A pack folder that lost a file to antivirus
///   quarantine is `notInstalled`, so the download is offered again.
/// - `unavailable`: no engine, and no pack for this platform.
///
/// The licence is the engine's whatever its source (the bundled and development programs are
/// built from the pinned commit with the same libraries), so it is the lock's pack licence
/// even where no pack is offered.
pub fn status_from(
    lock: &SdLock,
    layout: &EngineLayout,
    platform: &str,
    downloading: Option<u64>,
) -> EngineStatusDto {
    let pack = lock.packs.iter().find(|p| p.platform == platform);
    let found = engine::discover(layout, lock, platform).map(|e| e.source);
    let (state, bytes_done, offered) = match (found, pack) {
        (Some(EngineSource::Bundled | EngineSource::Dev), _) => ("builtIn", None, None),
        (Some(EngineSource::Pack), pack) => ("installed", None, pack),
        (None, None) => ("unavailable", None, None),
        (None, Some(pack)) => {
            let (state, bytes) = match (downloading, engine::pack_state(lock, &layout.engines_dir))
            {
                (Some(done), _) => ("downloading", Some(done)),
                (None, PackState::Partial { bytes }) => ("partial", Some(bytes)),
                (None, _) => ("notInstalled", None),
            };
            (state, bytes, Some(pack))
        }
    };
    let licensed = pack.or_else(|| lock.packs.first());
    EngineStatusDto {
        state: state.to_owned(),
        build: lock.sd_cpp.tag.clone(),
        download_bytes: offered.map_or(0, |p| p.download_size()),
        license_name: licensed.map(|p| p.license_name.clone()).unwrap_or_default(),
        license_url: licensed.map(|p| p.license_url.clone()).unwrap_or_default(),
        bytes_done,
    }
}

/// The engine's state on this computer now: [`status_from`] with this computer's folders and
/// platform, and the bytes on disk while the pack downloads.
pub fn engine_status(app: &AppHandle) -> EngineStatusDto {
    let lock = sd_lock::embedded();
    let layout = engine_layout(app);
    let downloading = app.state::<EngineDownloads>().running().then(|| {
        match engine::pack_state(lock, &layout.engines_dir) {
            PackState::Partial { bytes } => bytes,
            _ => 0,
        }
    });
    status_from(lock, &layout, current_platform(), downloading)
}

/// Starts the pack download when this platform has a pack and it is not installed; returns at
/// once. Nothing starts on a platform without a pack (macOS, Linux), when the pack is
/// installed, or while its download already runs. How it goes comes as
/// [`ENGINE_PROGRESS_EVENT`]s.
pub async fn start_engine_download(app: &AppHandle) -> AppResult<()> {
    let lock = sd_lock::embedded();
    let layout = engine_layout(app);
    if lock.pack_for_current_platform().is_none()
        || engine::pack_state(lock, &layout.engines_dir) == PackState::Installed
    {
        return Ok(());
    }
    let Some(token) = app.state::<EngineDownloads>().begin() else {
        return Ok(());
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move { run_download(&app, &layout, &token).await });
    Ok(())
}

/// Downloads and installs the pack into `layout`, saying how it goes, then frees the
/// download's place before the last event, so a status read on that event sees the result.
async fn run_download(app: &AppHandle, layout: &EngineLayout, token: &CancellationToken) {
    let lock = sd_lock::embedded();
    let emit = |p: EngineProgressDto| {
        let _ = app.emit_to(crate::MAIN_WINDOW, ENGINE_PROGRESS_EVENT, p);
    };
    let throttle = Throttle::default();
    let progress = |p: PackProgress| {
        if throttle.pass(Instant::now(), p.unpacking) {
            emit(EngineProgressDto {
                done: p.done,
                total: p.total,
                unpacking: p.unpacking,
                finished: false,
                error: None,
            });
        }
    };
    let result = engine::install(lock, layout, token, &progress).await;
    app.state::<EngineDownloads>().end();
    let total = lock
        .pack_for_current_platform()
        .map_or(0, |p| p.download_size());
    let (done, error) = match &result {
        Ok(_) => (total, None),
        Err(e) => {
            let done = match engine::pack_state(lock, &layout.engines_dir) {
                PackState::Partial { bytes } => bytes,
                _ => 0,
            };
            let error = (!matches!(e, PackError::Cancelled)).then(|| {
                tracing::warn!("the Studio engine download failed: {e}");
                errors::engine_not_installed(e)
            });
            (done, error)
        }
    };
    emit(EngineProgressDto {
        done,
        total,
        unpacking: false,
        finished: true,
        error,
    });
}

/// *Download the Studio engine*: refused unless the user accepted its licence; then `start`.
async fn download<F, Fut>(accept_license: bool, start: F) -> AppResult<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = AppResult<()>>,
{
    if !accept_license {
        return Err(errors::engine_licence_not_accepted());
    }
    start().await
}

/// *Remove*: deletes the installed pack in `engines_dir`, a half-unpacked one and any partial
/// download; a bundled or development engine is left alone. Refused while the pack downloads,
/// as its download is still writing there.
async fn remove(downloads: &EngineDownloads, engines_dir: &Path) -> AppResult<()> {
    if downloads.running() {
        return Err(errors::engine_still_downloading());
    }
    engine::remove(sd_lock::embedded(), engines_dir)
        .await
        .map_err(|e| errors::engine_not_removed(e.to_string()))
}

// ---- commands ---------------------------------------------------------------------------------

/// The Studio engine on this computer.
#[tauri::command]
pub async fn studio_engine_status(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> AppResult<EngineStatusDto> {
    only_main_window(window.label())?;
    Ok(engine_status(&app))
}

/// Downloads the Windows engine pack, once the user accepted its licence (`accept_license`);
/// returns at once, and the download says how it goes as [`ENGINE_PROGRESS_EVENT`]s. Starts
/// nothing on a computer that needs no pack, or while a download runs.
#[tauri::command]
pub async fn studio_engine_download(
    window: tauri::WebviewWindow,
    app: AppHandle,
    accept_license: bool,
) -> AppResult<()> {
    only_main_window(window.label())?;
    download(accept_license, || start_engine_download(&app)).await
}

/// Stops the engine pack's download; what it downloaded stays, and a new download continues
/// it.
#[tauri::command]
pub async fn studio_engine_cancel(window: tauri::WebviewWindow, app: AppHandle) -> AppResult<()> {
    only_main_window(window.label())?;
    app.state::<EngineDownloads>().cancel();
    Ok(())
}

/// Removes the Windows engine pack and any partial download of it.
#[tauri::command]
pub async fn studio_engine_remove(window: tauri::WebviewWindow, app: AppHandle) -> AppResult<()> {
    only_main_window(window.label())?;
    remove(
        &app.state::<EngineDownloads>(),
        &engine_layout(&app).engines_dir,
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use runtime::llama::backend::PACK_COMPLETE_MARKER;
    use runtime::sd::engine::{EngineLayout, program_file_name};
    use runtime::sd::lock::{SdLock, embedded};

    use super::*;
    use crate::studio::test_support::Scratch;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"x").unwrap();
    }

    /// A layout whose folders hold nothing: its engines folder is in a scratch folder that is
    /// gone by the time it is read.
    fn empty_layout() -> EngineLayout {
        EngineLayout {
            engines_dir: Scratch::new().path().join("engines"),
            ..EngineLayout::default()
        }
    }

    /// What an install leaves in `<engines>/sd-1106`: every DLL the lock records the engine
    /// importing that the pack keeps, the Visual C++ runtime files, the program and the
    /// marker.
    fn install_pack(lock: &SdLock, engines_dir: &Path) {
        let dir = engines_dir.join(&lock.packs[0].dir);
        let recorded = lock
            .windows_imports
            .iter()
            .flat_map(|(file, imports)| std::iter::once(file).chain(imports));
        for name in recorded.filter(|n| n.to_ascii_lowercase().ends_with(".dll") && lock.keeps(n)) {
            touch(&dir.join(name));
        }
        for name in &lock.vc_runtime_files {
            touch(&dir.join(name));
        }
        touch(&dir.join(program_file_name()));
        touch(&dir.join(PACK_COMPLETE_MARKER));
    }

    /// Part of the pack's zip in `<engines>/.downloads`, named as `runtime::packs` names a
    /// download in progress.
    fn part_of_the_pack(lock: &SdLock, engines_dir: &Path, bytes: &[u8]) {
        let file = &lock.packs[0].files[0];
        let part = format!("{}.{}.part", file.name, &file.sha256[..16]);
        let path = engines_dir.join(".downloads").join(part);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_mac_has_the_engine_built_in() {
        let scratch = Scratch::new();
        let bundled = scratch.path().join("MacOS");
        touch(&bundled.join(program_file_name()));
        let layout = EngineLayout {
            bundled_dir: Some(bundled),
            ..empty_layout()
        };
        let s = status_from(embedded(), &layout, "macos-aarch64", None);
        assert_eq!(s.state, "builtIn");
        assert_eq!(s.download_bytes, 0);
        assert_eq!(s.bytes_done, None);
        assert_eq!(s.build, "master-1106-532d2c1");
        // the engine's licence, the same whatever its source
        assert_eq!(s.license_name, embedded().packs[0].license_name);
        // without the bundled program, a Mac has no engine and nothing to download
        let s = status_from(embedded(), &empty_layout(), "macos-aarch64", None);
        assert_eq!((s.state.as_str(), s.download_bytes), ("unavailable", 0));
    }

    #[test]
    fn windows_without_the_pack_offers_its_29_mb_download() {
        let s = status_from(embedded(), &empty_layout(), "windows-x86_64", None);
        assert_eq!(s.state, "notInstalled");
        assert_eq!(s.download_bytes, embedded().packs[0].download_size());
        assert_eq!(s.build, "master-1106-532d2c1");
        assert_eq!(s.bytes_done, None);
        assert_eq!(s.license_name, embedded().packs[0].license_name);
        assert_eq!(s.license_url, embedded().packs[0].license_url);
        // about 29 MB, as the download button says
        assert_eq!(s.download_bytes.div_ceil(1024 * 1024), 29);
    }

    #[test]
    fn linux_without_a_dev_engine_says_unavailable() {
        assert_eq!(
            status_from(embedded(), &empty_layout(), "linux-x86_64", None).state,
            "unavailable"
        );
    }

    #[test]
    fn a_dev_engine_is_built_in_wherever_it_is() {
        let scratch = Scratch::new();
        let dev = scratch.path().join("sd");
        touch(&dev.join(program_file_name()));
        let layout = EngineLayout {
            dev_override: Some(dev),
            ..empty_layout()
        };
        for platform in ["linux-x86_64", "windows-x86_64", "macos-aarch64"] {
            let s = status_from(embedded(), &layout, platform, None);
            assert_eq!(
                (s.state.as_str(), s.download_bytes),
                ("builtIn", 0),
                "{platform}"
            );
        }
    }

    #[test]
    fn windows_shows_the_pack_partly_downloaded_downloading_and_installed() {
        let lock = embedded();
        let scratch = Scratch::new();
        let layout = EngineLayout {
            engines_dir: scratch.path().join("engines"),
            ..EngineLayout::default()
        };
        let size = lock.packs[0].download_size();

        part_of_the_pack(lock, &layout.engines_dir, b"12345");
        let s = status_from(lock, &layout, "windows-x86_64", None);
        assert_eq!(
            (s.state.as_str(), s.bytes_done, s.download_bytes),
            ("partial", Some(5), size)
        );

        // a download running says so, with the bytes it has done
        let s = status_from(lock, &layout, "windows-x86_64", Some(1_000_000));
        assert_eq!(
            (s.state.as_str(), s.bytes_done, s.download_bytes),
            ("downloading", Some(1_000_000), size)
        );

        install_pack(lock, &layout.engines_dir);
        // an install deletes its download
        std::fs::remove_dir_all(layout.engines_dir.join(".downloads")).unwrap();
        for downloading in [None, Some(size)] {
            let s = status_from(lock, &layout, "windows-x86_64", downloading);
            assert_eq!(
                (s.state.as_str(), s.bytes_done, s.download_bytes),
                ("installed", None, size),
                "{downloading:?}"
            );
        }
        // the pack is Windows' engine: a Linux computer does not run it
        assert_eq!(
            status_from(lock, &layout, "linux-x86_64", None).state,
            "unavailable"
        );

        // antivirus quarantine removed the program: the download is offered again
        std::fs::remove_file(
            layout
                .engines_dir
                .join(&lock.packs[0].dir)
                .join(program_file_name()),
        )
        .unwrap();
        assert_eq!(
            status_from(lock, &layout, "windows-x86_64", None).state,
            "notInstalled"
        );
    }

    #[tokio::test]
    async fn the_download_needs_the_licence_accepted() {
        let started = std::sync::atomic::AtomicUsize::new(0);
        let start = || async {
            started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        };
        let err = download(false, start).await.unwrap_err();
        assert_eq!(err.code, "bad_request");
        assert_eq!(
            err.message,
            "Accept the licence to download the Studio engine."
        );
        assert_eq!(started.load(std::sync::atomic::Ordering::SeqCst), 0);

        download(true, start).await.unwrap();
        assert_eq!(started.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn one_download_at_a_time() {
        let downloads = EngineDownloads::default();
        assert!(!downloads.running());
        let token = downloads.begin().expect("the first download starts");
        assert!(downloads.running());
        // a second press while it runs starts nothing
        assert!(downloads.begin().is_none());

        downloads.cancel();
        assert!(token.is_cancelled());
        // still running until its task ends
        assert!(downloads.running());
        downloads.end();
        assert!(!downloads.running());
        let again = downloads.begin().expect("a new download starts");
        assert!(!again.is_cancelled());
        // cancelling with nothing running changes nothing
        downloads.end();
        downloads.cancel();
        assert!(!downloads.running());
    }

    #[test]
    fn progress_goes_out_every_300_ms_at_most() {
        let throttle = Throttle::default();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        assert!(throttle.pass(at(0), false));
        assert!(!throttle.pass(at(100), false));
        assert!(!throttle.pass(at(299), false));
        assert!(throttle.pass(at(300), false));
        assert!(!throttle.pass(at(450), false));
        // unpacking begins: said at once, so the bar does not sit at 100 % unexplained
        assert!(throttle.pass(at(460), true));
        assert!(!throttle.pass(at(500), false));
    }

    #[tokio::test]
    async fn removing_takes_the_pack_and_its_partial_download() {
        let lock = embedded();
        let scratch = Scratch::new();
        let engines_dir = scratch.path().join("engines");
        install_pack(lock, &engines_dir);
        part_of_the_pack(lock, &engines_dir, b"123");
        let layout = EngineLayout {
            engines_dir: engines_dir.clone(),
            ..EngineLayout::default()
        };
        assert_eq!(
            status_from(lock, &layout, "windows-x86_64", None).state,
            "installed"
        );

        // not while it downloads: the download is still writing there
        let downloads = EngineDownloads::default();
        let _token = downloads.begin().unwrap();
        let err = remove(&downloads, &engines_dir).await.unwrap_err();
        assert_eq!(err.message, "The Studio engine is still downloading.");
        assert!(engines_dir.join(&lock.packs[0].dir).is_dir());
        downloads.end();

        remove(&downloads, &engines_dir).await.unwrap();
        assert!(!engines_dir.join(&lock.packs[0].dir).exists());
        let s = status_from(lock, &layout, "windows-x86_64", None);
        assert_eq!((s.state.as_str(), s.bytes_done), ("notInstalled", None));
        // nothing left to remove is not an error
        remove(&downloads, &engines_dir).await.unwrap();
    }
}
