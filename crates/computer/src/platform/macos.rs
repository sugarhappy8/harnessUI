//! macOS capture, input, permissions and the overlay window helper (ARCHITECTURE.md 9.3).
//!
//! - **Capture:** on macOS 14 and later, ScreenCaptureKit's `SCScreenshotManager` with a
//!   content filter that excludes this app's processes (the overlay included). On macOS
//!   13 only, the older `CGWindowListCreateImageFromArray` over every on-screen window
//!   except this app's. Bindings: `objc2-screen-capture-kit`, from the objc2 family Tauri
//!   already uses, with no Swift build step (the `screencapturekit` crate compiles a Swift
//!   bridge, which needs Xcode on every build machine).
//! - **Input:** enigo (CGEvent), keyboard and pointer, every call on the main thread
//!   through [`MainThread`]. Points are global display coordinates (top-left origin).
//! - **Targets:** `CGWindowListCopyWindowInfo` front to back, the first window that
//!   contains the point (skipping transparent windows and this app's click-through
//!   windows), and its owner pid; this app's window beneath a window that may let clicks
//!   through counts as hit (see [`super::hit`]). The menu bar belongs to the application
//!   in front. Keyboard input goes to the application Accessibility reports as focused
//!   (it follows non-activating panels such as Spotlight or a password manager's quick
//!   access window), else to `NSWorkspace.frontmostApplication`.
//! - **Permissions** are only checked here, never requested: `CGPreflightScreenCaptureAccess`
//!   and `AXIsProcessTrusted` show no system prompt.
//!
//! The window list cannot see `ignoresMouseEvents`, so the overlay's window number must
//! be registered with [`SelfProcess::with_click_through_windows`] while it is
//! click-through; otherwise the injector treats it as this app's window and refuses
//! every point under it (safe, but unusable).

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::mpsc::sync_channel;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSRunningApplication, NSWindow, NSWindowCollectionBehavior, NSWorkspace};
use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGDirectDisplayID, CGDisplayBounds,
    CGDisplayCopyDisplayMode, CGDisplayIsBuiltin, CGDisplayMode, CGError, CGEvent, CGEventFlags,
    CGEventSource, CGEventSourceStateID, CGGetActiveDisplayList, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGMainDisplayID, CGMouseButton, CGPreflightScreenCaptureAccess,
    CGRectMakeWithDictionaryRepresentation, CGWindowImageOption, CGWindowListCopyWindowInfo,
    CGWindowListOption, kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer, kCGWindowName,
    kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCRunningApplication, SCScreenshotManager, SCShareableContent,
    SCStreamConfiguration, SCWindow,
};

use crate::apps::{AppInfo, Target};
use crate::geometry::{GlobalPoint, MonitorInfo, Rect};
use crate::keys::Key;
use crate::platform::hit::{Hit, ListedWindow as WinInfo, hit_test};
use crate::platform::{
    Backends, Capture, CaptureError, Captured, Gate, Injector, InputError, InputEvent,
    MAIN_THREAD_TIMEOUT, MainThread, MouseButton, PlatformError, Pressable, SelfProcess,
    run_on_main, run_on_main_even_late,
};

/// System Settings → Privacy & Security → Screen Recording.
pub const SCREEN_RECORDING_SETTINGS_URL: &str = crate::MACOS_SCREEN_RECORDING_URL;
/// System Settings → Privacy & Security → Accessibility.
pub const ACCESSIBILITY_SETTINGS_URL: &str = crate::MACOS_ACCESSIBILITY_URL;

/// How long ScreenCaptureKit may take to answer.
const SCK_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an Accessibility query may take (the default is about 6 s).
const AX_TIMEOUT_SECONDS: f32 = 0.5;

/// Screen Recording is granted (no system prompt).
pub fn screen_recording_granted() -> bool {
    CGPreflightScreenCaptureAccess()
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    /// `Boolean AXIsProcessTrusted(void)`; `Boolean` is an unsigned char.
    fn AXIsProcessTrusted() -> u8;
    /// `AXUIElementRef AXUIElementCreateSystemWide(void)` (Create rule).
    fn AXUIElementCreateSystemWide() -> *mut c_void;
    /// `AXError AXUIElementCopyAttributeValue(AXUIElementRef, CFStringRef, CFTypeRef *)`
    /// (the value follows the Create rule).
    fn AXUIElementCopyAttributeValue(
        element: *mut c_void,
        attribute: *const c_void,
        value: *mut *const c_void,
    ) -> i32;
    /// `AXError AXUIElementGetPid(AXUIElementRef, pid_t *)`.
    fn AXUIElementGetPid(element: *mut c_void, pid: *mut i32) -> i32;
    /// `AXError AXUIElementSetMessagingTimeout(AXUIElementRef, float)`.
    fn AXUIElementSetMessagingTimeout(element: *mut c_void, seconds: f32) -> i32;
}

