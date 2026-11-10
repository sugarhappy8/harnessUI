//! Settings → Local models → Backend downloads: CUDA and ROCm as one-click,
//! verified downloads (decisions D1, D2, D4; ARCHITECTURE.md 7.1).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use runtime::packs::{PackError, PackState};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Downloads in progress, by pack id.
#[derive(Default)]
pub struct PackDownloads(Mutex<HashMap<String, CancellationToken>>);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackDto {
    pub id: String,
    pub title: String,
    pub backend: String,
    pub requirements: String,
    pub license_name: String,
    pub license_url: String,
    pub download_bytes: u64,
    /// "notInstalled", "partial", "downloading", "installed".
    pub state: String,
    pub bytes_done: u64,
    /// False when this computer cannot use the pack; `reason` says why.
    pub available: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackProgressDto {
    pub id: String,
    pub done: u64,
    pub total: u64,
    pub unpacking: bool,
    pub finished: bool,
    pub error: Option<AppError>,
}

fn packs_dir(app: &AppHandle) -> PathBuf {
    crate::runtime_manager::layout(app).packs_dir
}

/// Windows build number (26200 is Windows 11 25H2), from the registry.
#[cfg(windows)]
fn windows_build() -> Option<u32> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::w;
    let mut buf = [0u16; 32];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: the buffer and its byte length describe writable memory; the key and value
    // names are static wide strings.
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if rc.is_err() {
        return None;
    }
    let s = String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]);
    s.trim().parse().ok()
}

/// Whether this computer can use the pack, with the reason when not.
async fn availability(state: &AppState, pack: &runtime::PackSpec) -> (bool, Option<String>) {
    match pack.backend {
        runtime::BackendKind::Rocm => {
            #[cfg(windows)]
            {
                if let Some(build) = windows_build()
                    && build < 26200
                {
                    return (
                        false,
                        Some(format!(
                            "ROCm needs Windows 11 25H2 (build 26200); this PC runs build {build}."
                        )),
                    );
                }
            }
            let gpus: Vec<String> = state
                .runtime
                .backends(false)
                .await
                .into_iter()
                .flat_map(|(_, p)| p.devices.into_iter().map(|d| d.description))
                .collect();
            let rdna4 = gpus.iter().any(|d| {
                let d = d.to_ascii_lowercase();
                d.contains("rx 90") || d.contains("gfx1200") || d.contains("gfx1201")
            });
            if gpus.is_empty() {
                (true, Some("The GPU could not be identified; this build supports only RX 9000-series cards.".into()))
            } else if rdna4 {
                (true, None)
            } else {
                (
                    false,
                    Some("This ROCm build supports only AMD Radeon RX 9000-series cards.".into()),
                )
            }
        }
        runtime::BackendKind::Cuda => {
            let gpus: Vec<String> = state
                .runtime
                .backends(false)
                .await
                .into_iter()
                .flat_map(|(_, p)| p.devices.into_iter().map(|d| d.description))
                .collect();
            if gpus.iter().any(|d| {
                d.to_ascii_lowercase().contains("nvidia")
                    || d.contains("GeForce")
                    || d.contains("RTX")
            }) {
                (true, None)
            } else if gpus.is_empty() {
                (
                    true,
                    Some("The GPU could not be identified; CUDA needs an NVIDIA card.".into()),
                )
            } else {
                (false, Some("CUDA needs an NVIDIA graphics card.".into()))
            }
        }
        _ => (true, None),
    }
}

#[tauri::command]
pub async fn list_packs(app: AppHandle, state: State<'_, AppState>) -> AppResult<Vec<PackDto>> {
    let lock = runtime::llama::lock::embedded();
    let dir = packs_dir(&app);
    let running = app.state::<PackDownloads>();
    let mut out = Vec::new();
    for pack in lock.packs_for_current_platform() {
        let downloading = running
            .0
            .lock()
            .map(|m| m.contains_key(&pack.id))
            .unwrap_or(false);
        let (state_name, bytes) = match runtime::packs::state(pack, &dir) {
            PackState::Installed => ("installed", pack.download_size()),
            _ if downloading => ("downloading", 0),
            PackState::Partial { bytes } => ("partial", bytes),
            PackState::NotInstalled => ("notInstalled", 0),
        };
        let (available, reason) = availability(&state, pack).await;
        out.push(PackDto {
            id: pack.id.clone(),
            title: pack.title.clone(),
            backend: pack.backend.as_str().to_string(),
            requirements: pack.requirements.clone(),
            license_name: pack.license_name.clone(),
            license_url: pack.license_url.clone(),
            download_bytes: pack.download_size(),
            state: state_name.to_string(),
            bytes_done: bytes,
            available,
            reason,
        });
    }
    Ok(out)
}

