//! Windows capture and input (ARCHITECTURE.md 9.3).
//!
//! - **Capture:** `xcap` (GDI `BitBlt` of one monitor, physical pixels; the app is
//!   per-monitor-v2 DPI aware). This app's visible top-level windows, the overlay
//!   included, are given `WDA_EXCLUDEFROMCAPTURE` just for the capture (Windows 10 2004
//!   and later; DWM then leaves them out of every capture path) and restored right after.
//!   Top-level windows owned by the webview's own processes (rare pop-ups) cannot be
//!   excluded this way.
//! - **Pointer:** our own `SendInput` code: absolute moves normalised over the virtual
//!   desktop with `MOUSEEVENTF_VIRTUALDESK`, so every monitor works, negative origins
//!   included (enigo can only address the primary monitor). The cursor is read back after
//!   a move and corrected if Windows rounded it onto a neighbouring pixel. Buttons follow
//!   the user's swapped-buttons setting.
//! - **Keyboard:** enigo, every call on the main thread through [`MainThread`].
//! - **Targets:** `WindowFromPoint` / `GetForegroundWindow`, then `GetAncestor(GA_ROOT)`
//!   and `GA_ROOTOWNER`, and `GetWindowThreadProcessId` for each. The application is the
//!   top-level window's process (for a UWP app hosted by ApplicationFrameHost, the
//!   hosted `CoreWindow`'s process).
//! - **Elevation:** `OpenProcess` + `OpenProcessToken` + `TokenElevation`. Windows drops
//!   synthetic input into elevated windows without an error (UIPI), so when this app is
//!   not elevated, a target that is, or whose process cannot be opened, is refused.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmFlush, DwmGetWindowAttribute};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, VIRTUAL_KEY, VK_CONTROL,
    VK_LBUTTON, VK_LWIN, VK_MBUTTON, VK_MENU, VK_RBUTTON, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, GA_ROOT, GA_ROOTOWNER, GetAncestor, GetClassNameW, GetCursorPos,
    GetForegroundWindow, GetSystemMetrics, GetWindowDisplayAffinity, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_SWAPBUTTON, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SetCursorPos, SetWindowDisplayAffinity,
    WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WindowFromPoint,
};
use windows::core::{BOOL, PWSTR, w};

use crate::apps::{AppInfo, Target, is_console_window_class};
use crate::geometry::{GlobalPoint, MonitorInfo, Rect, virtual_desk_absolute};
use crate::keys::Key;
use crate::platform::{
    Backends, Capture, CaptureError, Captured, Gate, Injector, InputError, InputEvent,
    MAIN_THREAD_TIMEOUT, MainThread, MouseButton, PlatformError, Pressable, SelfProcess,
    run_on_main, run_on_main_even_late,
};

/// `dwExtraInfo` on every event this app injects, so its own keyboard hooks can tell.
const EVENT_MARKER: usize = 0x4852_4E53;
/// One wheel notch.
const WHEEL_DELTA: i32 = 120;

