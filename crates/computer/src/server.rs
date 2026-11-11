//! The built-in `computer` server (ARCHITECTURE.md 9.3 "Computer use", 13.3, D7):
//! `computer_screenshot`, `computer_click`, `computer_move`, `computer_drag`,
//! `computer_scroll`, `computer_type`, `computer_key`.
//!
//! - `describe` maps the model's coordinates through the chat's latest screenshot, finds
//!   the application the input would reach and returns the action: `Scope::Screenshots`
//!   (read-only) for screenshots, `Scope::ComputerInput { kind, app }` for input, where
//!   `app` is the application the input goes to (under the point for the pointer, in
//!   front for the keyboard). Input to this app, to an elevated window or to the
//!   overlay's Stop button is a hard limit. Terminals and console windows, settings,
//!   password managers, remote-desktop windows, applications that cannot be identified,
//!   typing into the Windows shell (the Run box, Explorer's address bar, Start's search), a
//!   drag between two applications and shortcuts the OS handles itself always ask.
//! - `call` runs the approved action from its payload (not from the arguments), re-checks
//!   the screen layout, and goes through the [`InputGuard`], which re-checks the target
//!   application before every single event, and refuses a target that always asks unless
//!   the user approved this action on an always-ask card.
//!
//! Screenshots are returned as plain tool output; what they show is untrusted data and
//! the agent loop treats it as such.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use mcp_host::{BuiltinServer, CallContext, ChatContext, Content, Described, ToolDef, ToolOutput};
use policy::{Action, Risk, Scope};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::apps::{Target, target_always_ask_reason};
use crate::geometry::{
    GlobalPoint, MonitorInfo, ScreenshotGeometry, monitor_for_window, order_monitors,
};
use crate::guard::{Expect, Held, InputGuard, OpError, StopHandle, refuse_point, refuse_target};
use crate::image_ops;
use crate::keys::{Key, KeyCombo, TextPart, parse_combo, system_shortcut, text_events};
use crate::platform::{Capture, CaptureError, Injector, InputError, MouseButton, SelfProcess};
use crate::profile::ModelProfile;

pub const SERVER_NAME: &str = "computer";
pub const TOOL_SCREENSHOT: &str = "computer_screenshot";
pub const TOOL_CLICK: &str = "computer_click";
pub const TOOL_MOVE: &str = "computer_move";
pub const TOOL_DRAG: &str = "computer_drag";
pub const TOOL_SCROLL: &str = "computer_scroll";
pub const TOOL_TYPE: &str = "computer_type";
pub const TOOL_KEY: &str = "computer_key";

/// Longest text one `computer_type` call types.
pub const MAX_TYPE_CHARS: usize = 4000;
/// Characters typed per event; the target is re-checked before each.
const TYPE_CHUNK: usize = 8;
/// Holds and drags are capped (4.2).
pub const MAX_HOLD_MS: u64 = 2000;
/// Time for the screen to settle after input, before the call returns.
const SETTLE: Duration = Duration::from_millis(150);

/// Called before every input action. `Err(reason)` refuses the action: the overlay is not
/// visible on the active Space, the stop hotkey is not registered, …
pub type ReadyCheck = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

/// Settings for [`ComputerServer`].
#[derive(Clone, Default)]
pub struct ComputerConfig {
    /// Used for chats without their own profile ([`ComputerServer::set_profile`]).
    pub default_profile: ModelProfile,
    pub ready_check: Option<ReadyCheck>,
}

impl std::fmt::Debug for ComputerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComputerConfig")
            .field("default_profile", &self.default_profile)
            .field("ready_check", &self.ready_check.is_some())
            .finish()
    }
}

#[derive(Debug, Default)]
struct ChatState {
    profile: Option<ModelProfile>,
    last: Option<ScreenshotGeometry>,
}

#[derive(Debug, Default)]
struct State {
    next_id: u64,
    chats: HashMap<i64, ChatState>,
}

/// The `computer` server.
pub struct ComputerServer {
    capture: Arc<dyn Capture>,
    guard: Arc<InputGuard>,
    config: ComputerConfig,
    state: Mutex<State>,
    /// One input operation at a time; held by the blocking task until it finishes.
    op_lock: Arc<tokio::sync::Mutex<()>>,
    /// One capture at a time. On Windows a capture hides this app's windows and shows
    /// them again afterwards; two overlapping captures would show them in the middle of
    /// the other one.
    capture_lock: Arc<Mutex<()>>,
}

impl std::fmt::Debug for ComputerServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComputerServer")
            .field("config", &self.config)
            .field("guard", &self.guard)
            .finish_non_exhaustive()
    }
}

/// What an input call does, computed by `describe`, shown on the card and run by `call`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InputPlan {
    /// Applications the input may reach (one, or two for a drag between applications).
    apps: Vec<String>,
    /// The monitor of the screenshot the coordinates refer to, as it was then.
    monitor: Option<MonitorInfo>,
    /// The model's own coordinates, for messages.
    shown: String,
    step: Step,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
enum Step {
    Click {
        at: GlobalPoint,
        button: MouseButton,
        clicks: u32,
        modifiers: Vec<Key>,
    },
    Move {
        at: GlobalPoint,
    },
    Drag {
        from: GlobalPoint,
        to: GlobalPoint,
        button: MouseButton,
        duration_ms: u64,
    },
    Scroll {
        at: GlobalPoint,
        dx: i32,
        dy: i32,
    },
    Type {
        text: String,
    },
    Key {
        combo: KeyCombo,
        repeat: u32,
    },
}