/// The application with the keyboard focus, as Accessibility reports it. Unlike
/// `NSWorkspace.frontmostApplication` this follows non-activating panels (Spotlight, a
/// password manager's quick-access window, launchers) that take the keyboard without
/// becoming the frontmost application. `None` without the Accessibility permission (input
/// cannot be posted then anyway) or when the query fails.
fn ax_focused_pid() -> Option<u32> {
    // SAFETY: the system-wide element and the attribute value are owned (Create rule) and
    // released by CFRetained; every pointer passed is valid for the call; the attribute
    // name is a CFString that outlives the call.
    unsafe {
        let system = CFRetained::<CFType>::from_raw(
            NonNull::new(AXUIElementCreateSystemWide())?.cast::<CFType>(),
        );
        let system_ptr = (&*system as *const CFType).cast_mut().cast::<c_void>();
        let _ = AXUIElementSetMessagingTimeout(system_ptr, AX_TIMEOUT_SECONDS);
        let attribute = CFString::from_static_str("AXFocusedApplication");
        let mut value: *const c_void = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(
            system_ptr,
            (&*attribute as *const CFString).cast::<c_void>(),
            &mut value,
        );
        if err != 0 {
            return None;
        }
        let app = CFRetained::<CFType>::from_raw(NonNull::new(value.cast_mut())?.cast::<CFType>());
        let mut pid: i32 = 0;
        let err = AXUIElementGetPid(
            (&*app as *const CFType).cast_mut().cast::<c_void>(),
            &mut pid,
        );
        (err == 0 && pid > 0).then_some(pid as u32)
    }
}

/// Accessibility (needed to post input events) is granted. Uses `AXIsProcessTrusted`,
/// which never shows the system prompt.
pub fn accessibility_granted() -> bool {
    // SAFETY: no arguments, no preconditions.
    unsafe { AXIsProcessTrusted() != 0 }
}

/// Lets the overlay appear on every Space, including over another app's full-screen
/// Space: adds the full-screen-auxiliary and can-join-all-spaces collection behaviour
/// (and removes the flags macOS refuses to combine with them).
///
/// # Safety
///
/// `ns_window` must be a valid `NSWindow *` (Tauri's `WebviewWindow::ns_window()`) that
/// stays alive for the call.
pub unsafe fn set_overlay_window_behavior(ns_window: *mut c_void) -> Result<(), String> {
    if MainThreadMarker::new().is_none() {
        return Err("set_overlay_window_behavior must run on the main thread".into());
    }
    // SAFETY: the caller passes a valid NSWindow pointer; we are on the main thread.
    let Some(win) = (unsafe { (ns_window as *const NSWindow).as_ref() }) else {
        return Err("no window".into());
    };
    let mut b = win.collectionBehavior();
    b.remove(
        NSWindowCollectionBehavior::MoveToActiveSpace
            | NSWindowCollectionBehavior::FullScreenPrimary
            | NSWindowCollectionBehavior::FullScreenNone,
    );
    b |= NSWindowCollectionBehavior::FullScreenAuxiliary
        | NSWindowCollectionBehavior::CanJoinAllSpaces;
    win.setCollectionBehavior(b);
    Ok(())
}

/// Whether the window is visible on the active Space (the computer tool refuses to act
/// when the overlay is not; 9.3). `None` off the main thread or for a null pointer.
///
/// # Safety
///
/// As [`set_overlay_window_behavior`].
pub unsafe fn window_is_on_active_space(ns_window: *mut c_void) -> Option<bool> {
    MainThreadMarker::new()?;
    // SAFETY: as above.
    let win = unsafe { (ns_window as *const NSWindow).as_ref() }?;
    Some(win.isOnActiveSpace())
}

pub(super) fn native(
    main: Arc<dyn MainThread>,
    me: SelfProcess,
) -> Result<Backends, PlatformError> {
    let capture = MacCapture {
        main: main.clone(),
        me: me.clone(),
    };
    let injector = MacInjector { main, me };
    Ok((Arc::new(capture), Arc::new(injector)))
}