pub(super) fn native(
    main: Arc<dyn MainThread>,
    me: SelfProcess,
) -> Result<Backends, PlatformError> {
    let self_elevated = process_elevated(std::process::id()).unwrap_or(false);
    let capture = WinCapture {
        main: main.clone(),
        pid: me.pid(),
    };
    let injector = WinInjector {
        main,
        me,
        self_elevated,
        send_lock: Mutex::new(()),
    };
    Ok((Arc::new(capture), Arc::new(injector)))
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful Open* call and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Full image path of a process, or `None` when it cannot be opened.
fn process_path(pid: u32) -> Option<String> {
    // SAFETY: plain Win32 calls with valid arguments; the buffer outlives the call and its
    // length is passed in and out.
    unsafe {
        let h = Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
            .ok()?;
        Some(String::from_utf16_lossy(
            &buf[..(len as usize).min(buf.len())],
        ))
    }
}

/// Whether a process runs elevated; `None` when it cannot be opened or queried.
fn process_elevated(pid: u32) -> Option<bool> {
    // SAFETY: as above; TOKEN_ELEVATION is a plain u32 struct of the size passed.
    unsafe {
        let h = Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut token = HANDLE::default();
        OpenProcessToken(h.0, TOKEN_QUERY, &mut token).ok()?;
        let token = Handle(token);
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        GetTokenInformation(
            token.0,
            TokenElevation,
            Some(&mut e as *mut TOKEN_ELEVATION as *mut c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .ok()?;
        Some(e.TokenIsElevated != 0)
    }
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: hwnd may be stale; the call then returns 0 and leaves pid 0.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn window_title(hwnd: HWND) -> Option<String> {
    let mut buf = [0u16; 512];
    // SAFETY: the buffer is valid for its length. For another process's window this reads
    // the title without sending a message, so a hung window cannot block us.
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    (n > 0).then(|| String::from_utf16_lossy(&buf[..(n as usize).min(buf.len())]))
}

fn window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: the buffer is valid for its length; a stale handle returns 0.
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..(n.max(0) as usize).min(buf.len())])
}

fn window_rect(hwnd: HWND) -> Option<Rect> {
    let mut r = RECT::default();
    // SAFETY: r is a valid RECT of the size passed.
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok()
            || GetWindowRect(hwnd, &mut r).is_ok()
    };
    ok.then(|| {
        Rect::new(
            f64::from(r.left),
            f64::from(r.top),
            f64::from(r.right - r.left),
            f64::from(r.bottom - r.top),
        )
    })
}

fn is_frame_host(path: &str) -> bool {
    path.to_ascii_lowercase()
        .ends_with("\\applicationframehost.exe")
}

struct WinInjector {
    main: Arc<dyn MainThread>,
    me: SelfProcess,
    self_elevated: bool,
    /// Serialises the gate check and `SendInput` for pointer events with Stop's releases.
    send_lock: Mutex<()>,
}

impl WinInjector {
    fn target_for(&self, hwnd: HWND) -> Result<Target, InputError> {
        if hwnd.is_invalid() {
            return Err(InputError::NoTarget("no window is there right now".into()));
        }
        // SAFETY: GetAncestor accepts any handle and returns null for a stale one.
        let (root, owner) =
            unsafe { (GetAncestor(hwnd, GA_ROOT), GetAncestor(hwnd, GA_ROOTOWNER)) };
        let mut pids = vec![window_pid(hwnd)];
        for w in [root, owner] {
            if !w.is_invalid() {
                let p = window_pid(w);
                if !pids.contains(&p) {
                    pids.push(p);
                }
            }
        }
        let top = if root.is_invalid() { hwnd } else { root };
        // This app's own windows: decided before anything else is asked of them (reading a
        // title from our own window would send a message to our UI thread).
        if self.me.owns_any(&pids) {
            return Ok(Target {
                pids,
                app: AppInfo {
                    id: "this-app".into(),
                    name: "this app".into(),
                    exe_name: String::new(),
                    bundle_id: None,
                },
                elevated: false,
                window_title: None,
                window_id: top.0 as usize as u64,
                window_rect: None,
                console: false,
            });
        }
        let mut ident_pid = window_pid(top);
        let mut path = process_path(ident_pid);
        if path.as_deref().is_some_and(is_frame_host) {
            // A UWP app hosted by ApplicationFrameHost: the hosted CoreWindow's process is
            // the application.
            // SAFETY: FindWindowExW with a valid parent and class name.
            if let Ok(core) =
                unsafe { FindWindowExW(Some(top), None, w!("Windows.UI.Core.CoreWindow"), None) }
                && !core.is_invalid()
            {
                let p = window_pid(core);
                if p != 0 {
                    if !pids.contains(&p) {
                        pids.push(p);
                    }
                    if self.me.owns_any(&[p]) {
                        return self.target_for(core);
                    }
                    ident_pid = p;
                    path = process_path(p);
                }
            }
        }
        let app = match &path {
            Some(p) => AppInfo::from_windows_path(p),
            None => AppInfo {
                id: format!("pid:{ident_pid}"),
                name: format!("process {ident_pid}"),
                exe_name: String::new(),
                bundle_id: None,
            },
        };
        let elevated =
            !self.self_elevated && pids.iter().any(|p| process_elevated(*p).unwrap_or(true));
        Ok(Target {
            pids,
            app,
            elevated,
            window_title: window_title(top),
            window_id: top.0 as usize as u64,
            window_rect: window_rect(top),
            // A console window is reported as owned by the program running in it, so its
            // class is what shows it is a shell.
            console: is_console_window_class(&window_class(top)),
        })
    }

