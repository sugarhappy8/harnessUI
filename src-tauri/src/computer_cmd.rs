//! Computer use in the app (ARCHITECTURE.md 9.3 "Computer use", 13.3, D7): the native
//! server on Windows and macOS, the indicator while it acts, per-model coordinate
//! profiles, the macOS permission checks, and Stop.
//!
//! The tools are offered only for models that can see images, and only once the model's
//! coordinate convention is known: verified by the catalog or set by the user after the
//! calibration test (9.3, "Model conventions").

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The computer server, when this OS has one.
#[derive(Default)]
pub struct ComputerHandle(pub Option<Arc<computer::ComputerServer>>);

/// Setting key prefix for a model's profile: `computerProfile:<model ref>`.
fn profile_key(model_ref: &str) -> String {
    format!("computerProfile:{model_ref}")
}

/// A model's computer-use profile and whether the user confirmed it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredProfile {
    pub profile: computer::ModelProfile,
    /// Set by the user after the calibration test (or for a catalog-verified model).
    pub confirmed: bool,
}

/// The profile for a model, if computer use is enabled for it.
pub async fn profile_for(state: &AppState, model_ref: &str) -> Option<StoredProfile> {
    state
        .store
        .get_setting(&profile_key(model_ref))
        .await
        .ok()
        .flatten()
}

/// Runs closures on the app's main thread (enigo and AppKit need it).
#[cfg(any(target_os = "windows", target_os = "macos"))]
struct TauriMainThread(AppHandle);

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl computer::MainThread for TauriMainThread {
    fn run(&self, f: Box<dyn FnOnce() + Send>) {
        if let Err(e) = self.0.run_on_main_thread(f) {
            tracing::warn!("could not run on the main thread: {e}");
        }
    }
}

/// This process's descendants (the WebView2 processes on Windows), so input is never
/// sent to their windows.
#[cfg(windows)]
fn child_pids() -> Vec<u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    let me = std::process::id();
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    // SAFETY: a process snapshot read with the documented first/next loop; the handle is
    // closed afterwards.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return Vec::new();
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                pairs.push((entry.th32ProcessID, entry.th32ParentProcessID));
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    let mut out = Vec::new();
    let mut frontier = vec![me];
    while let Some(parent) = frontier.pop() {
        for &(pid, ppid) in &pairs {
            if ppid == parent && pid != me && !out.contains(&pid) {
                out.push(pid);
                frontier.push(pid);
            }
        }
    }
    out
}

#[cfg(not(windows))]
fn child_pids() -> Vec<u32> {
    Vec::new()
}

/// The indicator's Stop button, in global input coordinates (physical pixels on Windows,
/// points on macOS): input there is refused.
fn indicator_rects(app: &AppHandle) -> Vec<computer::Rect> {
    let Some(w) = app.get_webview_window(crate::overlay::INDICATOR_WINDOW) else {
        return Vec::new();
    };
    if !w.is_visible().unwrap_or(false) {
        return Vec::new();
    }
    let (Ok(pos), Ok(size)) = (w.outer_position(), w.outer_size()) else {
        return Vec::new();
    };
    let scale = if cfg!(target_os = "macos") {
        w.scale_factor().unwrap_or(1.0)
    } else {
        1.0
    };
    vec![computer::Rect::new(
        f64::from(pos.x) / scale,
        f64::from(pos.y) / scale,
        f64::from(size.width) / scale,
        f64::from(size.height) / scale,
    )]
}

/// The overlay's window number (macOS), which the injector must look through.
#[cfg(target_os = "macos")]
fn overlay_window_numbers(app: &AppHandle) -> Vec<u64> {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    let Some(w) = app.get_webview_window(crate::OVERLAY_WINDOW) else {
        return Vec::new();
    };
    let Ok(ptr) = w.ns_window() else {
        return Vec::new();
    };
    if ptr.is_null() {
        return Vec::new();
    }
    // SAFETY: `ptr` is this app's live NSWindow; `windowNumber` is a read-only property
    // that may be read from any thread.
    let number: isize = unsafe { msg_send![ptr as *mut AnyObject, windowNumber] };
    u64::try_from(number).map(|n| vec![n]).unwrap_or_default()
}

/// Whether the overlay is on the active Space (macOS); asks the main thread, waits 1 s.
#[cfg(target_os = "macos")]
fn overlay_on_active_space(app: &AppHandle) -> Option<bool> {
    let w = app.get_webview_window(crate::OVERLAY_WINDOW)?;
    let addr = w.ns_window().ok()? as usize;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        // SAFETY: the overlay is one of this app's live NSWindows; this runs on the main thread.
        let on = unsafe { computer::window_is_on_active_space(addr as *mut std::ffi::c_void) };
        let _ = tx.send(on);
    })
    .ok()?;
    rx.recv_timeout(std::time::Duration::from_secs(1))
        .ok()
        .flatten()
}