/// Moves a value to the thread waiting for a completion handler.
struct SendBox<T>(T);
// SAFETY: used only for ScreenCaptureKit results (SCShareableContent, CGImage) and
// error strings, which are immutable once delivered; the value is moved to exactly one
// waiting thread and never used from two threads at once.
unsafe impl<T> Send for SendBox<T> {}

fn dict_value(d: &CFDictionary, key: &CFString) -> *const c_void {
    // SAFETY: d is a valid dictionary from the window list and key a CFString constant;
    // CFDictionaryGetValue returns null for a missing key.
    unsafe { d.value((key as *const CFString).cast()) }
}

fn dict_number<'a>(d: &'a CFDictionary, key: &CFString) -> Option<&'a CFNumber> {
    let v = dict_value(d, key);
    // SAFETY: the window list stores CFNumbers under these keys.
    (!v.is_null()).then(|| unsafe { &*(v as *const CFNumber) })
}

fn dict_string(d: &CFDictionary, key: &CFString) -> Option<String> {
    let v = dict_value(d, key);
    // SAFETY: the window list stores CFStrings under these keys.
    (!v.is_null()).then(|| unsafe { &*(v as *const CFString) }.to_string())
}

/// On-screen windows, front to back, without desktop elements.
fn window_list() -> Vec<WinInfo> {
    let Some(list) = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    ) else {
        return Vec::new();
    };
    // SAFETY: the window-info keys are constant CFStrings exported by CoreGraphics.
    let (k_num, k_pid, k_layer, k_alpha, k_bounds, k_owner, k_name) = unsafe {
        (
            kCGWindowNumber,
            kCGWindowOwnerPID,
            kCGWindowLayer,
            kCGWindowAlpha,
            kCGWindowBounds,
            kCGWindowOwnerName,
            kCGWindowName,
        )
    };
    let mut out = Vec::new();
    for i in 0..list.count() {
        // SAFETY: i is in bounds; the list holds one CFDictionary per window and is not
        // mutated while we read it.
        let d = unsafe { &*(list.value_at_index(i) as *const CFDictionary) };
        let bounds_dict = dict_value(d, k_bounds);
        let mut r = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0));
        // SAFETY: the bounds entry is a CGRect dictionary representation; r is valid.
        let has_bounds = !bounds_dict.is_null()
            && unsafe {
                CGRectMakeWithDictionaryRepresentation(
                    Some(&*(bounds_dict as *const CFDictionary)),
                    &mut r,
                )
            };
        if !has_bounds {
            continue;
        }
        out.push(WinInfo {
            number: dict_number(d, k_num).and_then(|n| n.as_i64()).unwrap_or(0) as u32,
            pid: dict_number(d, k_pid).and_then(|n| n.as_i64()).unwrap_or(0) as u32,
            layer: dict_number(d, k_layer)
                .and_then(|n| n.as_i64())
                .unwrap_or(0),
            alpha: dict_number(d, k_alpha)
                .and_then(|n| n.as_f64())
                .unwrap_or(1.0),
            bounds: Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height),
            owner: dict_string(d, k_owner).unwrap_or_default(),
            name: dict_string(d, k_name),
        });
    }
    out
}

/// Bundle id, name and executable path of a process.
fn app_of_pid(pid: u32, fallback_name: &str) -> AppInfo {
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32);
    match app {
        Some(a) => {
            let bundle = a.bundleIdentifier().map(|s| s.to_string());
            let name = a
                .localizedName()
                .map(|s| s.to_string())
                .unwrap_or_else(|| fallback_name.to_string());
            let exe = a
                .executableURL()
                .and_then(|u| u.path())
                .map(|s| s.to_string());
            AppInfo::from_macos(bundle.as_deref(), &name, exe.as_deref())
        }
        None => AppInfo::from_macos(None, fallback_name, None),
    }
}

fn self_app() -> AppInfo {
    AppInfo {
        id: "this-app".into(),
        name: "this app".into(),
        exe_name: String::new(),
        bundle_id: None,
    }
}

/// The frontmost application's pid, read on the main thread (where NSWorkspace keeps it
/// current).
fn frontmost_pid(main: &dyn MainThread) -> Option<u32> {
    run_on_main(main, MAIN_THREAD_TIMEOUT, || {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|a| a.processIdentifier() as u32)
    })
    .ok()
    .flatten()
}