    fn send_mouse(
        &self,
        flags: MOUSE_EVENT_FLAGS,
        dx: i32,
        dy: i32,
        data: i32,
    ) -> Result<(), InputError> {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data as u32,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: EVENT_MARKER,
                },
            },
        };
        // SAFETY: one fully initialised INPUT and its exact size.
        let n = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
        if n == 1 {
            Ok(())
        } else {
            Err(InputError::Platform(format!(
                "SendInput was refused ({})",
                windows::core::Error::from_thread()
            )))
        }
    }

    fn physical(button: MouseButton) -> MouseButton {
        // SAFETY: GetSystemMetrics has no preconditions.
        let swapped = unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0;
        match (button, swapped) {
            (MouseButton::Left, true) => MouseButton::Right,
            (MouseButton::Right, true) => MouseButton::Left,
            (b, _) => b,
        }
    }

    fn move_to(&self, to: GlobalPoint) -> Result<(), InputError> {
        let (px, py) = to.device();
        // SAFETY: GetSystemMetrics has no preconditions.
        let desk = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        let (nx, ny) = virtual_desk_absolute(px, py, desk);
        self.send_mouse(
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            nx,
            ny,
            0,
        )?;
        // Read back; correct a one-pixel rounding difference (or a move the input thread
        // has not processed yet, which then lands on the same pixel).
        let mut pt = POINT::default();
        // SAFETY: pt is a valid POINT.
        if unsafe { GetCursorPos(&mut pt) }.is_ok() && (pt.x, pt.y) != (px, py) {
            // SAFETY: plain call; fails harmlessly off every monitor.
            let _ = unsafe { SetCursorPos(px, py) };
        }
        Ok(())
    }

    fn pointer(&self, event: &InputEvent, gate: &Gate) -> Result<(), InputError> {
        let _serial = self.send_lock.lock().unwrap_or_else(|e| e.into_inner());
        if !gate.is_open() {
            return Err(InputError::Stopped);
        }
        match event {
            InputEvent::Move { to } => self.move_to(*to),
            InputEvent::Button { button, down } => {
                let flags = match (Self::physical(*button), down) {
                    (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
                    (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
                    (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
                    (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
                    (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
                    (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
                };
                self.send_mouse(flags, 0, 0, 0)
            }
            InputEvent::Scroll { dx, dy } => {
                if *dy != 0 {
                    // Positive wheel data scrolls up (away from the user).
                    self.send_mouse(MOUSEEVENTF_WHEEL, 0, 0, -dy * WHEEL_DELTA)?;
                }
                if *dx != 0 {
                    self.send_mouse(MOUSEEVENTF_HWHEEL, 0, 0, dx * WHEEL_DELTA)?;
                }
                Ok(())
            }
            _ => Err(InputError::Platform("not a pointer event".into())),
        }
    }

    fn keyboard(&self, event: &InputEvent, gate: &Gate) -> Result<(), InputError> {
        let release = event.is_release();
        let event = event.clone();
        let gate = gate.clone();
        let step = move || {
            // Checked on the main thread, right before enigo sends, so an event queued
            // before Stop cannot land after Stop's releases.
            if !gate.is_open() {
                return Err(InputError::Stopped);
            }
            with_enigo(|e| enigo_keyboard(e, &event))
        };
        // A press the main thread did not get to in time is abandoned (never sent late);
        // a release still goes out late.
        if release {
            run_on_main_even_late(&*self.main, MAIN_THREAD_TIMEOUT, step)?
        } else {
            run_on_main(&*self.main, MAIN_THREAD_TIMEOUT, step)?
        }
    }
}

thread_local! {
    /// The main thread's enigo, created on first use.
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
                windows_dw_extra_info: Some(EVENT_MARKER),
                ..Default::default()
            };
            let e = enigo::Enigo::new(&settings)
                .map_err(|e| InputError::Platform(format!("keyboard input is unavailable: {e}")))?;
            *slot = Some(e);
        }
        match slot.as_mut() {
            Some(e) => f(e),
            None => Err(InputError::Platform("keyboard input is unavailable".into())),
        }
    })
}

fn enigo_key(k: Key) -> Result<enigo::Key, InputError> {
    use enigo::Key as E;
    Ok(match k {
        Key::Shift => E::Shift,
        Key::Control => E::Control,
        Key::Alt => E::Alt,
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
        Key::Insert => E::Insert,
        Key::CapsLock => E::CapsLock,
        Key::PrintScreen => E::PrintScr,
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

fn enigo_keyboard(e: &mut enigo::Enigo, event: &InputEvent) -> Result<(), InputError> {
    use enigo::{Direction, Keyboard};
    let r = match event {
        InputEvent::Key { key, down } => {
            let dir = if *down {
                Direction::Press
            } else {
                Direction::Release
            };
            e.key(enigo_key(*key)?, dir)
        }
        InputEvent::Text { text } => e.text(text),
        _ => return Err(InputError::Platform("not a keyboard event".into())),
    };
    r.map_err(|err| InputError::Platform(err.to_string()))
}

impl Injector for WinInjector {
    fn target_at(&self, p: GlobalPoint) -> Result<Target, InputError> {
        let (x, y) = p.device();
        // SAFETY: plain call.
        let hwnd = unsafe { WindowFromPoint(POINT { x, y }) };
        self.target_for(hwnd)
    }

    fn keyboard_target(&self) -> Result<Target, InputError> {
        // SAFETY: plain call.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_invalid() {
            return Err(InputError::NoTarget(
                "no window has the keyboard focus right now".into(),
            ));
        }
        self.target_for(hwnd)
    }

    fn send(&self, event: &InputEvent, gate: &Gate) -> Result<(), InputError> {
        match event {
            InputEvent::Move { .. } | InputEvent::Button { .. } | InputEvent::Scroll { .. } => {
                self.pointer(event, gate)
            }
            InputEvent::Key { .. } | InputEvent::Text { .. } => self.keyboard(event, gate),
        }
    }

    fn is_pressed(&self, what: Pressable) -> Option<bool> {
        let down = |vk: VIRTUAL_KEY| -> bool {
            // SAFETY: plain call. The high bit is set while the key is down; injected input
            // counts.
            (unsafe { GetAsyncKeyState(i32::from(vk.0)) } as u16 & 0x8000) != 0
        };
        Some(match what {
            Pressable::Button(b) => down(match Self::physical(b) {
                MouseButton::Left => VK_LBUTTON,
                MouseButton::Right => VK_RBUTTON,
                MouseButton::Middle => VK_MBUTTON,
            }),
            Pressable::Key(Key::Shift) => down(VK_SHIFT),
            Pressable::Key(Key::Control) => down(VK_CONTROL),
            Pressable::Key(Key::Alt) => down(VK_MENU),
            Pressable::Key(Key::Meta) => down(VK_LWIN) || down(VK_RWIN),
            Pressable::Key(_) => return None,
        })
    }

    fn mac_keys(&self) -> bool {
        false
    }
}

struct WinCapture {
    main: Arc<dyn MainThread>,
    pid: u32,
}

struct Collect {
    pid: u32,
    out: Vec<isize>,
}

unsafe extern "system" fn collect_own(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the &mut Collect passed to EnumWindows below, alive for the call.
    let ctx = unsafe { &mut *(lparam.0 as *mut Collect) };
    // SAFETY: plain call on the enumerated handle.
    if window_pid(hwnd) == ctx.pid && unsafe { IsWindowVisible(hwnd) }.as_bool() {
        ctx.out.push(hwnd.0 as isize);
    }
    BOOL(1)
}

fn own_windows(pid: u32) -> Vec<isize> {
    let mut ctx = Collect {
        pid,
        out: Vec::new(),
    };
    // SAFETY: the callback only touches ctx, which outlives the call.
    let _ = unsafe { EnumWindows(Some(collect_own), LPARAM(&mut ctx as *mut Collect as isize)) };
    ctx.out
}

impl WinCapture {
    /// Gives this app's visible windows `WDA_EXCLUDEFROMCAPTURE` on the main thread.
    /// Returns (every window is excluded, the handles changed, to restore later).
    fn exclude_own(&self) -> (bool, Arc<Mutex<Vec<isize>>>) {
        let changed: Arc<Mutex<Vec<isize>>> = Arc::new(Mutex::new(Vec::new()));
        let pid = self.pid;
        let record = changed.clone();
        let all = run_on_main(&*self.main, MAIN_THREAD_TIMEOUT, move || {
            let mut all = true;
            for h in own_windows(pid) {
                let hwnd = HWND(h as *mut c_void);
                let mut cur = 0u32;
                // SAFETY: plain calls on our own window handles.
                let ok = unsafe { GetWindowDisplayAffinity(hwnd, &mut cur) }.is_ok();
                if ok && cur == WDA_EXCLUDEFROMCAPTURE.0 {
                    continue;
                }
                if ok
                    && cur == WDA_NONE.0
                    && unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) }.is_ok()
                {
                    record.lock().unwrap_or_else(|e| e.into_inner()).push(h);
                } else {
                    all = false;
                }
            }
            all
        })
        .unwrap_or(false);
        (all, changed)
    }

    /// Restores what [`Self::exclude_own`] changed. Queued on the main thread after the
    /// exclusion, so it also undoes an exclusion that finished after we stopped waiting.
    fn restore_own(&self, changed: Arc<Mutex<Vec<isize>>>) {
        self.main.run(Box::new(move || {
            for h in changed.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
                // SAFETY: plain call on our own window handle (fails harmlessly if gone).
                let _ = unsafe { SetWindowDisplayAffinity(HWND(h as *mut c_void), WDA_NONE) };
            }
        }));
    }
}

