//! A fake desktop for tests, on every OS: monitors, windows in z-order, a cursor and the
//! OS's idea of which keys and buttons are down. [`FakeDesktop::capture`] and
//! [`FakeDesktop::injector`] implement the platform traits on top of it, record every
//! event, and can be told to fail, stall or panic on a given event.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::apps::{AppInfo, Target};
use crate::geometry::{GlobalPoint, MonitorInfo, Rect};
use crate::keys::Key;
use crate::platform::{
    Capture, CaptureError, Captured, Gate, Injector, InputError, InputEvent, MouseButton, Pressable,
};

/// A window on the fake desktop.
#[derive(Debug, Clone)]
pub struct FakeWindow {
    pub id: u64,
    pub rect: Rect,
    /// Owner of the window itself.
    pub pid: u32,
    /// Owner of its top-level window, when different (a WebView2 child window belongs to
    /// a browser process; its top-level window to the app).
    pub root_pid: Option<u32>,
    pub app: AppInfo,
    pub title: String,
    pub elevated: bool,
    /// A console window (see [`Target::console`]).
    pub console: bool,
    /// RGB used when the fake capture draws it.
    pub colour: [u8; 3],
}

impl FakeWindow {
    pub fn new(id: u64, rect: Rect, pid: u32, app: AppInfo) -> Self {
        let c = (id as u8).wrapping_mul(53);
        Self {
            id,
            rect,
            pid,
            root_pid: None,
            app,
            title: format!("window {id}"),
            elevated: false,
            console: false,
            colour: [c, c.wrapping_add(90), c.wrapping_add(180)],
        }
    }
}

/// What to do on a given event (counted from 0 over all `send` calls).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    Fail,
    Panic,
}

#[derive(Debug, Default)]
pub struct FakeState {
    pub monitors: Vec<MonitorInfo>,
    /// Front to back.
    pub windows: Vec<FakeWindow>,
    /// Window with keyboard focus.
    pub foreground: Option<u64>,
    pub cursor: GlobalPoint,
    /// Every event that was sent, in order.
    pub events: Vec<InputEvent>,
    /// Events refused because their gate was closed.
    pub gated: usize,
    pub keys_down: BTreeSet<Key>,
    pub buttons_down: BTreeSet<MouseButton>,
    /// Number of `send` calls so far (sent, refused or faulted).
    pub sends: usize,
    pub fault: Option<(usize, Fault)>,
    /// Sleep this long in every `send` (to test Stop during an operation).
    pub send_delay: Option<Duration>,
    /// Pids the capture leaves out (this app).
    pub capture_excludes: Vec<u32>,
    pub capture_error: Option<CaptureError>,
    /// Sleep this long in every capture (to test overlapping captures).
    pub capture_delay: Option<Duration>,
    /// Captures running right now, and the most that ever ran at once.
    pub captures_in_flight: usize,
    pub max_captures_in_flight: usize,
    /// Report OS key/button state (`is_pressed`), as Windows and macOS can.
    pub reports_state: bool,
    pub mac: bool,
}

/// Shared fake desktop. Clone it freely.
#[derive(Debug, Clone, Default)]
pub struct FakeDesktop(Arc<Mutex<FakeState>>);

impl FakeDesktop {
    pub fn new(monitors: Vec<MonitorInfo>) -> Self {
        let d = Self::default();
        d.state().monitors = monitors;
        d.state().reports_state = true;
        d
    }

    /// Locks the state (a poisoned lock from a test panic is still usable).
    pub fn state(&self) -> MutexGuard<'_, FakeState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds a window in front of the others and gives it the keyboard focus.
    pub fn push_front(&self, w: FakeWindow) {
        let mut s = self.state();
        s.foreground = Some(w.id);
        s.windows.insert(0, w);
    }

    pub fn events(&self) -> Vec<InputEvent> {
        self.state().events.clone()
    }

    pub fn clear_events(&self) {
        self.state().events.clear();
    }

    pub fn capture(&self) -> Arc<dyn Capture> {
        Arc::new(FakeCapture(self.clone()))
    }

    pub fn injector(&self) -> Arc<dyn Injector> {
        Arc::new(FakeInjector(self.clone()))
    }
}

fn desktop_app() -> AppInfo {
    AppInfo {
        id: "desktop".into(),
        name: "Desktop".into(),
        exe_name: "desktop".into(),
        bundle_id: None,
    }
}

fn target_of(w: &FakeWindow) -> Target {
    let mut pids = vec![w.pid];
    if let Some(r) = w.root_pid {
        pids.push(r);
    }
    Target {
        pids,
        app: w.app.clone(),
        elevated: w.elevated,
        window_title: Some(w.title.clone()),
        window_id: w.id,
        window_rect: Some(w.rect),
        console: w.console,
    }
}

struct FakeCapture(FakeDesktop);