struct MacInjector {
    main: Arc<dyn MainThread>,
    me: SelfProcess,
}

impl MacInjector {
    fn target_for_pid(&self, pid: u32, owner: &str, window: Option<&WinInfo>) -> Target {
        if self.me.owns_any(&[pid]) {
            return Target {
                pids: vec![pid],
                app: self_app(),
                elevated: false,
                window_title: None,
                window_id: window.map_or(0, |w| u64::from(w.number)),
                window_rect: window.map(|w| w.bounds),
                console: false,
            };
        }
        Target {
            pids: vec![pid],
            app: app_of_pid(pid, owner),
            elevated: false,
            window_title: window.and_then(|w| w.name.clone()),
            window_id: window.map_or(0, |w| u64::from(w.number)),
            window_rect: window.map(|w| w.bounds),
            console: false,
        }
    }

    /// The application in front. `keyboard`: the one keyboard input reaches, which is the
    /// application Accessibility reports as focused when that differs (a non-activating
    /// panel); both pids are then in the target, so either being this app refuses it.
    fn frontmost_target(&self, keyboard: bool) -> Result<Target, InputError> {
        let front = frontmost_pid(&*self.main);
        let focused = if keyboard { ax_focused_pid() } else { None };
        let pid = focused
            .or(front)
            .ok_or_else(|| InputError::NoTarget("no application is in front right now".into()))?;
        let windows = window_list();
        let win = windows
            .iter()
            .find(|w| w.pid == pid && w.layer == 0)
            .or_else(|| windows.iter().find(|w| w.pid == pid));
        let owner = win.map(|w| w.owner.clone()).unwrap_or_default();
        let mut target = self.target_for_pid(pid, &owner, win);
        if let Some(f) = front
            && !target.pids.contains(&f)
        {
            target.pids.push(f);
        }
        Ok(target)
    }

    fn enigo_send(&self, event: &InputEvent, gate: &Gate) -> Result<(), InputError> {
        let release = event.is_release();
        let event = event.clone();
        let gate = gate.clone();
        let step = move || {
            // Checked on the main thread, right before enigo sends, so an event queued
            // before Stop cannot land after Stop's releases (which also run here).
            if !gate.is_open() {
                return Err(InputError::Stopped);
            }
            with_enigo(|e| enigo_event(e, &event))
        };
        // A press, move or click the main thread did not get to in time is abandoned
        // (never sent late, after its target check); a release still goes out late.
        if release {
            run_on_main_even_late(&*self.main, MAIN_THREAD_TIMEOUT, step)?
        } else {
            run_on_main(&*self.main, MAIN_THREAD_TIMEOUT, step)?
        }
    }
}

thread_local! {
    /// The main thread's enigo, created on first use (after the permission screen).
    static ENIGO: RefCell<Option<enigo::Enigo>> = const { RefCell::new(None) };
}

fn with_enigo<R>(
    f: impl FnOnce(&mut enigo::Enigo) -> Result<R, InputError>,
) -> Result<R, InputError> {
    ENIGO.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let settings = enigo::Settings {
                release_keys_when_dropped: true,
                open_prompt_to_get_permissions: false,
                ..Default::default()
            };
            let e = enigo::Enigo::new(&settings).map_err(|e| match e {
                enigo::NewConError::NoPermission => InputError::NoPermission(
                    "Accessibility permission is missing: allow this app in System Settings → Privacy & Security → Accessibility.".into(),
                ),
                other => InputError::Platform(format!("input is unavailable: {other}")),
            })?;
            *slot = Some(e);
        }
        match slot.as_mut() {
            Some(e) => f(e),
            None => Err(InputError::Platform("input is unavailable".into())),
        }
    })
}