fn pack_error(e: &PackError) -> AppError {
    match e {
        PackError::PathTooLong(_) => AppError::new(
            "path_too_long",
            "The app's data folder path is too long for ROCm.",
            "Use a shorter Windows user name or move the app data folder; see the ROCm notes in the README.",
            e.to_string(),
        ),
        PackError::ChecksumMismatch { .. } | PackError::SizeMismatch { .. } => AppError::new(
            "checksum_mismatch",
            "The downloaded backend does not match its pinned checksum, so it was not installed.",
            "Try again. If it keeps failing, report it: the published file may have changed.",
            e.to_string(),
        ),
        PackError::Network { .. } => AppError::new(
            "network",
            "The backend download was interrupted.",
            "Check your connection and try again; it continues where it stopped.",
            e.to_string(),
        ),
        PackError::DiskFull => AppError::new(
            "disk_full",
            "The disk is full.",
            "Free some space and try again.",
            e.to_string(),
        ),
        PackError::BlockedHost(_) => AppError::new(
            "blocked_host",
            "The download was redirected to a host the app does not trust.",
            "Report this; GitHub may have changed its download servers.",
            e.to_string(),
        ),
        _ => AppError::new(
            "pack_failed",
            "The backend could not be installed.",
            "Try again, or copy the details into a bug report.",
            e.to_string(),
        ),
    }
}

/// Starts a download. The UI shows the license first; `accept_license` records the
/// user's consent and the download is refused without it.
#[tauri::command]
pub async fn download_pack(app: AppHandle, id: String, accept_license: bool) -> AppResult<()> {
    if !accept_license {
        return Err(AppError::bad_request(
            "The backend's license must be accepted before it is downloaded.",
        ));
    }
    let lock = runtime::llama::lock::embedded();
    let pack = lock
        .pack(&id)
        .ok_or_else(|| AppError::not_found("backend"))?
        .clone();
    let token = CancellationToken::new();
    {
        let running = app.state::<PackDownloads>();
        let mut map = running.0.lock().map_err(AppError::internal)?;
        if map.contains_key(&id) {
            return Ok(());
        }
        map.insert(id.clone(), token.clone());
    }
    let layout = crate::runtime_manager::layout(&app);
    let vc_dir = layout.installed_dir.clone();
    let dir = layout.packs_dir.clone();
    tauri::async_runtime::spawn(async move {
        let emit = |p: PackProgressDto| {
            let _ = app.emit_to(crate::MAIN_WINDOW, "pack-progress", p);
        };
        let last = Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(1));
        let progress = |p: runtime::packs::PackProgress| {
            let Ok(mut t) = last.lock() else { return };
            if t.elapsed() < std::time::Duration::from_millis(300) && !p.unpacking {
                return;
            }
            *t = std::time::Instant::now();
            emit(PackProgressDto {
                id: pack.id.clone(),
                done: p.done,
                total: p.total,
                unpacking: p.unpacking,
                finished: false,
                error: None,
            });
        };
        let result =
            runtime::packs::install(lock, &pack, &dir, vc_dir.as_deref(), &token, &progress).await;
        if let Ok(mut map) = app.state::<PackDownloads>().0.lock() {
            map.remove(&pack.id);
        }
        let error = match &result {
            Ok(_) => None,
            Err(PackError::Cancelled) => None,
            Err(e) => {
                tracing::warn!(pack = %pack.id, "backend download failed: {e}");
                Some(pack_error(e))
            }
        };
        if result.is_ok() {
            // The new backend shows up in the next probe.
            let state = app.state::<AppState>();
            let _ = state.runtime.backends(true).await;
        }
        emit(PackProgressDto {
            id: pack.id.clone(),
            done: 0,
            total: pack.download_size(),
            unpacking: false,
            finished: true,
            error,
        });
    });
    Ok(())
}

#[tauri::command]
pub async fn cancel_pack_download(app: AppHandle, id: String) -> AppResult<()> {
    if let Ok(map) = app.state::<PackDownloads>().0.lock()
        && let Some(t) = map.get(&id)
    {
        t.cancel();
    }
    Ok(())
}

#[tauri::command]
pub async fn remove_pack(app: AppHandle, state: State<'_, AppState>, id: String) -> AppResult<()> {
    let lock = runtime::llama::lock::embedded();
    let pack = lock
        .pack(&id)
        .ok_or_else(|| AppError::not_found("backend"))?;
    // Stop the model first if it runs on this pack.
    if state.runtime.status().backend.as_deref() == Some(pack.backend.as_str()) {
        state.runtime.unload().await;
    }
    runtime::packs::remove(pack, &packs_dir(&app))
        .await
        .map_err(|e| pack_error(&e))?;
    let _ = state.runtime.backends(true).await;
    Ok(())
}
