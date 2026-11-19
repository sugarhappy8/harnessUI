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

use models::gen_catalog::{Commercial, GenLicence};
use runtime::llama::lock::{PackSpec, current_platform};
use runtime::packs::{PackError, PackProgress, PackState};
use runtime::sd::engine::{self, Engine, EngineLayout, EngineSource};
use runtime::sd::lock::{self as sd_lock, SdLock};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

use super::errors;
use super::local::{EngineReplace, EngineUse};
use super::manager::StudioManager;
use crate::commands::only_main_window;
use crate::dto::{EngineProgressDto, EngineStatusDto};
use crate::error::AppResult;

/// The event that carries the engine pack's download progress to the main window.
pub const ENGINE_PROGRESS_EVENT: &str = "studio-engine-progress";

/// The least time between two progress events of the download.
const PROGRESS_EVERY: Duration = Duration::from_millis(300);

/// The engine pack's download in progress, if any: one at a time for the whole app. A removal
/// holds the same slot while it runs ([`EngineDownloads::hold`]).
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

    /// Claims the slot for a removal, so no download starts while the removal deletes the
    /// folders it would write to; freed when the claim is dropped. `None` when a download (or
    /// another removal) holds it.
    fn hold(&self) -> Option<Claim<'_>> {
        self.begin().map(|_| Claim(self))
    }

    /// Whether a download runs. A cancelled one runs until its task ends. A removal holding
    /// the slot counts too.
    fn running(&self) -> bool {
        self.slot().is_some()
    }

    /// Asks the running download, if any, to stop.
    fn cancel(&self) {
        if let Some(token) = self.slot().as_ref() {
            token.cancel();
        }
    }

    /// The download's task (or the removal) has ended.
    fn end(&self) {
        *self.slot() = None;
    }
}

/// Frees the download's slot when dropped: at the end of the download's task or of a
/// removal, however it ends, a panic included, so the status never stays `downloading`.
struct Claim<'a>(&'a EngineDownloads);

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.0.end();
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
pub fn current_engine(app: &AppHandle) -> Option<Engine> {
    engine::discover(&engine_layout(app), sd_lock::embedded(), current_platform())
}

/// The id of the Studio engine's licence among a local model's licences.
pub const ENGINE_LICENCE_ID: &str = "studio-engine";

/// What a local model's download needs of the Studio engine.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineNeed {
    /// An engine is found: the app's own program, a development folder or the installed pack.
    Found,
    /// No engine yet, and this platform has a pack: it downloads with the model, under this
    /// licence, accepted with the model's.
    Pack(GenLicence),
    /// No engine, and no pack for this platform: a Mac whose bundled program is missing, Linux
    /// without a development folder.
    Missing,
}

