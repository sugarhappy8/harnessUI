//! Computer use in the app (ARCHITECTURE.md 9.3 "Computer use", 13.3, D7): the native
//! server on Windows and macOS, the indicator while it acts, per-model coordinate
//! profiles, the macOS permission checks, and Stop.
//!
//! The tools are offered for every model that can see images, with nothing to set up: how
//! the model writes screen positions is worked out from its name
//! ([`computer::auto_profile`]), and the user can choose another convention per model in
//! Settings → Computer use (9.3, "Model conventions"). What keeps computer use safe is
//! unchanged and is not here: the tools are off in a chat until the user switches them on,
//! every kind of input asks per application, and the indicator and the stop hotkey must
//! be working before any input is sent.

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

/// The convention the user chose for a model in Settings → Computer use. A model without
/// one runs with the automatic profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredProfile {
    pub profile: computer::ModelProfile,
    /// Older versions kept computer use off for a model until this was ticked. Nothing
    /// reads it now: saving a choice is the user's decision, ticked or not. It is still
    /// written (as `true`) so an older version reading these settings keeps working.
    #[serde(default)]
    pub confirmed: bool,
}

/// The profile computer use runs with for a model, and where it came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChosenProfile {
    pub profile: computer::ModelProfile,
    /// The user chose it in Settings; otherwise it is the automatic one.
    pub by_user: bool,
    /// What the model's name says, whether or not it is in use.
    pub auto: computer::AutoProfile,
}

/// The user's saved choice when there is one, else the automatic profile.
fn choose(stored: Option<StoredProfile>, model_ref: &str) -> ChosenProfile {
    let auto = computer::auto_profile(model_ref);
    match stored {
        Some(s) => ChosenProfile {
            profile: s.profile,
            by_user: true,
            auto,
        },
        None => ChosenProfile {
            profile: auto.profile,
            by_user: false,
            auto,
        },
    }
}

/// The profile for a model: every model has one.
pub async fn profile_for(state: &AppState, model_ref: &str) -> ChosenProfile {
    // A setting that cannot be read is treated as no choice.
    let stored = state
        .store
        .get_setting(&profile_key(model_ref))
        .await
        .ok()
        .flatten();
    choose(stored, model_ref)
}

/// Shown on the Computer group in the Tools panel for a model without vision.
pub const NO_VISION: &str =
    "This model cannot see images, which computer use needs. Pick a vision model.";

/// Why the computer tools cannot be offered for a chat's model, if they cannot.
///
/// One reason is left: the model cannot see the screenshots. That is known for the local
/// model that is loaded (`loaded_path`, `loaded_vision`); a local model that is not loaded
/// yet and a remote model may well be able to see, so they are not refused here. On an
/// OS without computer use the server is never registered, so there are no tools to mark.
pub fn unavailable_reason(
    model_ref: &str,
    loaded_path: Option<&str>,
    loaded_vision: bool,
) -> Option<&'static str> {
    let path = model_ref.strip_prefix("local:")?;
    (loaded_path == Some(path) && !loaded_vision).then_some(NO_VISION)
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

/// For a chat no turn has prepared ([`prepare_turn`] sets the real one): the profile of an
/// unknown model.
#[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
fn unprepared_profile() -> computer::ModelProfile {
    computer::auto_profile("").profile
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
            default_profile: unprepared_profile(),
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

/// Called when a turn starts: tells the server how the chat's model writes positions.
pub async fn prepare_turn(app: &AppHandle, chat_id: i64, model_ref: &str) {
    let Some(server) = app.state::<ComputerHandle>().0.clone() else {
        return;
    };
    let state = app.state::<AppState>();
    let chosen = profile_for(&state, model_ref).await;
    tracing::debug!(
        convention = ?chosen.profile.convention,
        by_user = chosen.by_user,
        family = ?chosen.auto.family,
        "computer use profile for this turn"
    );
    server.set_profile(chat_id, chosen.profile);
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
    /// How the asked-about model writes positions (`None` when no model was named).
    pub profile: Option<ProfileStatus>,
}

/// A model's coordinate convention, for Settings → Computer use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileStatus {
    /// What computer use runs with for this model.
    pub convention: computer::Convention,
    /// It is the automatic pick; false when the user chose one.
    pub automatic: bool,
    /// What Automatic picks for this model.
    pub auto_convention: computer::Convention,
    /// The model's name was not recognised, so the automatic pick is a best guess.
    pub auto_guess: bool,
}