fn enigo_key(k: Key) -> Result<enigo::Key, InputError> {
    use enigo::Key as E;
    Ok(match k {
        Key::Shift => E::Shift,
        Key::Control => E::Control,
        Key::Alt => E::Option,
        Key::Meta => E::Meta,
        Key::Enter => E::Return,
        Key::Tab => E::Tab,
        Key::Escape => E::Escape,
        Key::Backspace => E::Backspace,
        Key::Delete => E::Delete,
        Key::Space => E::Space,
        Key::Up => E::UpArrow,
        Key::Down => E::DownArrow,
        Key::Left => E::LeftArrow,
        Key::Right => E::RightArrow,
        Key::Home => E::Home,
        Key::End => E::End,
        Key::PageUp => E::PageUp,
        Key::PageDown => E::PageDown,
        // Mac keyboards have Help where PC keyboards have Insert.
        Key::Insert => E::Help,
        Key::CapsLock => E::CapsLock,
        Key::PrintScreen => {
            return Err(InputError::Platform(
                "Mac keyboards have no Print Screen key; use cmd+shift+3".into(),
            ));
        }
        Key::F(n) => match n {
            1 => E::F1,
            2 => E::F2,
            3 => E::F3,
            4 => E::F4,
            5 => E::F5,
            6 => E::F6,
            7 => E::F7,
            8 => E::F8,
            9 => E::F9,
            10 => E::F10,
            11 => E::F11,
            12 => E::F12,
            13 => E::F13,
            14 => E::F14,
            15 => E::F15,
            16 => E::F16,
            17 => E::F17,
            18 => E::F18,
            19 => E::F19,
            20 => E::F20,
            _ => return Err(InputError::Platform(format!("F{n} is not supported"))),
        },
        Key::Char(c) => E::Unicode(c),
    })
}

fn enigo_event(e: &mut enigo::Enigo, event: &InputEvent) -> Result<(), InputError> {
    use enigo::{Axis, Button, Coordinate, Direction, Keyboard, Mouse};
    let dir = |down: bool| {
        if down {
            Direction::Press
        } else {
            Direction::Release
        }
    };
    let r = match event {
        InputEvent::Move { to } => {
            // enigo takes whole points, so on a 2x display a click can land up to one
            // point (two pixels) up-left of the mapped position.
            // TODO(M7): post the CGEvent with fractional points ourselves if the M7
            // calibration shows misclicks on Retina displays.
            let (x, y) = to.device();
            e.move_mouse(x, y, Coordinate::Abs)
        }
        InputEvent::Button { button, down } => {
            let b = match button {
                MouseButton::Left => Button::Left,
                MouseButton::Right => Button::Right,
                MouseButton::Middle => Button::Middle,
            };
            e.button(b, dir(*down))
        }
        InputEvent::Scroll { dx, dy } => {
            // enigo: positive scrolls down / right.
            let mut r = Ok(());
            if *dy != 0 {
                r = e.scroll(*dy, Axis::Vertical);
            }
            if r.is_ok() && *dx != 0 {
                r = e.scroll(*dx, Axis::Horizontal);
            }
            r
        }
        InputEvent::Key { key, down } => e.key(enigo_key(*key)?, dir(*down)),
        InputEvent::Text { text } => e.text(text),
    };
    r.map_err(|err| InputError::Platform(err.to_string()))
}

impl Injector for MacInjector {
    fn target_at(&self, p: GlobalPoint) -> Result<Target, InputError> {
        let click_through = self.me.click_through_windows();
        let windows = window_list();
        match hit_test(&windows, p, |pid| self.me.owns_any(&[pid]), &click_through) {
            Hit::Window(i) | Hit::SelfBeneath(i) => {
                let w = &windows[i];
                return Ok(self.target_for_pid(w.pid, &w.owner, Some(w)));
            }
            // The menu bar shows the menus of the application in front.
            Hit::MenuBar => return self.frontmost_target(false),
            Hit::Desktop => {}
        }
        // Nothing but the desktop, which Finder draws.
        Ok(Target {
            pids: Vec::new(),
            app: AppInfo::from_macos(Some("com.apple.finder"), "Finder", None),
            elevated: false,
            window_title: None,
            window_id: 0,
            window_rect: None,
            console: false,
        })
    }

    fn keyboard_target(&self) -> Result<Target, InputError> {
        self.frontmost_target(true)
    }

    fn send(&self, event: &InputEvent, gate: &Gate) -> Result<(), InputError> {
        self.enigo_send(event, gate)
    }

    fn is_pressed(&self, what: Pressable) -> Option<bool> {
        let state = CGEventSourceStateID::CombinedSessionState;
        Some(match what {
            Pressable::Button(b) => CGEventSource::button_state(
                state,
                match b {
                    MouseButton::Left => CGMouseButton::Left,
                    MouseButton::Right => CGMouseButton::Right,
                    MouseButton::Middle => CGMouseButton::Center,
                },
            ),
            Pressable::Key(k) => {
                let flags = CGEventSource::flags_state(state);
                let mask = match k {
                    Key::Shift => CGEventFlags::MaskShift,
                    Key::Control => CGEventFlags::MaskControl,
                    Key::Alt => CGEventFlags::MaskAlternate,
                    Key::Meta => CGEventFlags::MaskCommand,
                    _ => return None,
                };
                flags.contains(mask)
            }
        })
    }