fn s<'a>(args: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    args.get(k).and_then(Value::as_str)
}

fn u(args: &Map<String, Value>, k: &str) -> Option<u64> {
    args.get(k).and_then(Value::as_u64)
}

fn coord(args: &Map<String, Value>, k: &str) -> Result<Vec<f64>, String> {
    let arr = args
        .get(k)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{k} is required: [x, y] or a box [x1, y1, x2, y2]"))?;
    arr.iter()
        .map(|v| {
            v.as_f64()
                .ok_or_else(|| format!("{k} must contain numbers only"))
        })
        .collect()
}

fn fmt_coord(c: &[f64]) -> String {
    let parts: Vec<String> = c
        .iter()
        .map(|v| {
            if v.fract() == 0.0 && v.abs() < 1e9 {
                format!("{}", *v as i64)
            } else {
                format!("{v:.1}")
            }
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

fn fmt_point(p: GlobalPoint) -> String {
    let (x, y) = (p.x, p.y);
    if x.fract() == 0.0 && y.fract() == 0.0 {
        format!("({}, {})", x as i64, y as i64)
    } else {
        format!("({x:.1}, {y:.1})")
    }
}

fn monitor_line(m: &MonitorInfo, index: usize, count: usize) -> String {
    let scale = if (m.ui_scale - 1.0).abs() > 0.01 {
        format!(", {}% scaling", (m.ui_scale * 100.0).round() as i64)
    } else {
        String::new()
    };
    format!(
        "monitor {} of {} ({}{}, {}x{}{})",
        index + 1,
        count,
        policy::sanitize::one_line(&m.name, 60),
        if m.primary { ", primary" } else { "" },
        m.bounds.width.round() as i64,
        m.bounds.height.round() as i64,
        scale
    )
}

fn button_arg(args: &Map<String, Value>) -> Result<MouseButton, String> {
    match s(args, "button").unwrap_or("left") {
        "left" => Ok(MouseButton::Left),
        "right" => Ok(MouseButton::Right),
        "middle" => Ok(MouseButton::Middle),
        other => Err(format!(
            "button must be left, right or middle, not {}",
            policy::sanitize::one_line(other, 20)
        )),
    }
}

fn input_error_text(e: &InputError) -> String {
    e.to_string()
}

fn op_output(r: Result<String, OpError>) -> ToolOutput {
    match r {
        Ok(text) => ToolOutput::text(text),
        Err(e) => ToolOutput::error(e.to_string()),
    }
}

impl ComputerServer {
    /// A server over any capture and injector (the native ones, or the fakes in tests).
    pub fn new(
        capture: Arc<dyn Capture>,
        injector: Arc<dyn Injector>,
        me: SelfProcess,
        config: ComputerConfig,
    ) -> Self {
        Self {
            capture,
            guard: Arc::new(InputGuard::new(injector, me)),
            config,
            state: Mutex::new(State::default()),
            op_lock: Arc::new(tokio::sync::Mutex::new(())),
            capture_lock: Arc::new(Mutex::new(())),
        }
    }

    /// The server over this OS's native capture and input.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub fn native(
        main: Arc<dyn crate::platform::MainThread>,
        me: SelfProcess,
        config: ComputerConfig,
    ) -> Result<Self, crate::platform::PlatformError> {
        let (capture, injector) = crate::platform::native(main, me.clone())?;
        Ok(Self::new(capture, injector, me, config))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sets the model profile for a chat (call before each turn, with the chat's model).
    pub fn set_profile(&self, chat_id: i64, profile: ModelProfile) {
        let mut st = self.state();
        let chat = st.chats.entry(chat_id).or_default();
        if chat.profile != Some(profile) {
            // Coordinates of an older screenshot no longer mean the same thing.
            chat.last = None;
        }
        chat.profile = Some(profile);
    }

    /// Forgets a chat's profile and latest screenshot.
    pub fn forget_chat(&self, chat_id: i64) {
        self.state().chats.remove(&chat_id);
    }

    /// The latest screenshot's geometry for a chat (for the calibration test).
    pub fn last_screenshot(&self, chat_id: i64) -> Option<ScreenshotGeometry> {
        self.state()
            .chats
            .get(&chat_id)
            .and_then(|c| c.last.clone())
    }

    fn profile(&self, chat_id: i64) -> ModelProfile {
        self.state()
            .chats
            .get(&chat_id)
            .and_then(|c| c.profile)
            .unwrap_or(self.config.default_profile)
    }

    /// For the stop hotkey, the overlay's Stop button and the panic hook.
    pub fn stop_handle(&self) -> StopHandle {
        self.guard.stop_handle()
    }

    /// Stop: releases every key and mouse button. Never blocks.
    pub fn release_all(&self) {
        self.guard.stop_handle().release_all();
    }

    /// Releases everything and waits. Call at `RunEvent::Exit` (Tauri exits the process
    /// without running destructors).
    pub fn shutdown(&self) {
        self.guard.shutdown();
    }

    /// Keys and buttons currently held by the injector.
    pub fn held(&self) -> Held {
        self.guard.held()
    }

    async fn blocking<T: Send + 'static>(
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, String> {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| format!("internal error: {e}"))
    }

    async fn monitors(&self) -> Result<Vec<MonitorInfo>, String> {
        let cap = self.capture.clone();
        let mut ms = Self::blocking(move || cap.monitors())
            .await?
            .map_err(|e| e.to_string())?;
        if ms.is_empty() {
            return Err("no monitor was found".into());
        }
        order_monitors(&mut ms);
        Ok(ms)
    }

    async fn target_at(&self, p: GlobalPoint) -> Result<Target, String> {
        let inj = self.guard.injector().clone();
        Self::blocking(move || inj.target_at(p))
            .await?
            .map_err(|e| input_error_text(&e))
    }

    async fn keyboard_target(&self) -> Result<Target, String> {
        let inj = self.guard.injector().clone();
        Self::blocking(move || inj.keyboard_target())
            .await?
            .map_err(|e| input_error_text(&e))
    }

    fn mac(&self) -> bool {
        self.guard.injector().mac_keys()
    }

    fn base_action(tool: &str) -> Action {
        Action {
            server: SERVER_NAME.into(),
            tool: tool.into(),
            summary: String::new(),
            preview: String::new(),
            scope: Scope::Screenshots,
            risk: Risk::Normal,
            read_only: false,
            outbound: false,
            keep_asking: None,
            network: false,
            hard_limit: None,
            payload: Value::Null,
        }
    }

    fn hard_limit(tool: &str, kind: &str, why: String) -> Described {
        let mut a = Self::base_action(tool);
        a.summary = policy::sanitize::one_line(&why, 200);
        a.scope = Scope::ComputerInput {
            kind: kind.into(),
            app: String::new(),
        };
        a.hard_limit = Some(why);
        Described::Action(a)
    }

    async fn describe_screenshot(&self, args: &Map<String, Value>) -> Described {
        let ms = match self.monitors().await {
            Ok(m) => m,
            Err(e) => return Described::Invalid(e),
        };
        let index = match u(args, "monitor") {
            Some(n) if n >= 1 && (n as usize) <= ms.len() => n as usize - 1,
            Some(n) => {
                return Described::Invalid(format!(
                    "there is no monitor {n}; this computer has {} (1 to {})",
                    ms.len(),
                    ms.len()
                ));
            }
            None => {
                let cap = self.capture.clone();
                let fg = Self::blocking(move || cap.foreground_window_rect())
                    .await
                    .ok()
                    .flatten();
                monitor_for_window(&ms, fg).unwrap_or(0)
            }
        };
        let m = &ms[index];
        let line = monitor_line(m, index, ms.len());
        let mut a = Self::base_action(TOOL_SCREENSHOT);
        a.summary = policy::sanitize::one_line(&format!("Take a screenshot of {line}"), 200);
        a.preview = policy::sanitize::visible(&format!(
            "Captures {line}.\nThis app's own windows are left out where the system allows. The image is sent to the model."
        ));
        a.scope = Scope::Screenshots;
        a.read_only = true;
        a.payload = json!({ "monitor": m });
        Described::Action(a)
    }

    /// The chat's latest screenshot, or why there is none.
    fn last_for(&self, chat: &ChatContext) -> Result<ScreenshotGeometry, String> {
        self.state()
            .chats
            .get(&chat.chat_id)
            .and_then(|c| c.last.clone())
            .ok_or_else(|| {
                "Take a computer_screenshot first: coordinates refer to the latest screenshot."
                    .to_string()
            })
    }

    // `Described` is the host's type; the error path is the rare one.
    #[allow(clippy::result_large_err)]
    async fn pointer_target(&self, p: GlobalPoint, tool: &str) -> Result<Target, Described> {
        if let Some(r) = refuse_point(self.guard.self_process(), p) {
            return Err(Self::hard_limit(tool, "pointer", r.to_string()));
        }
        let t = self.target_at(p).await.map_err(Described::Invalid)?;
        if let Some(r) = refuse_target(self.guard.self_process(), &t, false) {
            return Err(Self::hard_limit(tool, "pointer", r.to_string()));
        }
        Ok(t)
    }

    fn target_lines(t: &Target) -> String {
        let mut s = format!("Application: {}", t.app.display());
        if let Some(title) = t.window_title.as_deref().filter(|x| !x.is_empty()) {
            s.push_str(&format!(
                "\nWindow: \"{}\"",
                policy::sanitize::one_line(title, 120)
            ));
        }
        s
    }

    /// `tail` is model-chosen text shown last on the card (the text to type), after every
    /// line the app wrote, so it cannot pose as one of them.
    #[allow(clippy::too_many_arguments)]
    fn input_action(
        tool: &str,
        kind: &str,
        summary: String,
        preview: String,
        tail: Option<&str>,
        app: &str,
        always_ask: Option<String>,
        outbound: bool,
        plan: &InputPlan,
    ) -> Described {
        let mut a = Self::base_action(tool);
        let mut preview = preview;
        if let Some(why) = &always_ask {
            preview.push_str(&format!("\nAlways asks: {why}."));
        }
        if let Some(tail) = tail {
            preview.push_str(tail);
        }
        a.summary = policy::sanitize::one_line(&summary, 200);
        a.preview = policy::sanitize::visible(&preview);
        a.scope = Scope::ComputerInput {
            kind: kind.into(),
            app: app.into(),
        };
        a.risk = if always_ask.is_some() {
            Risk::AlwaysAsk
        } else {
            Risk::Normal
        };
        a.read_only = false;
        a.outbound = outbound;
        a.payload = serde_json::to_value(plan).unwrap_or(Value::Null);
        Described::Action(a)
    }

    async fn describe_pointer(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
    ) -> Described {
        let geo = match self.last_for(chat) {
            Ok(g) => g,
            Err(e) => return Described::Invalid(e),
        };
        let map = |key: &str| -> Result<(Vec<f64>, GlobalPoint), String> {
            let c = coord(args, key)?;
            let p = geo.to_global(&c).map_err(|e| e.to_string())?;
            Ok((c, p))
        };
        let shot = format!(
            "on screenshot {} ({}x{}, {})",
            geo.id,
            geo.image.0,
            geo.image.1,
            geo.convention.label()
        );
        let screen = |p: GlobalPoint| {
            format!(
                "Screen point {} on {}",
                fmt_point(p),
                policy::sanitize::one_line(&geo.monitor.name, 60)
            )
        };
        match tool {
            TOOL_CLICK | TOOL_MOVE | TOOL_SCROLL => {
                let (c, p) = match map("coordinate") {
                    Ok(v) => v,
                    Err(e) => return Described::Invalid(e),
                };
                let t = match self.pointer_target(p, tool).await {
                    Ok(t) => t,
                    Err(d) => return d,
                };
                let app_name = policy::sanitize::one_line(&t.app.name, 60);
                let mut always = target_always_ask_reason(&t, false).map(str::to_string);
                let (step, summary, what) = match tool {
                    TOOL_CLICK => {
                        let button = match button_arg(args) {
                            Ok(b) => b,
                            Err(e) => return Described::Invalid(e),
                        };
                        let clicks = u(args, "clicks").unwrap_or(1).clamp(1, 3) as u32;
                        let mut modifiers = Vec::new();
                        for m in args
                            .get("modifiers")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                        {
                            let name = m.as_str().unwrap_or("");
                            match parse_combo(name) {
                                Ok(KeyCombo {
                                    modifiers: ms,
                                    key: None,
                                }) if ms.len() == 1 => {
                                    if !modifiers.contains(&ms[0]) {
                                        modifiers.push(ms[0]);
                                    }
                                }
                                _ => {
                                    return Described::Invalid(format!(
                                        "modifiers may only contain shift, ctrl, alt and cmd, not {}",
                                        policy::sanitize::one_line(name, 20)
                                    ));
                                }
                            }
                        }
                        if !modifiers.is_empty() {
                            // Modifier keys go to the window with the keyboard focus.
                            match self.keyboard_target().await {
                                Ok(k)
                                    if refuse_target(self.guard.self_process(), &k, true)
                                        .is_none()
                                        && k.app.id == t.app.id =>
                                {
                                    // The focused window may always ask where the clicked
                                    // one does not (a console window of the same program).
                                    if always.is_none()
                                        && let Some(why) = target_always_ask_reason(&k, false)
                                    {
                                        always = Some(why.to_string());
                                    }
                                }
                                _ => {
                                    return Described::Invalid(format!(
                                        "{app_name} is not the application in front, so a click with modifier keys would send those keys elsewhere. Click it once without modifiers to bring it to the front, then try again."
                                    ));
                                }
                            }
                        }
                        let mac = self.mac();
                        let held = KeyCombo {
                            modifiers: modifiers.clone(),
                            key: None,
                        };
                        // The Windows key held during a click goes to the Windows shell.
                        if let Some(why) = system_shortcut(&held, mac) {
                            always.get_or_insert_with(|| {
                                format!("{} {why}, outside the application", held.label(mac))
                            });
                        }
                        let mods = modifiers
                            .iter()
                            .map(|k| k.label(mac))
                            .collect::<Vec<_>>()
                            .join("+");
                        let kind = match clicks {
                            2 => "Double-click",
                            3 => "Triple-click",
                            _ => "Click",
                        };
                        let what = format!(
                            "{}{kind} ({} button) at {}",
                            if mods.is_empty() {
                                String::new()
                            } else {
                                format!("{mods}+")
                            },
                            button.label(),
                            fmt_coord(&c)
                        );
                        (
                            Step::Click {
                                at: p,
                                button,
                                clicks,
                                modifiers,
                            },
                            format!("{what} in {app_name}"),
                            what,
                        )
                    }
                    TOOL_MOVE => {
                        let what = format!("Move the pointer to {}", fmt_coord(&c));
                        (Step::Move { at: p }, format!("{what} in {app_name}"), what)
                    }
                    _ => {
                        let amount = u(args, "amount").unwrap_or(3).clamp(1, 20) as i32;
                        let (dx, dy, dir) = match s(args, "direction").unwrap_or("") {
                            "up" => (0, -amount, "up"),
                            "down" => (0, amount, "down"),
                            "left" => (-amount, 0, "left"),
                            "right" => (amount, 0, "right"),
                            _ => {
                                return Described::Invalid(
                                    "direction must be up, down, left or right".into(),
                                );
                            }
                        };
                        let what = format!("Scroll {dir} {amount} at {}", fmt_coord(&c));
                        (
                            Step::Scroll { at: p, dx, dy },
                            format!("{what} in {app_name}"),
                            what,
                        )
                    }
                };
                let plan = InputPlan {
                    apps: vec![t.app.id.clone()],
                    monitor: Some(geo.monitor.clone()),
                    shown: fmt_coord(&c),
                    step,
                };
                let preview = format!("{what} {shot}\n{}\n{}", screen(p), Self::target_lines(&t));
                Self::input_action(
                    tool, "pointer", summary, preview, None, &t.app.id, always, false, &plan,
                )
            }
            TOOL_DRAG => {
                let (cf, from) = match map("from") {
                    Ok(v) => v,
                    Err(e) => return Described::Invalid(e),
                };
                let (ct, to) = match map("to") {
                    Ok(v) => v,
                    Err(e) => return Described::Invalid(e),
                };
                let button = match button_arg(args) {
                    Ok(b) => b,
                    Err(e) => return Described::Invalid(e),
                };
                let duration_ms = u(args, "duration_ms")
                    .unwrap_or(500)
                    .clamp(100, MAX_HOLD_MS);
                let start = match self.pointer_target(from, tool).await {
                    Ok(t) => t,
                    Err(d) => return d,
                };
                let end = match self.pointer_target(to, tool).await {
                    Ok(t) => t,
                    Err(d) => return d,
                };
                // The path must not cross this app's windows either.
                for i in 1..DRAG_CHECKS {
                    let f = i as f64 / DRAG_CHECKS as f64;
                    let q = lerp(from, to, f);
                    if let Err(d) = self.pointer_target(q, tool).await {
                        return d;
                    }
                }
                let mut apps = vec![start.app.id.clone()];
                let mut always = target_always_ask_reason(&start, false)
                    .or_else(|| target_always_ask_reason(&end, false))
                    .map(str::to_string);
                if end.app.id != start.app.id {
                    apps.push(end.app.id.clone());
                    always.get_or_insert_with(|| {
                        format!(
                            "the drag goes from {} to another application, {}",
                            policy::sanitize::one_line(&start.app.name, 60),
                            policy::sanitize::one_line(&end.app.name, 60)
                        )
                    });
                }
                let what = format!(
                    "Drag ({} button) from {} to {}",
                    button.label(),
                    fmt_coord(&cf),
                    fmt_coord(&ct)
                );
                let plan = InputPlan {
                    apps,
                    monitor: Some(geo.monitor.clone()),
                    shown: format!("{} to {}", fmt_coord(&cf), fmt_coord(&ct)),
                    step: Step::Drag {
                        from,
                        to,
                        button,
                        duration_ms,
                    },
                };
                let mut preview = format!(
                    "{what} {shot}\nFrom: {}\n{}\nTo: {}",
                    screen(from),
                    Self::target_lines(&start),
                    screen(to)
                );
                if end.app.id != start.app.id {
                    preview.push_str(&format!("\n{}", Self::target_lines(&end)));
                }
                let summary = format!(
                    "{what} in {}",
                    policy::sanitize::one_line(&start.app.name, 60)
                );
                Self::input_action(
                    tool,
                    "pointer",
                    summary,
                    preview,
                    None,
                    &start.app.id,
                    always,
                    false,
                    &plan,
                )
            }
            other => Described::Invalid(format!("the computer server has no tool named {other}")),
        }
    }

    async fn describe_keyboard(&self, tool: &str, args: &Map<String, Value>) -> Described {
        let mac = self.mac();
        // Typing: text, or a key other than a lone modifier.
        let mut typing = true;
        let (step, what, mut always): (Step, String, Option<String>) = match tool {
            TOOL_TYPE => {
                let text = s(args, "text").unwrap_or("");
                if text.is_empty() {
                    return Described::Invalid("text is empty".into());
                }
                if text.chars().count() > MAX_TYPE_CHARS {
                    return Described::Invalid(format!(
                        "text is longer than {MAX_TYPE_CHARS} characters; type it in parts"
                    ));
                }
                if let Err(e) = text_events(text, TYPE_CHUNK) {
                    return Described::Invalid(e);
                }
                (
                    Step::Type {
                        text: text.to_string(),
                    },
                    format!("Type {} characters", text.chars().count()),
                    None,
                )
            }
            _ => {
                let combo = match parse_combo(s(args, "keys").unwrap_or("")) {
                    Ok(c) => c,
                    Err(e) => return Described::Invalid(e),
                };
                let repeat = u(args, "repeat").unwrap_or(1).clamp(1, 20) as u32;
                let label = combo.label(mac);
                let what = if repeat > 1 {
                    format!("Press {label} {repeat} times")
                } else {
                    format!("Press {label}")
                };
                let always = system_shortcut(&combo, mac)
                    .map(|why| format!("{label} {why}, outside the application in front"));
                typing = combo.key.is_some();
                (Step::Key { combo, repeat }, what, always)
            }
        };
        let t = match self.keyboard_target().await {
            Ok(t) => t,
            Err(e) => return Described::Invalid(e),
        };
        if let Some(r) = refuse_target(self.guard.self_process(), &t, true) {
            return Self::hard_limit(tool, "keyboard", r.to_string());
        }
        if let Some(why) = target_always_ask_reason(&t, typing) {
            always.get_or_insert_with(|| why.to_string());
        }
        let app_name = policy::sanitize::one_line(&t.app.name, 60);
        let preview = format!(
            "{what} into the application in front\n{}",
            Self::target_lines(&t)
        );
        let tail = match &step {
            Step::Type { text } => Some(format!("\nText:\n{text}")),
            _ => None,
        };
        let summary = match &step {
            Step::Type { text } => format!(
                "Type \"{}\" in {app_name}",
                policy::sanitize::one_line(text, 80)
            ),
            _ => format!("{what} in {app_name}"),
        };
        let plan = InputPlan {
            apps: vec![t.app.id.clone()],
            monitor: None,
            shown: String::new(),
            step,
        };
        // Typing discloses the text to another program (9.3: "Typing itself discloses the
        // typed text"), so keyboard input counts as outbound for D9.
        Self::input_action(
            tool,
            "keyboard",
            summary,
            preview,
            tail.as_deref(),
            &t.app.id,
            always,
            true,
            &plan,
        )
    }

    async fn call_screenshot(&self, action: &Action, ctx: &CallContext) -> ToolOutput {
        let Some(approved) = action
            .payload
            .get("monitor")
            .and_then(|m| serde_json::from_value::<MonitorInfo>(m.clone()).ok())
        else {
            return ToolOutput::error("internal error: the approved action has no monitor");
        };
        let ms = match self.monitors().await {
            Ok(m) => m,
            Err(e) => return ToolOutput::error(e),
        };
        let Some(index) = ms.iter().position(|m| m.id == approved.id) else {
            return ToolOutput::error(
                "That monitor is no longer connected. Take a screenshot again.",
            );
        };
        let monitor = ms[index].clone();
        let profile = self.profile(ctx.chat.chat_id);
        let cap = self.capture.clone();
        let m2 = monitor.clone();
        let capture_lock = self.capture_lock.clone();
        let captured = tokio::select! {
            r = Self::blocking(move || {
                let _one_at_a_time = capture_lock.lock().unwrap_or_else(|e| e.into_inner());
                let shot = cap.capture(&m2)?;
                let cursor = cap.cursor_position();
                Ok::<_, CaptureError>((shot, cursor))
            }) => r,
            _ = ctx.cancel.cancelled() => return ToolOutput::error("Stopped by the user."),
        };
        let (shot, cursor) = match captured {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return ToolOutput::error(e.to_string()),
            Err(e) => return ToolOutput::error(e),
        };
        let captured_size = (shot.width, shot.height);
        let image_size = profile.resize.target_size(shot.width, shot.height);
        let id = {
            let mut st = self.state();
            st.next_id += 1;
            st.next_id
        };
        let geo = ScreenshotGeometry {
            id,
            monitor: monitor.clone(),
            captured: captured_size,
            image: image_size,
            convention: profile.convention,
        };
        let marker = cursor.and_then(|c| geo.global_to_image(c));
        let encoded = Self::blocking(move || -> Result<Vec<u8>, String> {
            let mut img = image_ops::resize(shot.rgba, captured_size, image_size)?;
            if let Some((x, y)) = marker {
                let scale = (f64::from(image_size.0) / 1600.0).max(1.0);
                image_ops::draw_cursor(&mut img, x, y, scale);
            }
            image_ops::encode_png(&img)
        })
        .await;
        let png = match encoded {
            Ok(Ok(p)) => p,
            Ok(Err(e)) | Err(e) => return ToolOutput::error(e),
        };
        let mut text = format!(
            "Screenshot {id}: {}, sent as {}x{} pixels. {}",
            monitor_line(&monitor, index, ms.len()),
            image_size.0,
            image_size.1,
            profile.convention.instructions(image_size)
        );
        if let Some((mx, my)) = cursor.and_then(|c| geo.global_to_model(c)) {
            text.push_str(&format!(
                " The pointer (drawn as an arrow) is at [{mx}, {my}]."
            ));
        }
        if ms.len() > 1 {
            text.push_str(&format!(
                " Other monitors: pass monitor 1 to {} to see them.",
                ms.len()
            ));
        }
        if !shot.self_excluded {
            text.push_str(
                " This app's own windows may appear in the image; they cannot be controlled.",
            );
        }
        if let Some(note) = &shot.note {
            text.push(' ');
            text.push_str(note);
        }
        let png_len = png.len();
        let data = base64::engine::general_purpose::STANDARD.encode(png);
        {
            let mut st = self.state();
            st.chats.entry(ctx.chat.chat_id).or_default().last = Some(geo.clone());
        }
        ToolOutput {
            content: vec![
                Content::Image {
                    mime: "image/png".into(),
                    data_base64: data,
                },
                Content::Text { text },
            ],
            is_error: false,
            full_text: None,
            ui: Some(json!({
                "screenshot": geo,
                "pngBytes": png_len,
                "selfExcluded": shot.self_excluded,
            })),
        }
    }

    async fn call_input(&self, action: &Action, ctx: &CallContext) -> ToolOutput {
        let plan: InputPlan = match serde_json::from_value(action.payload.clone()) {
            Ok(p) => p,
            Err(_) => return ToolOutput::error("internal error: the approved action has no plan"),
        };
        // The approved scope names the application the plan was made for.
        match &action.scope {
            Scope::ComputerInput { app, .. } if plan.apps.first() == Some(app) => {}
            _ => {
                return ToolOutput::error(
                    "internal error: the approved action does not match its plan",
                );
            }
        }
        if let Some(check) = self.config.ready_check.clone() {
            // The app's check may wait for its main thread (showing the indicator, asking
            // AppKit whether it is on the active Space): not on an async worker.
            let r = tokio::select! {
                r = Self::blocking(move || check()) => r,
                _ = ctx.cancel.cancelled() => return op_output(Err(OpError::Stopped)),
            };
            match r {
                Ok(Ok(())) => {}
                Ok(Err(why)) | Err(why) => {
                    return ToolOutput::error(format!("Computer use cannot act right now: {why}"));
                }
            }
        }
        // Coordinates are only valid for the monitor layout they were computed on.
        if let Some(m) = &plan.monitor {
            match self.monitors().await {
                Ok(ms)
                    if ms
                        .iter()
                        .any(|x| x.id == m.id && x.bounds.approx_eq(&m.bounds)) => {}
                Ok(_) => {
                    return ToolOutput::error(
                        "The screen layout changed since the screenshot (a monitor was added, removed, moved or rescaled). Nothing was sent; take a new screenshot.",
                    );
                }
                Err(e) => return ToolOutput::error(e),
            }
        }
        // Another chat's operation may hold the injector (a drag lasts up to 2 s); Stop
        // must not wait for it.
        let lock = tokio::select! {
            l = self.op_lock.clone().lock_owned() => l,
            _ = ctx.cancel.cancelled() => return op_output(Err(OpError::Stopped)),
        };
        let guard = self.guard.clone();
        let cancel = ctx.cancel.clone();
        // Only an action the user approved on an always-ask card may reach a target that
        // always asks; anything else (possibly allowed by a grant) is refused there.
        let always_ask_approved = action.risk == Risk::AlwaysAsk;
        let task = tokio::task::spawn_blocking(move || {
            let _lock = lock;
            run_plan(&guard, &plan, cancel, always_ask_approved)
        });
        let result = tokio::select! {
            r = task => match r {
                Ok(r) => r,
                Err(e) if e.is_panic() => Err(OpError::Input(InputError::Platform(
                    "the input step failed unexpectedly; every key and button was released".into(),
                ))),
                Err(e) => Err(OpError::Input(InputError::Platform(e.to_string()))),
            },
            _ = ctx.cancel.cancelled() => {
                let stop = self.guard.stop_handle();
                let _ = Self::blocking(move || stop.release_all_and_wait()).await;
                Err(OpError::Stopped)
            }
        };
        if result.is_ok() {
            tokio::select! {
                _ = tokio::time::sleep(SETTLE) => {}
                _ = ctx.cancel.cancelled() => {}
            }
        }
        op_output(result)
    }
}

/// Points checked along a drag's path before it starts.
const DRAG_CHECKS: usize = 8;

fn lerp(a: GlobalPoint, b: GlobalPoint, f: f64) -> GlobalPoint {
    GlobalPoint::new(a.x + (b.x - a.x) * f, a.y + (b.y - a.y) * f)
}

/// Runs an approved plan through the guard. Blocking; runs on a blocking thread.
fn run_plan(
    guard: &InputGuard,
    plan: &InputPlan,
    cancel: tokio_util::sync::CancellationToken,
    always_ask_approved: bool,
) -> Result<String, OpError> {
    let Some(app) = plan.apps.first() else {
        return Err(OpError::Input(InputError::NoTarget(
            "the plan names no application".into(),
        )));
    };
    let expect = Expect::App(app);
    let mut op = guard.begin_with(cancel, always_ask_approved);
    match &plan.step {
        Step::Click {
            at,
            button,
            clicks,
            modifiers,
        } => {
            op.move_to(*at, expect)?;
            for m in modifiers {
                op.key_down(*m, expect)?;
            }
            for i in 0..*clicks {
                op.button_down(*button, *at, expect)?;
                op.pause(Duration::from_millis(30))?;
                op.button_up(*button)?;
                if i + 1 < *clicks {
                    op.pause(Duration::from_millis(60))?;
                }
            }
            for m in modifiers.iter().rev() {
                op.key_up(*m)?;
            }
            let n = match clicks {
                2 => "Double-clicked",
                3 => "Triple-clicked",
                _ => "Clicked",
            };
            Ok(format!(
                "{n} the {} button at {}. Take a screenshot to see the result.",
                button.label(),
                plan.shown
            ))
        }
        Step::Move { at } => {
            op.move_to(*at, expect)?;
            Ok(format!("Moved the pointer to {}.", plan.shown))
        }
        Step::Drag {
            from,
            to,
            button,
            duration_ms,
        } => {
            let apps = Expect::AnyOf(&plan.apps);
            // Check the whole path before pressing anything.
            op.check_pointer(*from, expect)?;
            for i in 1..DRAG_CHECKS {
                op.check_pointer(
                    lerp(*from, *to, i as f64 / DRAG_CHECKS as f64),
                    Expect::NotSelf,
                )?;
            }
            op.check_pointer(*to, apps)?;
            op.move_to(*from, expect)?;
            op.button_down(*button, *from, expect)?;
            let duration = (*duration_ms).min(MAX_HOLD_MS);
            let steps = (duration / 16).clamp(5, 60);
            let step_wait = Duration::from_millis(duration / steps);
            for i in 1..=steps {
                let q = lerp(*from, *to, i as f64 / steps as f64);
                let e = if i == steps { apps } else { Expect::NotSelf };
                op.move_to(q, e)?;
                op.pause(step_wait)?;
            }
            op.button_up(*button)?;
            Ok(format!("Dragged from {}.", plan.shown))
        }
        Step::Scroll { at, dx, dy } => {
            op.move_to(*at, expect)?;
            let n = dx.abs().max(dy.abs());
            for _ in 0..n {
                op.scroll(*at, dx.signum(), dy.signum(), expect)?;
                op.pause(Duration::from_millis(20))?;
            }
            let dir = match (dx.signum(), dy.signum()) {
                (_, 1) => "down",
                (_, -1) => "up",
                (1, _) => "right",
                _ => "left",
            };
            Ok(format!("Scrolled {dir} {n} at {}.", plan.shown))
        }
        Step::Type { text } => {
            let parts = text_events(text, TYPE_CHUNK)
                .map_err(|e| OpError::Input(InputError::Platform(e)))?;
            for part in parts {
                match part {
                    TextPart::Text(t) => op.text(&t, expect)?,
                    TextPart::Key(k) => {
                        op.key_down(k, expect)?;
                        op.key_up(k)?;
                    }
                }
                op.pause(Duration::from_millis(2))?;
            }
            Ok(format!("Typed {} characters.", text.chars().count()))
        }
        Step::Key { combo, repeat } => {
            let keys = combo.press_order();
            for i in 0..*repeat {
                for k in &keys {
                    op.key_down(*k, expect)?;
                }
                for k in keys.iter().rev() {
                    op.key_up(*k)?;
                }
                if i + 1 < *repeat {
                    op.pause(Duration::from_millis(30))?;
                }
            }
            let mac = guard.injector().mac_keys();
            Ok(if *repeat > 1 {
                format!("Pressed {} {repeat} times.", combo.label(mac))
            } else {
                format!("Pressed {}.", combo.label(mac))
            })
        }
    }
}

fn coordinate_schema(what: &str) -> Value {
    json!({
        "type": "array",
        "items": {"type": "number"},
        "minItems": 2,
        "maxItems": 4,
        "description": format!("{what}: [x, y] in the coordinates the latest screenshot's text names (pixels of that image, or a 0-1000 grid), or a box [x1, y1, x2, y2] whose centre is used")
    })
}

#[async_trait]
impl BuiltinServer for ComputerServer {
    fn name(&self) -> &str {
        SERVER_NAME
    }

    fn tools(&self) -> Vec<ToolDef> {
        let def =
            |name: &str, desc: &str, props: Value, required: Vec<&str>, read_only: bool| ToolDef {
                name: name.into(),
                description: desc.into(),
                input_schema: json!({
                    "type": "object",
                    "properties": props,
                    "required": required,
                    "additionalProperties": false
                }),
                destructive: Some(false),
                read_only: Some(read_only),
            };
        let button = json!({"type": "string", "enum": ["left", "right", "middle"], "description": "Mouse button (default left)"});
        vec![
            def(
                TOOL_SCREENSHOT,
                "Take a screenshot of one monitor (default: the one with the window in front). Returns the image and a line saying how to write coordinates for it. Coordinates in the other computer tools refer to the latest screenshot, so take a new one after the screen changes.",
                json!({"monitor": {"type": "integer", "minimum": 1, "maximum": 16, "description": "Monitor number, 1 is the primary"}}),
                vec![],
                true,
            ),
            def(
                TOOL_CLICK,
                "Click at a point of the latest screenshot.",
                json!({
                    "coordinate": coordinate_schema("Where to click"),
                    "button": button,
                    "clicks": {"type": "integer", "minimum": 1, "maximum": 3, "description": "2 for a double-click, 3 for a triple-click (default 1)"},
                    "modifiers": {"type": "array", "items": {"type": "string", "enum": ["shift", "ctrl", "alt", "cmd"]}, "maxItems": 4, "description": "Keys held during the click"}
                }),
                vec!["coordinate"],
                false,
            ),
            def(
                TOOL_MOVE,
                "Move the mouse pointer to a point of the latest screenshot without clicking (to hover).",
                json!({"coordinate": coordinate_schema("Where to move")}),
                vec!["coordinate"],
                false,
            ),
            def(
                TOOL_DRAG,
                "Press a mouse button at one point of the latest screenshot, move to another and release.",
                json!({
                    "from": coordinate_schema("Start point"),
                    "to": coordinate_schema("End point"),
                    "button": button,
                    "duration_ms": {"type": "integer", "minimum": 100, "maximum": MAX_HOLD_MS, "description": "How long the drag takes (default 500)"}
                }),
                vec!["from", "to"],
                false,
            ),
            def(
                TOOL_SCROLL,
                "Scroll with the mouse wheel at a point of the latest screenshot.",
                json!({
                    "coordinate": coordinate_schema("Where to scroll"),
                    "direction": {"type": "string", "enum": ["up", "down", "left", "right"]},
                    "amount": {"type": "integer", "minimum": 1, "maximum": 20, "description": "Wheel notches (default 3)"}
                }),
                vec!["coordinate", "direction"],
                false,
            ),
            def(
                TOOL_TYPE,
                "Type text into the application in front, as if on the keyboard. Newlines press Enter and tabs press Tab. Click the text field first.",
                json!({"text": {"type": "string", "minLength": 1, "maxLength": MAX_TYPE_CHARS}}),
                vec!["text"],
                false,
            ),
            def(
                TOOL_KEY,
                "Press a key or key combination in the application in front, e.g. \"Enter\", \"Escape\", \"ctrl+c\", \"alt+F4\", \"cmd+shift+t\" (on macOS use cmd for shortcuts).",
                json!({
                    "keys": {"type": "string", "minLength": 1, "maxLength": 64, "description": "Modifiers first, then one key, joined by +"},
                    "repeat": {"type": "integer", "minimum": 1, "maximum": 20, "description": "Press it this many times (default 1)"}
                }),
                vec!["keys"],
                false,
            ),
        ]
    }

    async fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
    ) -> Described {
        match tool {
            TOOL_SCREENSHOT => self.describe_screenshot(args).await,
            TOOL_CLICK | TOOL_MOVE | TOOL_DRAG | TOOL_SCROLL => {
                self.describe_pointer(tool, args, chat).await
            }
            TOOL_TYPE | TOOL_KEY => self.describe_keyboard(tool, args).await,
            other => Described::Invalid(format!("the computer server has no tool named {other}")),
        }
    }

    async fn call(
        &self,
        tool: &str,
        _args: &Map<String, Value>,
        action: &Action,
        ctx: CallContext,
    ) -> ToolOutput {
        if action.server != SERVER_NAME || action.tool != tool {
            return ToolOutput::error("internal error: the approved action is for another tool");
        }
        if let Some(h) = &action.hard_limit {
            return ToolOutput::error(h.clone());
        }
        if ctx.cancel.is_cancelled() {
            return ToolOutput::error("Stopped by the user.");
        }
        match tool {
            TOOL_SCREENSHOT => self.call_screenshot(action, &ctx).await,
            TOOL_CLICK | TOOL_MOVE | TOOL_DRAG | TOOL_SCROLL | TOOL_TYPE | TOOL_KEY => {
                self.call_input(action, &ctx).await
            }
            other => ToolOutput::error(format!("the computer server has no tool named {other}")),
        }
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