/// The Studio engine's licence as a local model's download lists it for acceptance: the
/// pack's licence name and link from `sd.lock.json` (what [`status_from`] shows beside
/// *Download the Studio engine*). The pack bundles no canonical text to hash, so its SHA-256 is
/// that of `<pack id>@<build>:<licence name> <licence link>`, as a searched repository's own
/// label is pinned to its revision (`gen_search`): an acceptance holds for this build and
/// this licence, and a new build or licence asks again.
pub fn engine_licence(pack: &PackSpec) -> GenLicence {
    let pinned = format!(
        "{}@{}:{} {}",
        pack.id, pack.build, pack.license_name, pack.license_url
    );
    GenLicence {
        id: ENGINE_LICENCE_ID.to_owned(),
        name: pack.license_name.clone(),
        url: pack.license_url.clone(),
        sha256: Sha256::digest(pinned.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        commercial_outputs: Commercial::Yes,
        revenue_limit_usd: None,
        excluded_regions: Vec::new(),
        obligations: Vec::new(),
        plain_words: errors::ENGINE_LICENCE_WORDS.to_owned(),
    }
}

/// What a local model's download on `platform` needs of the engine: nothing when one is found
/// (built in, a development folder or the installed pack), else the pack where `platform` has
/// one, else nothing it can download.
pub fn engine_need_from(lock: &SdLock, layout: &EngineLayout, platform: &str) -> EngineNeed {
    if engine::discover(layout, lock, platform).is_some() {
        return EngineNeed::Found;
    }
    match lock.packs.iter().find(|p| p.platform == platform) {
        Some(pack) => EngineNeed::Pack(engine_licence(pack)),
        None => EngineNeed::Missing,
    }
}

/// [`engine_need_from`] for this computer now.
pub fn engine_need(app: &AppHandle) -> EngineNeed {
    engine_need_from(sd_lock::embedded(), &engine_layout(app), current_platform())
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

/// Whether `platform` needs the pack downloaded: it has one, and no engine is found yet (a
/// development folder, the bundled program or the installed pack).
fn needs_the_pack(lock: &SdLock, layout: &EngineLayout, platform: &str) -> bool {
    matches!(
        engine_need_from(lock, layout, platform),
        EngineNeed::Pack(_)
    )
}

/// The count of the Studio engine's processes the app runs (its runs and device listings).
fn engine_use(app: &AppHandle) -> EngineUse {
    app.try_state::<EngineUse>()
        .map(|state| state.inner().clone())
        .unwrap_or_default()
}

/// The engine's folder for a pack download that just claimed the download's slot: its
/// install replaces that folder, so it is refused while an engine process runs from it (a run
/// with an engine found earlier, whose pack has since lost a file, or a device listing), and
/// the slot is freed again. Held for the whole download, so no engine process starts from the
/// folder meanwhile.
fn claim_for_install(downloads: &EngineDownloads, engine: &EngineUse) -> AppResult<EngineReplace> {
    engine.replace().ok_or_else(|| {
        downloads.end();
        errors::engine_in_use()
    })
}

/// Starts the pack download when this platform has a pack and has no engine yet; returns at
/// once. Nothing starts on a platform without a pack (macOS, Linux), when an engine is found
/// (the pack installed, or a development folder on a Windows development machine), or while
/// its download already runs or a removal holds its slot; it is refused while an engine process
/// runs (`studio_engine_in_use`), as its install replaces the engine's folder. How it goes
/// comes as [`ENGINE_PROGRESS_EVENT`]s.
pub async fn start_engine_download(app: &AppHandle) -> AppResult<()> {
    let lock = sd_lock::embedded();
    let layout = engine_layout(app);
    if !needs_the_pack(lock, &layout, current_platform()) {
        return Ok(());
    }
    let downloads = app.state::<EngineDownloads>();
    let Some(token) = downloads.begin() else {
        return Ok(());
    };
    let folder = claim_for_install(&downloads, &engine_use(app))?;
    let app = app.clone();
    tauri::async_runtime::spawn(async move { run_download(&app, &layout, &token, folder).await });
    Ok(())
}

/// Downloads and installs the pack into `layout`, saying how it goes, then frees the
/// download's place (and the engine's folder, `folder`) before the last event, so a status
/// read on that event sees the result. [`start_engine_download`] claimed both; a [`Claim`]
/// frees the place however the task ends, so a panic in the install does not leave the status
/// `downloading`.
async fn run_download(
    app: &AppHandle,
    layout: &EngineLayout,
    token: &CancellationToken,
    folder: EngineReplace,
) {
    let claim = Claim(app.state::<EngineDownloads>().inner());
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
    drop(folder);
    drop(claim);
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

/// *Remove*: `delete` takes the installed pack, a half-unpacked one and any partial download
/// (`engine::remove`; a bundled or development engine is left alone). Refused while the pack
/// downloads, as its download is still writing there, and while a local job works
/// (`local_running`) or any engine process runs (`engine`: a run, or the one-off device
/// listing a first estimate or command starts, which can take 30 s), as the engine runs from
/// it. The removal holds the download's slot and the engine's folder until it ends, so no
/// download (from its button, or with a first local model) starts writing into the folders it
/// deletes, and no engine process starts from them.
async fn remove<F, Fut>(
    downloads: &EngineDownloads,
    engine: &EngineUse,
    local_running: bool,
    delete: F,
) -> AppResult<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), PackError>>,
{
    if local_running {
        return Err(errors::engine_in_use());
    }
    let Some(_claim) = downloads.hold() else {
        return Err(errors::engine_still_downloading());
    };
    let Some(_folder) = engine.replace() else {
        return Err(errors::engine_in_use());
    };
    delete()
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
    let engines_dir = engine_layout(&app).engines_dir;
    let local_running = app
        .try_state::<std::sync::Arc<StudioManager>>()
        .is_some_and(|studio| studio.local_running());
    remove(
        &app.state::<EngineDownloads>(),
        &engine_use(&app),
        local_running,
        || engine::remove(sd_lock::embedded(), &engines_dir),
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
    async fn the_slot_is_freed_however_the_download_ends() {
        let downloads = std::sync::Arc::new(EngineDownloads::default());

        // the install panics inside the spawned task: its claim frees the slot as it unwinds
        let _token = downloads.begin().unwrap();
        let task = tokio::spawn({
            let downloads = downloads.clone();
            async move {
                let _claim = Claim(&downloads);
                tokio::task::yield_now().await;
                panic!("the install panicked");
            }
        });
        assert!(task.await.unwrap_err().is_panic());
        assert!(!downloads.running(), "not left downloading");
        assert!(downloads.begin().is_some(), "a new download starts");
        downloads.end();

        // the install returns: dropping the claim frees the slot (before the last event)
        let _token = downloads.begin().unwrap();
        let claim = Claim(&downloads);
        assert!(downloads.running());
        drop(claim);
        assert!(!downloads.running());
    }

    #[test]
    fn the_pack_downloads_only_where_no_engine_is_found() {
        let lock = embedded();
        let scratch = Scratch::new();
        let layout = EngineLayout {
            engines_dir: scratch.path().join("engines"),
            ..EngineLayout::default()
        };
        assert!(needs_the_pack(lock, &layout, "windows-x86_64"));
        // no pack for these platforms: nothing to download
        assert!(!needs_the_pack(lock, &layout, "linux-x86_64"));
        assert!(!needs_the_pack(lock, &layout, "macos-aarch64"));

        // a Windows development machine with APP_DEV_SD_DIR has its engine already
        let dev = scratch.path().join("sd");
        touch(&dev.join(program_file_name()));
        let with_dev = EngineLayout {
            dev_override: Some(dev),
            ..layout.clone()
        };
        assert_eq!(
            status_from(lock, &with_dev, "windows-x86_64", None).state,
            "builtIn"
        );
        assert!(!needs_the_pack(lock, &with_dev, "windows-x86_64"));

        // the pack installed
        install_pack(lock, &layout.engines_dir);
        assert!(!needs_the_pack(lock, &layout, "windows-x86_64"));
    }

    /// A local model's download brings the engine only where it is a pack and is not found;
    /// with no engine and no pack it can only say the engine is missing.
    #[test]
    fn a_model_download_needs_the_engine_only_where_a_pack_can_bring_it() {
        let lock = embedded();
        let scratch = Scratch::new();
        let layout = EngineLayout {
            engines_dir: scratch.path().join("engines"),
            ..EngineLayout::default()
        };
        let EngineNeed::Pack(licence) = engine_need_from(lock, &layout, "windows-x86_64") else {
            panic!("Windows without the pack downloads it");
        };
        assert_eq!(licence, engine_licence(&lock.packs[0]));
        // the licence status_from shows beside the engine's own download
        let shown = status_from(lock, &layout, "windows-x86_64", None);
        assert_eq!(
            (licence.name.as_str(), licence.url.as_str()),
            (shown.license_name.as_str(), shown.license_url.as_str())
        );
        assert_eq!(licence.id, ENGINE_LICENCE_ID);
        assert_eq!(licence.commercial_outputs, Commercial::Yes);
        assert_eq!(
            licence.plain_words,
            "Free for any use, including commercial."
        );

        // no pack for these platforms: a Mac without its bundled program, Linux without a
        // development folder
        for platform in ["linux-x86_64", "macos-aarch64", "macos-x86_64"] {
            assert_eq!(
                engine_need_from(lock, &layout, platform),
                EngineNeed::Missing,
                "{platform}"
            );
        }

        // found: a Mac's bundled program, a development folder, the installed pack
        let bundled = scratch.path().join("MacOS");
        touch(&bundled.join(program_file_name()));
        let mac = EngineLayout {
            bundled_dir: Some(bundled),
            ..layout.clone()
        };
        assert_eq!(
            engine_need_from(lock, &mac, "macos-aarch64"),
            EngineNeed::Found
        );
        let dev = scratch.path().join("sd");
        touch(&dev.join(program_file_name()));
        let with_dev = EngineLayout {
            dev_override: Some(dev),
            ..layout.clone()
        };
        for platform in ["linux-x86_64", "windows-x86_64"] {
            assert_eq!(
                engine_need_from(lock, &with_dev, platform),
                EngineNeed::Found,
                "{platform}"
            );
        }
        install_pack(lock, &layout.engines_dir);
        assert_eq!(
            engine_need_from(lock, &layout, "windows-x86_64"),
            EngineNeed::Found
        );
    }

    /// The engine's licence is pinned to its build and its licence: either changing asks
    /// for it again.
    #[test]
    fn the_engine_licence_is_pinned_to_its_build_and_licence() {
        let pack = embedded().packs[0].clone();
        let licence = engine_licence(&pack);
        assert_eq!(licence.sha256.len(), 64);
        assert!(licence.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(
            engine_licence(&pack.clone()),
            licence,
            "the same every time"
        );
        let other_build = PackSpec {
            build: "master-1107-0000000".into(),
            ..pack.clone()
        };
        let other_licence = PackSpec {
            license_name: format!("{}; another library MIT", pack.license_name),
            ..pack.clone()
        };
        let other_link = PackSpec {
            license_url: format!("{}#another", pack.license_url),
            ..pack
        };
        for changed in [other_build, other_licence, other_link] {
            assert_ne!(engine_licence(&changed).sha256, licence.sha256);
        }
    }

    #[tokio::test]
    async fn a_removal_holds_the_slot_so_no_download_starts_meanwhile() {
        let downloads = EngineDownloads::default();
        let engine = EngineUse::default();
        let tried = std::sync::atomic::AtomicBool::new(false);
        remove(&downloads, &engine, false, || async {
            // what start_engine_download asks before it starts: the slot is taken
            assert!(downloads.running());
            assert!(downloads.begin().is_none(), "no download starts");
            assert!(engine.process().is_none(), "no engine process starts");
            tried.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap();
        assert!(tried.load(std::sync::atomic::Ordering::SeqCst));
        // freed when the removal ends
        assert!(!downloads.running());

        // and when it fails
        let err = remove(&downloads, &engine, false, || async {
            assert!(downloads.begin().is_none(), "no download starts");
            Err(PackError::Cancelled)
        })
        .await
        .unwrap_err();
        assert_eq!(err.message, "The Studio engine could not be removed.");
        assert!(!downloads.running());
        assert!(
            engine.process().is_some(),
            "an engine process starts afterwards"
        );
        assert!(downloads.begin().is_some(), "a download starts afterwards");
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
        let delete = || engine::remove(lock, &engines_dir);
        let processes = EngineUse::default();

        // not while it downloads: the download is still writing there
        let downloads = EngineDownloads::default();
        let token = downloads.begin().unwrap();
        let err = remove(&downloads, &processes, false, delete)
            .await
            .unwrap_err();
        assert_eq!(err.message, "The Studio engine is still downloading.");
        assert!(engines_dir.join(&lock.packs[0].dir).is_dir());
        // the refusal leaves the download's claim alone
        assert!(downloads.running());
        assert!(!token.is_cancelled());
        downloads.end();

        // not while a local job runs: the engine is working from it
        let err = remove(&downloads, &processes, true, delete)
            .await
            .unwrap_err();
        assert_eq!(err.code, "studio_engine_in_use");
        assert_eq!(
            err.next_step,
            "Stop it in Studio, or remove the engine when it finishes."
        );
        assert!(engines_dir.join(&lock.packs[0].dir).is_dir());
        assert!(!downloads.running(), "the refusal holds no slot");

        // nor while an engine process runs from it with no local job: the one-off device
        // listing a first estimate starts (finding local-engine-folder-deleted-while-running)
        let listing = processes.process().unwrap();
        let err = remove(&downloads, &processes, false, delete)
            .await
            .unwrap_err();
        assert_eq!(err.code, "studio_engine_in_use");
        assert!(engines_dir.join(&lock.packs[0].dir).is_dir());
        assert!(!downloads.running(), "the refusal holds no slot");
        drop(listing);

        remove(&downloads, &processes, false, delete).await.unwrap();
        assert!(!engines_dir.join(&lock.packs[0].dir).exists());
        let s = status_from(lock, &layout, "windows-x86_64", None);
        assert_eq!((s.state.as_str(), s.bytes_done), ("notInstalled", None));
        // nothing left to remove is not an error
        remove(&downloads, &processes, false, delete).await.unwrap();
        assert!(!downloads.running());
    }

    /// Finding local-engine-folder-deleted-while-running: a pack download's install replaces
    /// the engine's folder, so it is refused while an engine process runs from it (a run with
    /// an engine found earlier, whose pack has since lost a file), freeing the download's slot
    /// again; once claimed, the folder lets no engine process start until the install ends.
    #[test]
    fn a_pack_install_waits_for_no_engine_process_and_starts_none() {
        let downloads = EngineDownloads::default();
        let engine = EngineUse::default();
        let run = engine.process().unwrap();
        assert!(downloads.begin().is_some());
        let err = claim_for_install(&downloads, &engine).err().unwrap();
        assert_eq!(err.code, "studio_engine_in_use");
        assert!(!downloads.running(), "the slot is freed again");
        drop(run);

        assert!(downloads.begin().is_some());
        let folder = claim_for_install(&downloads, &engine).unwrap();
        assert!(
            engine.process().is_none(),
            "no engine process starts meanwhile"
        );
        drop(folder);
        assert!(engine.process().is_some());
    }
}
