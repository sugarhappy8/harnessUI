//! The built-in `computer` tool server (ARCHITECTURE.md 9.3 "Computer use", 13.3, 13.5,
//! D7, 4.2 Stop).
//!
//! - [`ComputerServer`] implements `mcp_host::BuiltinServer` with the tools
//!   `computer_screenshot`, `computer_click`, `computer_move`, `computer_drag`,
//!   `computer_scroll`, `computer_type` and `computer_key`.
//! - [`platform`] holds the two small traits [`Capture`] and [`Injector`], their native
//!   implementations (compiled on Windows and macOS only), the [`MainThread`] trait the app
//!   implements with Tauri's `run_on_main_thread`, and a [`platform::fake`] desktop for
//!   tests on every OS.
//! - [`guard`] is the one injector all input goes through: never this app, only the
//!   approved application, everything held is released on Stop, panic and shutdown.
//! - [`geometry`] and [`profile`] map the model's coordinates back to the screen, after
//!   resizing each screenshot to exactly what llama.cpp would feed the model.
//! - [`apps`] identifies applications and lists the ones that always ask; [`keys`] parses
//!   key combinations and spots shortcuts the OS handles itself.
//!
//! This crate starts no processes.

pub mod apps;
pub mod geometry;
pub mod guard;
mod image_ops;
pub mod keys;
pub mod platform;
pub mod profile;
mod server;

pub use apps::{AppInfo, Target, always_ask_reason};
pub use geometry::{GlobalPoint, MonitorInfo, Rect, ScreenshotGeometry};
pub use guard::{ELEVATED_REFUSAL, Held, InputGuard, StopHandle};
pub use platform::{
    Capture, CaptureError, Captured, Injector, InputError, MainThread, MouseButton, PlatformError,
    SelfProcess,
};
pub use profile::{AutoProfile, Convention, ModelProfile, ResizeRule, SmartResize, auto_profile};
pub use server::{
    ComputerConfig, ComputerServer, MAX_HOLD_MS, MAX_TYPE_CHARS, ReadyCheck, SERVER_NAME,
    TOOL_CLICK, TOOL_DRAG, TOOL_KEY, TOOL_MOVE, TOOL_SCREENSHOT, TOOL_SCROLL, TOOL_TYPE,
};

#[cfg(target_os = "macos")]
pub use platform::macos::{
    ACCESSIBILITY_SETTINGS_URL, SCREEN_RECORDING_SETTINGS_URL, accessibility_granted,
    screen_recording_granted, set_overlay_window_behavior, window_is_on_active_space,
};

/// System Settings → Privacy & Security → Screen Recording (undocumented by Apple; the
/// permissions screen also spells out the manual path). Available on every OS so the UI
/// can be built and tested anywhere.
pub const MACOS_SCREEN_RECORDING_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
/// System Settings → Privacy & Security → Accessibility (undocumented by Apple).
pub const MACOS_ACCESSIBILITY_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