impl Capture for FakeCapture {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        Ok(self.0.state().monitors.clone())
    }

    fn capture(&self, monitor: &MonitorInfo) -> Result<Captured, CaptureError> {
        let delay = {
            let mut s = self.0.state();
            s.captures_in_flight += 1;
            s.max_captures_in_flight = s.max_captures_in_flight.max(s.captures_in_flight);
            s.capture_delay
        };
        if let Some(d) = delay {
            std::thread::sleep(d);
        }
        let r = self.capture_now(monitor);
        self.0.state().captures_in_flight -= 1;
        r
    }

    fn foreground_window_rect(&self) -> Option<Rect> {
        let s = self.0.state();
        let id = s.foreground?;
        s.windows.iter().find(|w| w.id == id).map(|w| w.rect)
    }

    fn cursor_position(&self) -> Option<GlobalPoint> {
        Some(self.0.state().cursor)
    }
}

impl FakeCapture {
    fn capture_now(&self, monitor: &MonitorInfo) -> Result<Captured, CaptureError> {
        let s = self.0.state();
        if let Some(e) = &s.capture_error {
            return Err(e.clone());
        }
        if !s.monitors.iter().any(|m| m.id == monitor.id) {
            return Err(CaptureError::NoMonitor);
        }
        let (w, h) = monitor.pixel_size();
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for px in rgba.chunks_exact_mut(4) {
            px.copy_from_slice(&[20, 20, 20, 255]);
        }
        // Back to front, skipping this app's windows.
        for win in s.windows.iter().rev() {
            if s.capture_excludes.contains(&win.pid)
                || win
                    .root_pid
                    .is_some_and(|p| s.capture_excludes.contains(&p))
            {
                continue;
            }
            let ppu = monitor.pixels_per_unit;
            let x0 = ((win.rect.x - monitor.bounds.x) * ppu).max(0.0) as u32;
            let y0 = ((win.rect.y - monitor.bounds.y) * ppu).max(0.0) as u32;
            let x1 =
                (((win.rect.x + win.rect.width - monitor.bounds.x) * ppu).max(0.0) as u32).min(w);
            let y1 =
                (((win.rect.y + win.rect.height - monitor.bounds.y) * ppu).max(0.0) as u32).min(h);
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = ((y * w + x) * 4) as usize;
                    rgba[i..i + 3].copy_from_slice(&win.colour);
                }
            }
        }
        Ok(Captured {
            width: w,
            height: h,
            rgba,
            self_excluded: true,
            note: None,
        })
    }
}

struct FakeInjector(FakeDesktop);

impl Injector for FakeInjector {
    fn target_at(&self, p: GlobalPoint) -> Result<Target, InputError> {
        let s = self.0.state();
        if !s.monitors.iter().any(|m| m.bounds.contains(p)) {
            return Err(InputError::NoTarget(
                "that point is not on any monitor".into(),
            ));
        }
        Ok(s.windows
            .iter()
            .find(|w| w.rect.contains(p))
            .map(target_of)
            .unwrap_or_else(|| Target {
                pids: vec![1],
                app: desktop_app(),
                elevated: false,
                window_title: None,
                window_id: 0,
                window_rect: None,
                console: false,
            }))
    }

    fn keyboard_target(&self) -> Result<Target, InputError> {
        let s = self.0.state();
        let id = s
            .foreground
            .ok_or_else(|| InputError::NoTarget("no window has the keyboard focus".into()))?;
        s.windows
            .iter()
            .find(|w| w.id == id)
            .map(target_of)
            .ok_or_else(|| InputError::NoTarget("no window has the keyboard focus".into()))
    }

    fn send(&self, event: &InputEvent, gate: &Gate) -> Result<(), InputError> {
        let delay = self.0.state().send_delay;
        if let Some(d) = delay {
            std::thread::sleep(d);
        }
        // Gate check and "send" in one locked step, as the native injectors do.
        let mut s = self.0.state();
        let n = s.sends;
        s.sends += 1;
        if !gate.is_open() {
            s.gated += 1;
            return Err(InputError::Stopped);
        }
        match s.fault {
            Some((at, Fault::Fail)) if at == n => {
                return Err(InputError::Platform("injected failure".into()));
            }
            Some((at, Fault::Panic)) if at == n => {
                drop(s);
                panic!("injected panic in the fake injector");
            }
            _ => {}
        }
        match event {
            InputEvent::Move { to } => s.cursor = *to,
            InputEvent::Button { button, down } => {
                if *down {
                    s.buttons_down.insert(*button);
                } else {
                    s.buttons_down.remove(button);
                }
            }
            InputEvent::Key { key, down } => {
                if *down {
                    s.keys_down.insert(*key);
                } else {
                    s.keys_down.remove(key);
                }
            }
            InputEvent::Scroll { .. } | InputEvent::Text { .. } => {}
        }
        s.events.push(event.clone());
        Ok(())
    }

    fn is_pressed(&self, what: Pressable) -> Option<bool> {
        let s = self.0.state();
        if !s.reports_state {
            return None;
        }
        Some(match what {
            Pressable::Key(k) => s.keys_down.contains(&k),
            Pressable::Button(b) => s.buttons_down.contains(&b),
        })
    }

    fn mac_keys(&self) -> bool {
        self.0.state().mac
    }
}
