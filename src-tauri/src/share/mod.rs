//! Other apps on this computer use the loaded model (0.0.5 item 3, direction A).
//! Experimental and off by default (D5).
//!
//! WP0 wrote the types and the command stubs, which return fixed sample data and change
//! nothing: no port is opened, no key is made, nothing is saved. WP5a owns this folder
//! from here (`mod.rs`, `proxy.rs`, `setup_text.rs`): the state machine, the sharing key
//! in the keychain, the proxy on a loopback port chosen at random from 49152-65535 on
//! first switch-on and then kept (D10), and the setup text.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::error::AppResult;
use crate::state::AppState;

/// The settings key of [`ShareSettings`].
#[allow(dead_code, reason = "0.0.5 contract: WP5a's state machine uses it")]
pub const SETTINGS_KEY: &str = "shareWithApps";

/// What is saved under [`SETTINGS_KEY`]: the switch and the kept port. Absent until
/// sharing is first switched on. The sharing key itself lives in the keychain only.
#[allow(dead_code, reason = "0.0.5 contract: WP5a's state machine uses it")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSettings {
    pub enabled: bool,
    pub port: u16,
}

/// What the Other apps page shows (`share_status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareStatus {
    /// The switch "Let other apps on this computer use the loaded model".
    pub enabled: bool,
    /// The kept port; `None` until sharing is first switched on.
    pub port: Option<u16>,
    /// The proxy is listening on `127.0.0.1:<port>`.
    pub listening: bool,
    /// Why it is not listening although switched on, in plain words (for example "Port N
    /// is used by another program. Choose another port.").
    pub error: Option<String>,
    /// Requests refused since sharing was switched on (counted in memory only).
    pub refused: u64,
    /// Another app's request is being answered now ("Used by another app").
    pub in_use: bool,
    /// The loaded model's context in tokens, for the 32,768-token warning; `None` when no
    /// model is loaded.
    pub model_ctx: Option<u32>,
}

/// An app the setup text is for. JSON: `"claudeCode"`, `"codex"`, `"claudeApp"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareApp {
    ClaudeCode,
    Codex,
    /// The Claude desktop app's third-party inference setting ("Not tried yet").
    ClaudeApp,
}

/// Which system a way of setting up is for (the Windows and Mac tabs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SetupSystem {
    Windows,
    Mac,
    /// The same on both.
    Any,
}

/// One step of a way of setting up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStep {
    /// What to do, in plain words.
    pub text: String,
    /// Text to copy (a Copy button), built with real JSON/TOML serialisers (WP5a).
    pub code: Option<String>,
    /// The full path of the file this step edits, when it edits one.
    pub file: Option<String>,
}

/// One way of setting up an app: the one-session way first, the settings-file way second.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupWay {
    pub title: String,
    pub system: SetupSystem,
    pub steps: Vec<SetupStep>,
}

/// The setup card for one app (`share_setup`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSetup {
    pub app: ShareApp,
    /// The app's name as shown.
    pub title: String,
    /// Marked "Not tried yet" on the card.
    pub not_tried: bool,
    pub ways: Vec<SetupWay>,
    /// The sentences every card ends with.
    pub notes: Vec<String>,
}

// TODO(WP5a): every command below is a stub that returns fixed sample data.

/// The fixed sample status of the stubs: off, never switched on.
fn sample_status() -> ShareStatus {
    ShareStatus {
        enabled: false,
        port: None,
        listening: false,
        error: None,
        refused: 0,
        in_use: false,
        model_ctx: None,
    }
}

/// The port the stubs report once switched on (a made-up example in the range of D10).
const SAMPLE_PORT: u16 = 51_234;