impl Capture for WinCapture {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        let all = xcap::Monitor::all().map_err(|e| CaptureError::Platform(e.to_string()))?;
        let mut out = Vec::with_capacity(all.len());
        for m in all {
            let get = |r: Result<i64, xcap::XCapError>| {
                r.map_err(|e| CaptureError::Platform(e.to_string()))
            };
            let x = get(m.x().map(i64::from))?;
            let y = get(m.y().map(i64::from))?;
            let w = get(m.width().map(i64::from))?;
            let h = get(m.height().map(i64::from))?;
            out.push(MonitorInfo {
                id: m.id().map_err(|e| CaptureError::Platform(e.to_string()))?,
                name: m
                    .friendly_name()
                    .or_else(|_| m.name())
                    .unwrap_or_else(|_| "monitor".into()),
                bounds: Rect::new(x as f64, y as f64, w as f64, h as f64),
                pixels_per_unit: 1.0,
                ui_scale: f64::from(m.scale_factor().unwrap_or(1.0)),
                primary: m.is_primary().unwrap_or(false),
            });
        }
        Ok(out)
    }

    fn capture(&self, monitor: &MonitorInfo) -> Result<Captured, CaptureError> {
        let all = xcap::Monitor::all().map_err(|e| CaptureError::Platform(e.to_string()))?;
        let m = all
            .into_iter()
            .find(|m| m.id().ok() == Some(monitor.id))
            .ok_or(CaptureError::NoMonitor)?;
        let (excluded, changed) = self.exclude_own();
        // Let DWM compose a frame without our windows before reading the screen.
        // SAFETY: plain calls.
        unsafe {
            let _ = DwmFlush();
            let _ = DwmFlush();
        }
        let shot = m.capture_image();
        self.restore_own(changed);
        let img = shot.map_err(|e| CaptureError::Platform(e.to_string()))?;
        let (width, height) = img.dimensions();
        Ok(Captured {
            width,
            height,
            rgba: img.into_raw(),
            self_excluded: excluded,
            note: (!excluded).then(|| {
                "Some of this app's windows could not be hidden from the screenshot (Windows 10 2004 or later is needed).".to_string()
            }),
        })
    }

    fn foreground_window_rect(&self) -> Option<Rect> {
        // SAFETY: plain call.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_invalid() {
            return None;
        }
        window_rect(hwnd)
    }

    fn cursor_position(&self) -> Option<GlobalPoint> {
        let mut pt = POINT::default();
        // SAFETY: pt is a valid POINT.
        unsafe { GetCursorPos(&mut pt) }
            .ok()
            .map(|_| GlobalPoint::new(f64::from(pt.x), f64::from(pt.y)))
    }
}