    fn mac_keys(&self) -> bool {
        true
    }
}

struct MacCapture {
    main: Arc<dyn MainThread>,
    me: SelfProcess,
}

fn sck_error(err: *mut NSError) -> String {
    // SAFETY: a non-null error from a completion handler is a valid NSError.
    match unsafe { err.as_ref() } {
        Some(e) => e.localizedDescription().to_string(),
        None => "ScreenCaptureKit returned nothing".into(),
    }
}

/// Captures one display with ScreenCaptureKit (macOS 14+), leaving out every window of
/// the given processes.
fn capture_sck(
    display: CGDirectDisplayID,
    size: (u32, u32),
    exclude: &[u32],
) -> Result<CFRetained<CGImage>, CaptureError> {
    let (tx, rx) = sync_channel::<Result<SendBox<Retained<SCShareableContent>>, String>>(1);
    let handler = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        // SAFETY: a non-null content pointer is a valid SCShareableContent; retaining it
        // keeps it alive after the handler returns.
        let r = match unsafe { Retained::retain(content) } {
            Some(c) => Ok(SendBox(c)),
            None => Err(sck_error(err)),
        };
        let _ = tx.send(r);
    });
    // SAFETY: the handler matches the declared block signature and lives until called.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };
    let content = rx
        .recv_timeout(SCK_TIMEOUT)
        .map_err(|_| CaptureError::Platform("ScreenCaptureKit did not answer".into()))?
        .map_err(CaptureError::Platform)?
        .0;
    // SAFETY: plain getters on a valid SCShareableContent.
    let (displays, apps) = unsafe { (content.displays(), content.applications()) };
    let Some(scd) = displays
        .to_vec()
        .into_iter()
        // SAFETY: plain getter.
        .find(|d| unsafe { d.displayID() } == display)
    else {
        return Err(CaptureError::NoMonitor);
    };
    let excluded: Vec<Retained<SCRunningApplication>> = apps
        .to_vec()
        .into_iter()
        // SAFETY: plain getter.
        .filter(|a| exclude.contains(&(unsafe { a.processID() } as u32)))
        .collect();
    let excluded = NSArray::from_retained_slice(&excluded);
    let no_windows: Retained<NSArray<SCWindow>> = NSArray::new();
    // SAFETY: a freshly allocated filter initialised with valid objects.
    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
            SCContentFilter::alloc(),
            &scd,
            &excluded,
            &no_windows,
        )
    };
    // SAFETY: plain setters on a fresh configuration.
    let config = unsafe {
        let c = SCStreamConfiguration::new();
        c.setWidth(size.0 as usize);
        c.setHeight(size.1 as usize);
        c.setShowsCursor(false);
        c.setCapturesAudio(false);
        c
    };
    let (tx, rx) = sync_channel::<Result<SendBox<CFRetained<CGImage>>, String>>(1);
    let handler = RcBlock::new(move |image: *mut CGImage, err: *mut NSError| {
        let r = match NonNull::new(image) {
            // SAFETY: a non-null CGImage from the handler; retained to outlive it.
            Some(p) => Ok(SendBox(unsafe { CFRetained::retain(p) })),
            None => Err(sck_error(err)),
        };
        let _ = tx.send(r);
    });
    // SAFETY: valid filter, configuration and handler (macOS 14+, checked by the caller).
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
            &filter,
            &config,
            Some(&handler),
        )
    };
    Ok(rx
        .recv_timeout(SCK_TIMEOUT)
        .map_err(|_| CaptureError::Platform("ScreenCaptureKit did not answer".into()))?
        .map_err(CaptureError::Platform)?
        .0)
}