/// The sample setup card the stub returns.
fn sample_setup(app: ShareApp) -> ShareSetup {
    let title = match app {
        ShareApp::ClaudeCode => "Claude Code",
        ShareApp::Codex => "Codex",
        ShareApp::ClaudeApp => "Claude app",
    };
    let step = |text: &str, code: Option<&str>| SetupStep {
        text: text.to_string(),
        code: code.map(str::to_string),
        file: None,
    };
    ShareSetup {
        app,
        title: title.to_string(),
        not_tried: app == ShareApp::ClaudeApp,
        ways: vec![
            SetupWay {
                title: "Sample: for one window only".into(),
                system: SetupSystem::Windows,
                steps: vec![step("Sample step.", Some("sample text to copy"))],
            },
            SetupWay {
                title: "Sample: for one window only".into(),
                system: SetupSystem::Mac,
                steps: vec![step("Sample step.", Some("sample text to copy"))],
            },
        ],
        notes: vec!["Sample note.".into()],
    }
}

#[tauri::command]
pub async fn share_status(
    window: tauri::WebviewWindow,
    _state: State<'_, AppState>,
) -> AppResult<ShareStatus> {
    crate::commands::only_main_window(window.label())?;
    Ok(sample_status())
}

/// Switches sharing on or off. Stub: reports the switch as asked, never listening, and
/// saves nothing.
#[tauri::command]
pub async fn set_share_enabled(
    window: tauri::WebviewWindow,
    _state: State<'_, AppState>,
    on: bool,
) -> AppResult<ShareStatus> {
    crate::commands::only_main_window(window.label())?;
    Ok(ShareStatus {
        enabled: on,
        port: on.then_some(SAMPLE_PORT),
        ..sample_status()
    })
}

/// Chooses another port. Stub: reports the port, saves nothing.
#[tauri::command]
pub async fn set_share_port(
    window: tauri::WebviewWindow,
    _state: State<'_, AppState>,
    port: u16,
) -> AppResult<ShareStatus> {
    crate::commands::only_main_window(window.label())?;
    Ok(ShareStatus {
        port: Some(port),
        ..sample_status()
    })
}

/// The sharing key, shown only on request. Stub: a fixed sample that opens nothing.
#[tauri::command]
pub async fn reveal_share_key(
    window: tauri::WebviewWindow,
    _state: State<'_, AppState>,
) -> AppResult<String> {
    crate::commands::only_main_window(window.label())?;
    Ok(format!("harnessui-share-{}", "0".repeat(32)))
}

/// Replaces the sharing key. Stub: changes nothing.
#[tauri::command]
pub async fn new_share_key(
    window: tauri::WebviewWindow,
    _state: State<'_, AppState>,
) -> AppResult<ShareStatus> {
    crate::commands::only_main_window(window.label())?;
    Ok(sample_status())
}

/// The setup card for one app.
#[tauri::command]
pub async fn share_setup(
    window: tauri::WebviewWindow,
    _state: State<'_, AppState>,
    app: ShareApp,
) -> AppResult<ShareSetup> {
    crate::commands::only_main_window(window.label())?;
    Ok(sample_setup(app))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The webview reads exactly these shapes (src/lib/types.ts).
    #[test]
    fn shapes() {
        assert_eq!(
            serde_json::to_value(sample_status()).unwrap(),
            json!({
                "enabled": false,
                "port": null,
                "listening": false,
                "error": null,
                "refused": 0,
                "inUse": false,
                "modelCtx": null
            })
        );
        assert_eq!(
            serde_json::to_value(ShareSettings {
                enabled: true,
                port: 50000
            })
            .unwrap(),
            json!({"enabled": true, "port": 50000})
        );
        for (app, name) in [
            (ShareApp::ClaudeCode, "claudeCode"),
            (ShareApp::Codex, "codex"),
            (ShareApp::ClaudeApp, "claudeApp"),
        ] {
            assert_eq!(serde_json::to_value(app).unwrap(), json!(name));
            let setup = serde_json::to_value(sample_setup(app)).unwrap();
            assert_eq!(setup["app"], json!(name));
            assert_eq!(setup["ways"][0]["system"], json!("windows"));
            assert!(setup["ways"][0]["steps"][0].get("code").is_some());
            assert_eq!(setup["notTried"], json!(app == ShareApp::ClaudeApp));
        }
    }
}