/// Builds the native server (Windows and macOS); `None` elsewhere or when the platform
/// layer cannot start.
pub fn build(app: &AppHandle) -> Option<Arc<computer::ComputerServer>> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let rects_app = app.clone();
        #[allow(unused_mut)]
        let mut me = computer::SelfProcess::current(child_pids)
            .with_protected_rects(move || indicator_rects(&rects_app));
        #[cfg(target_os = "macos")]
        {
            let overlay_app = app.clone();
            me = me.with_click_through_windows(move || overlay_window_numbers(&overlay_app));
        }
        let ready_app = app.clone();
        let config = computer::ComputerConfig {
            default_profile: computer::ModelProfile::default(),
            ready_check: Some(Arc::new(move || ready(&ready_app))),
        };
        match computer::ComputerServer::native(Arc::new(TauriMainThread(app.clone())), me, config) {
            Ok(server) => Some(Arc::new(server)),
            Err(e) => {
                tracing::warn!("computer use is unavailable: {e}");
                None
            }
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (app, indicator_rects, child_pids);
        None
    }
}

/// Before every input action: the indicator is up and the stop hotkey works (9.3).
#[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
fn ready(app: &AppHandle) -> Result<(), String> {
    if app.state::<crate::hotkey::Hotkey>().accelerator().is_none() {
        return Err("the stop hotkey is not registered, so computer use is off; choose a hotkey in Settings → Permissions".into());
    }
    let visible = app
        .get_webview_window(crate::OVERLAY_WINDOW)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if !visible && !crate::overlay::show(app, None) {
        return Err("the \"AI is controlling this computer\" indicator could not be shown, so computer use is off".into());
    }
    #[cfg(target_os = "macos")]
    if overlay_on_active_space(app) == Some(false) {
        return Err("the \"AI is controlling this computer\" indicator is not visible on the current Space, so computer use is off".into());
    }
    Ok(())
}

/// Called when a turn starts: sets the chat's profile, or keeps the tools from running.
pub async fn prepare_turn(app: &AppHandle, chat_id: i64, model_ref: &str) {
    let Some(server) = app.state::<ComputerHandle>().0.clone() else {
        return;
    };
    let state = app.state::<AppState>();
    if let Some(p) = profile_for(&state, model_ref).await {
        server.set_profile(chat_id, p.profile);
    }
}

/// Called when a turn ends: hides the indicator and releases anything still held.
pub fn end_turn(app: &AppHandle) {
    if let Some(server) = app.state::<ComputerHandle>().0.clone() {
        server.release_all();
    }
    crate::overlay::hide(app);
}

/// Stop (hotkey, Stop button, indicator): releases every key and button the injector holds.
pub fn release_all(app: &AppHandle) {
    if let Some(handle) = app.try_state::<ComputerHandle>()
        && let Some(server) = handle.0.clone()
    {
        server.release_all();
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerStatus {
    /// This OS has computer use and its platform layer started.
    pub available: bool,
    /// macOS: Screen Recording and Accessibility are granted (always true elsewhere).
    pub screen_recording: bool,
    pub accessibility: bool,
    pub screen_recording_url: &'static str,
    pub accessibility_url: &'static str,
    /// The current model's profile, if computer use is set up for it.
    pub profile: Option<StoredProfile>,
}

#[tauri::command]
pub async fn computer_status(
    app: AppHandle,
    state: State<'_, AppState>,
    model_ref: Option<String>,
) -> AppResult<ComputerStatus> {
    #[cfg(target_os = "macos")]
    let (screen_recording, accessibility) = (
        computer::screen_recording_granted(),
        computer::accessibility_granted(),
    );
    #[cfg(not(target_os = "macos"))]
    let (screen_recording, accessibility) = (true, true);
    let profile = match model_ref {
        Some(m) => profile_for(&state, &m).await,
        None => None,
    };
    Ok(ComputerStatus {
        available: app.state::<ComputerHandle>().0.is_some(),
        screen_recording,
        accessibility,
        screen_recording_url: computer::MACOS_SCREEN_RECORDING_URL,
        accessibility_url: computer::MACOS_ACCESSIBILITY_URL,
        profile,
    })
}

/// Saves a model's coordinate convention. `confirmed` is set once the user has seen the
/// calibration test land on its targets.
#[tauri::command]
pub async fn set_computer_profile(
    state: State<'_, AppState>,
    model_ref: String,
    convention: computer::Convention,
    confirmed: bool,
) -> AppResult<()> {
    if model_ref.trim().is_empty() {
        return Err(AppError::bad_request("No model is selected."));
    }
    let mut stored = profile_for(&state, &model_ref)
        .await
        .unwrap_or(StoredProfile {
            profile: computer::ModelProfile::default(),
            confirmed: false,
        });
    stored.profile.convention = convention;
    stored.confirmed = confirmed;
    state
        .store
        .set_setting(&profile_key(&model_ref), &stored)
        .await?;
    Ok(())
}