/// macOS 13: composites every on-screen window except this app's, over the display.
#[allow(deprecated)]
fn capture_legacy(bounds: CGRect, exclude: &[u32]) -> Result<CFRetained<CGImage>, CaptureError> {
    let mut ids: Vec<*const c_void> = window_list()
        .into_iter()
        .filter(|w| !exclude.contains(&w.pid))
        .map(|w| w.number as usize as *const c_void)
        .collect();
    // SAFETY: an array of raw window ids with no callbacks, as
    // CGWindowListCreateImageFromArray expects; `ids` outlives the call.
    let array =
        unsafe { CFArray::new(None, ids.as_mut_ptr(), ids.len() as isize, std::ptr::null()) }
            .ok_or_else(|| CaptureError::Platform("could not build the window list".into()))?;
    // SAFETY: valid bounds and window array.
    unsafe {
        objc2_core_graphics::CGWindowListCreateImageFromArray(
            bounds,
            &array,
            CGWindowImageOption::BestResolution,
        )
    }
    .ok_or_else(|| CaptureError::Platform("the screen could not be read".into()))
}

/// Draws a CGImage into an RGBA buffer of its own size.
fn rgba_of(image: &CGImage) -> Result<(u32, u32, Vec<u8>), CaptureError> {
    let w = CGImage::width(Some(image));
    let h = CGImage::height(Some(image));
    if w == 0 || h == 0 {
        return Err(CaptureError::Platform("the screenshot is empty".into()));
    }
    let mut buf = vec![0u8; w * h * 4];
    let space = CGColorSpace::new_device_rgb();
    // SAFETY: buf holds exactly w*h*4 bytes, rows of w*4 bytes; the context does not
    // outlive buf (dropped below before buf is returned).
    let ctx = unsafe {
        CGBitmapContextCreate(
            buf.as_mut_ptr().cast(),
            w,
            h,
            8,
            w * 4,
            space.as_deref(),
            CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
        )
    }
    .ok_or_else(|| CaptureError::Platform("could not read the screenshot's pixels".into()))?;
    CGContext::draw_image(
        Some(&ctx),
        CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64)),
        Some(image),
    );
    drop(ctx);
    Ok((w as u32, h as u32, buf))
}

impl Capture for MacCapture {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        let mut ids = [0 as CGDirectDisplayID; 32];
        let mut n = 0u32;
        // SAFETY: ids has room for 32 entries and n receives the count.
        let err = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) };
        if err != CGError::Success {
            return Err(CaptureError::Platform(format!(
                "the display list is unavailable (error {})",
                err.0
            )));
        }
        let main_id = CGMainDisplayID();
        let mut out = Vec::new();
        for (i, &id) in ids.iter().take(n as usize).enumerate() {
            let b = CGDisplayBounds(id);
            let mode = CGDisplayCopyDisplayMode(id);
            let px = CGDisplayMode::pixel_width(mode.as_deref()) as f64;
            let ppu = if b.size.width > 0.0 && px > 0.0 {
                px / b.size.width
            } else {
                1.0
            };
            out.push(MonitorInfo {
                id,
                // TODO(M7): NSScreen.localizedName (main thread only) for real names.
                name: if CGDisplayIsBuiltin(id) {
                    "Built-in display".into()
                } else {
                    format!("Display {}", i + 1)
                },
                bounds: Rect::new(b.origin.x, b.origin.y, b.size.width, b.size.height),
                pixels_per_unit: ppu,
                ui_scale: ppu,
                primary: id == main_id,
            });
        }
        Ok(out)
    }

    fn capture(&self, monitor: &MonitorInfo) -> Result<Captured, CaptureError> {
        if !screen_recording_granted() {
            return Err(CaptureError::NoPermission(
                "Screen Recording permission is missing: allow this app in System Settings → Privacy & Security → Screen Recording.".into(),
            ));
        }
        let exclude = self.me.pids();
        let image = if objc2::available!(macos = 14.0) {
            capture_sck(monitor.id, monitor.pixel_size(), &exclude)?
        } else {
            let b = monitor.bounds;
            capture_legacy(
                CGRect::new(CGPoint::new(b.x, b.y), CGSize::new(b.width, b.height)),
                &exclude,
            )?
        };
        let (width, height, rgba) = rgba_of(&image)?;
        Ok(Captured {
            width,
            height,
            rgba,
            self_excluded: true,
            note: None,
        })
    }

    fn foreground_window_rect(&self) -> Option<Rect> {
        let pid = frontmost_pid(&*self.main)?;
        window_list()
            .into_iter()
            .find(|w| w.pid == pid && w.layer == 0)
            .map(|w| w.bounds)
    }

    fn cursor_position(&self) -> Option<GlobalPoint> {
        let e = CGEvent::new(None)?;
        let p = CGEvent::location(Some(&e));
        Some(GlobalPoint::new(p.x, p.y))
    }
}