impl From<ChosenProfile> for ProfileStatus {
    fn from(c: ChosenProfile) -> Self {
        Self {
            convention: c.profile.convention,
            automatic: !c.by_user,
            auto_convention: c.auto.profile.convention,
            auto_guess: c.auto.is_guess(),
        }
    }
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
        Some(m) if !m.trim().is_empty() => Some(profile_for(&state, &m).await.into()),
        _ => None,
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

/// Saves the convention the user chose for a model, or with no convention goes back to
/// Automatic (the choice is forgotten).
#[tauri::command]
pub async fn set_computer_profile(
    state: State<'_, AppState>,
    model_ref: String,
    convention: Option<computer::Convention>,
) -> AppResult<()> {
    if model_ref.trim().is_empty() {
        return Err(AppError::bad_request("No model is selected."));
    }
    let key = profile_key(&model_ref);
    match convention {
        Some(c) => {
            state
                .store
                .set_setting(&key, &chosen_by_user(&model_ref, c))
                .await?;
        }
        None => {
            state.store.delete_setting(&key).await?;
        }
    }
    Ok(())
}

/// What is saved when the user picks a convention for a model.
fn chosen_by_user(model_ref: &str, convention: computer::Convention) -> StoredProfile {
    StoredProfile {
        profile: computer::auto_profile(model_ref).with_convention(convention),
        confirmed: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use computer::Convention;

    const TARS: &str =
        "local:/home/u/models/Mungert/UI-TARS-1.5-7B-GGUF/UI-TARS-1.5-7B-q4_k_m.gguf";
    const UNKNOWN: &str = "local:/home/u/models/someone/new-vision-model.gguf";

    /// The owner's decision of 2026-11-11: computer use needs no per-model setup. A model
    /// nobody set anything up for has a profile, picked from its name.
    #[test]
    fn every_model_has_a_profile_without_any_setup() {
        let known = choose(None, TARS);
        assert!(!known.by_user);
        assert_eq!(known.profile.convention, Convention::Pixels);
        assert_eq!(known.profile, computer::auto_profile(TARS).profile);
        assert!(!known.auto.is_guess());

        let guess = choose(None, UNKNOWN);
        assert!(!guess.by_user);
        assert_eq!(guess.profile.convention, Convention::Grid1000);
        assert!(guess.auto.is_guess());

        let remote = choose(None, "remote:3:qwen2.5-vl-7b");
        assert_eq!(remote.profile.convention, Convention::Pixels);
    }

    /// Profiles saved by older versions are still read, and one counts whether or not its
    /// confirmation box was ticked: saving it was the user's choice.
    #[test]
    fn a_saved_profile_wins_whether_or_not_it_was_confirmed() {
        for confirmed in ["true", "false"] {
            let old = format!(
                r#"{{"profile":{{"convention":"box1000","resize":{{"type":"smart","align":28,"minPixels":6272,"maxPixels":3211264,"longestEdge":0,"roundHalfEven":true}}}},"confirmed":{confirmed}}}"#
            );
            let stored: StoredProfile = serde_json::from_str(&old).unwrap();
            let chosen = choose(Some(stored), TARS);
            assert!(chosen.by_user, "confirmed {confirmed}");
            assert_eq!(chosen.profile.convention, Convention::Box1000);
            assert_eq!(
                chosen.profile.resize,
                computer::ResizeRule::Smart(computer::SmartResize::qwen2_vl(8, 4096))
            );
            // What Automatic would pick is still reported next to it.
            assert_eq!(chosen.auto.profile.convention, Convention::Pixels);
        }
        // The field may be missing altogether.
        let bare: StoredProfile = serde_json::from_str(
            r#"{"profile":{"convention":"grid1000","resize":{"type":"original"}}}"#,
        )
        .unwrap();
        assert_eq!(bare.profile.convention, Convention::Grid1000);
    }

    #[test]
    fn what_settings_shows_for_a_model() {
        assert_eq!(
            ProfileStatus::from(choose(None, TARS)),
            ProfileStatus {
                convention: Convention::Pixels,
                automatic: true,
                auto_convention: Convention::Pixels,
                auto_guess: false,
            }
        );
        assert_eq!(
            ProfileStatus::from(choose(None, UNKNOWN)),
            ProfileStatus {
                convention: Convention::Grid1000,
                automatic: true,
                auto_convention: Convention::Grid1000,
                auto_guess: true,
            }
        );
        // The user's choice, as `set_computer_profile` saves it and a later read finds it.
        let saved = serde_json::to_string(&chosen_by_user(UNKNOWN, Convention::Pixels)).unwrap();
        let read: StoredProfile = serde_json::from_str(&saved).unwrap();
        assert_eq!(
            read.profile,
            computer::auto_profile(UNKNOWN).with_convention(Convention::Pixels)
        );
        assert_eq!(
            ProfileStatus::from(choose(Some(read), UNKNOWN)),
            ProfileStatus {
                convention: Convention::Pixels,
                automatic: false,
                auto_convention: Convention::Grid1000,
                auto_guess: true,
            }
        );
        // An older version reading the saved choice still finds it confirmed.
        assert!(saved.contains(r#""confirmed":true"#), "{saved}");
        // The frontend's field names.
        let json = serde_json::to_value(ProfileStatus::from(choose(None, UNKNOWN))).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "convention": "grid1000",
                "automatic": true,
                "autoConvention": "grid1000",
                "autoGuess": true,
            })
        );
    }

    /// The one reason left for the Computer group to be unavailable: the loaded local
    /// model cannot see. There is no "set it up first" any more.
    #[test]
    fn the_tools_are_unavailable_only_for_a_model_that_cannot_see() {
        let path = "/home/u/models/someone/text-only.gguf";
        let local = format!("local:{path}");
        // Loaded, without vision.
        assert_eq!(
            unavailable_reason(&local, Some(path), false),
            Some(NO_VISION)
        );
        // Loaded with vision: available, with no profile saved by anyone.
        assert_eq!(unavailable_reason(&local, Some(path), true), None);
        // Not loaded yet, or another model is loaded: it may be able to see.
        assert_eq!(unavailable_reason(&local, None, false), None);
        assert_eq!(unavailable_reason(&local, Some("/other.gguf"), false), None);
        // Remote models are not refused here.
        assert_eq!(
            unavailable_reason("remote:3:some-model", Some(path), false),
            None
        );
        assert_eq!(unavailable_reason("remote:3:some-model", None, false), None);
        assert_eq!(
            NO_VISION,
            "This model cannot see images, which computer use needs. Pick a vision model."
        );
    }
}
