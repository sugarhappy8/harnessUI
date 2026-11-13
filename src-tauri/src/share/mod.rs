//! Other apps on this computer use the loaded model (0.0.6 item 3, direction A).
//! Experimental and off by default (D5).
//!
//! - **The switch never turns itself on.** Only `set_share_enabled`, a main-window command,
//!   changes it; it is saved under [`SETTINGS_KEY`] with the kept port, and a saved "on"
//!   starts the proxy again when the app starts.
//! - **The port** is chosen at random from [`PORT_RANGE`] on the first switch-on and then
//!   kept (D10). When it is taken the page says "Port N is used by another program. Choose
//!   another port." and nothing listens until the user picks another one.
//! - **The sharing key** (`harnessui-share-` and 32 random hex digits) lives in the
//!   keychain only, under [`KEY_SECRET`]; it is kept in memory for the proxy's checks,
//!   shown only by `reveal_share_key` and replaced by `new_share_key` (the old one stops
//!   working at once). It is never written to settings, files or logs.
//! - **The proxy** (`proxy.rs`) listens on `127.0.0.1` only; the setup text
//!   (`setup_text.rs`) is built with real JSON and TOML serialisers.
//! - **No approval mode changes any of this** (D12): sharing is not a tool action, and no
//!   tool reaches the switch, the port or the key in Ask first, Allow all or Full access.

mod proxy;
pub mod setup_text;
#[cfg(test)]
mod tests;

use std::ops::RangeInclusive;
use std::sync::{Arc, RwLock};

use rand::Rng;
use serde::{Deserialize, Serialize};
use store::secrets::SecretStore;
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::runtime_manager::RuntimeManager;
use crate::state::{AppState, BusyCounter};

/// The settings key of [`ShareSettings`].
pub const SETTINGS_KEY: &str = "shareWithApps";
/// The keychain entry of the sharing key.
pub const KEY_SECRET: &str = "share.apps-key";
/// What every sharing key starts with (32 hex digits follow).
pub const KEY_PREFIX: &str = "harnessui-share-";
/// The ports the first switch-on chooses from (the dynamic range).
pub const PORT_RANGE: RangeInclusive<u16> = 49152..=65535;
/// The lowest port a user may choose (lower ones need administrator rights on a Mac).
const LOWEST_PORT: u16 = 1024;
/// Random ports tried before giving up.
const PORT_TRIES: usize = 40;

/// What is saved under [`SETTINGS_KEY`]: the switch and the kept port. Absent until
/// sharing is first switched on. The sharing key itself lives in the keychain only.
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

/// Sharing as the Coding apps see it (WP6, [`Sharing::now`]). Never sent to the window,
/// and its `Debug` hides the key.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ShareNow {
    /// The kept port; `None` until sharing is first switched on.
    pub port: Option<u16>,
    /// The switch is on.
    pub enabled: bool,
    /// The port the proxy listens on now; `None` while the switch is off or nothing listens.
    pub listening: Option<u16>,
    /// The sharing key, only while the proxy listens.
    pub key: Option<String>,
}

impl std::fmt::Debug for ShareNow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShareNow")
            .field("port", &self.port)
            .field("enabled", &self.enabled)
            .field("listening", &self.listening)
            .field("key", &self.key.as_ref().map(|_| "hidden"))
            .finish()
    }
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
    /// Text to copy (a Copy button), built with real JSON/TOML serialisers.
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

// ---- The state machine -------------------------------------------------------------

/// What the sharing state machine works with: the app's store, keychain, loaded model and
/// busy counter (fakes in tests).
pub struct Env {
    pub store: store::Store,
    pub secrets: Arc<dyn SecretStore>,
    pub model: Arc<dyn proxy::ModelSource>,
    pub busy: Arc<BusyCounter>,
}

impl Env {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            store: state.store.clone(),
            secrets: state.secrets.clone(),
            model: Arc::new(RuntimeModel(state.runtime.clone())),
            busy: state.model_busy.clone(),
        }
    }
}

/// The runtime manager as the proxy sees it.
struct RuntimeModel(Arc<RuntimeManager>);

#[async_trait::async_trait]
impl proxy::ModelSource for RuntimeModel {
    async fn current(&self) -> proxy::Upstream {
        use crate::dto::RuntimeStateDto;
        match self.0.status().state {
            RuntimeStateDto::Ready => match self.0.endpoint().await {
                Some((base_url, key, model, n_ctx)) => proxy::Upstream::Ready {
                    base_url,
                    key,
                    model,
                    n_ctx,
                },
                None => proxy::Upstream::NotLoaded,
            },
            // A reload stops the old server before it starts the new one.
            RuntimeStateDto::Starting | RuntimeStateDto::Stopping => proxy::Upstream::Loading,
            RuntimeStateDto::Stopped | RuntimeStateDto::Failed => proxy::Upstream::NotLoaded,
        }
    }
}

/// The sharing state, managed by Tauri next to [`AppState`].
#[derive(Default)]
pub struct Sharing {
    /// Serialises the commands, and holds the listening proxy.
    inner: tokio::sync::Mutex<Inner>,
    /// The sharing key in memory (empty until read or made), read by the proxy.
    key: Arc<RwLock<String>>,
    counters: Arc<proxy::Counters>,
}

#[derive(Default)]
struct Inner {
    running: Option<proxy::Running>,
    error: Option<String>,
}

/// "Port N is used by another program. Choose another port."
pub fn port_taken(port: u16) -> String {
    format!("Port {port} is used by another program. Choose another port.")
}

fn keychain_error(e: store::secrets::SecretError) -> AppError {
    AppError::new(
        "keychain",
        "The sharing key could not be read from or saved to the system keychain.",
        "Unlock the keychain and try again.",
        e.to_string(),
    )
}

/// A fresh sharing key: the prefix and 32 hex digits (128 bits) from the CSPRNG.
pub fn new_key() -> String {
    let bytes: [u8; 16] = rand::rng().random();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{KEY_PREFIX}{hex}")
}

/// A key in the form [`new_key`] makes.
pub fn is_key(value: &str) -> bool {
    value.strip_prefix(KEY_PREFIX).is_some_and(|hex| {
        hex.len() == 32
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

impl Sharing {
    async fn saved(&self, env: &Env) -> Option<ShareSettings> {
        match env.store.get_setting::<ShareSettings>(SETTINGS_KEY).await {
            Ok(saved) => saved,
            Err(e) => {
                tracing::warn!("the sharing settings could not be read: {e}");
                None
            }
        }
    }

    async fn save(&self, env: &Env, settings: ShareSettings) -> AppResult<()> {
        env.store.set_setting(SETTINGS_KEY, &settings).await?;
        Ok(())
    }

    /// The sharing key: from memory, else from the keychain, else a new one saved there.
    async fn ensure_key(&self, env: &Env) -> AppResult<String> {
        if let Ok(key) = self.key.read()
            && !key.is_empty()
        {
            return Ok(key.clone());
        }
        let secrets = env.secrets.clone();
        let stored = tokio::task::spawn_blocking(move || secrets.get(KEY_SECRET))
            .await
            .map_err(AppError::internal)?
            .map_err(keychain_error)?;
        let key = match stored.filter(|k| is_key(k)) {
            Some(key) => key,
            None => {
                let key = new_key();
                self.store_key(env, &key).await?;
                key
            }
        };
        self.remember(&key);
        Ok(key)
    }

    async fn store_key(&self, env: &Env, key: &str) -> AppResult<()> {
        let secrets = env.secrets.clone();
        let value = key.to_owned();
        tokio::task::spawn_blocking(move || secrets.set(KEY_SECRET, &value))
            .await
            .map_err(AppError::internal)?
            .map_err(keychain_error)
    }

    fn remember(&self, key: &str) {
        if let Ok(mut held) = self.key.write() {
            *held = key.to_owned();
        }
    }

    fn deps(&self, env: &Env, port: u16) -> proxy::Deps {
        proxy::Deps {
            port,
            key: self.key.clone(),
            model: env.model.clone(),
            store: env.store.clone(),
            busy: env.busy.clone(),
            counters: self.counters.clone(),
            pictures: crate::attachments::PictureWorker::app(),
        }
    }

    fn serve(&self, env: &Env, listener: tokio::net::TcpListener) -> AppResult<proxy::Running> {
        let port = listener.local_addr().map_err(AppError::internal)?.port();
        proxy::serve(listener, self.deps(env, port)).map_err(AppError::internal)
    }

    /// The status the page shows.
    pub async fn status(&self, env: &Env) -> ShareStatus {
        let inner = self.inner.lock().await;
        self.status_of(env, &inner).await
    }

    /// Sharing as the Coding apps see it (WP6): the kept port, the switch, the port the
    /// proxy listens on now and, only while it listens, the sharing key the proxy checks
    /// (read from the keychain when sharing was switched on; never from an environment).
    pub async fn now(&self, env: &Env) -> ShareNow {
        let inner = self.inner.lock().await;
        let saved = self.saved(env).await;
        let enabled = saved.is_some_and(|s| s.enabled);
        let listening = inner
            .running
            .as_ref()
            .filter(|r| enabled && !r.is_finished())
            .map(|r| r.port());
        let key = listening
            .and_then(|_| self.key.read().ok().map(|k| k.clone()))
            .filter(|k| is_key(k));
        ShareNow {
            port: saved.map(|s| s.port),
            enabled,
            listening,
            key,
        }
    }

    async fn status_of(&self, env: &Env, inner: &Inner) -> ShareStatus {
        let saved = self.saved(env).await;
        let listening = inner.running.as_ref().is_some_and(|r| !r.is_finished());
        let model_ctx = match env.model.current().await {
            proxy::Upstream::Ready { n_ctx, .. } => Some(n_ctx),
            _ => None,
        };
        let enabled = saved.is_some_and(|s| s.enabled);
        ShareStatus {
            enabled,
            port: saved.map(|s| s.port),
            listening,
            error: if enabled && !listening {
                inner
                    .error
                    .clone()
                    .or_else(|| Some("Sharing has stopped. Switch it off and on again.".to_owned()))
            } else {
                None
            },
            refused: self.counters.refused(),
            in_use: self.counters.active() > 0,
            model_ctx,
        }
    }

    /// Switches sharing on or off (the user's switch only).
    pub async fn set_enabled(&self, env: &Env, on: bool) -> AppResult<ShareStatus> {
        let mut inner = self.inner.lock().await;
        if on {
            self.switch_on(env, &mut inner).await?;
        } else {
            if let Some(running) = inner.running.take() {
                running.stop().await;
            }
            inner.error = None;
            if let Some(saved) = self.saved(env).await {
                self.save(
                    env,
                    ShareSettings {
                        enabled: false,
                        ..saved
                    },
                )
                .await?;
            }
        }
        Ok(self.status_of(env, &inner).await)
    }

    async fn switch_on(&self, env: &Env, inner: &mut Inner) -> AppResult<()> {
        self.ensure_key(env).await?;
        let saved = self.saved(env).await;
        if inner.running.as_ref().is_some_and(|r| !r.is_finished())
            && let Some(saved) = saved
        {
            if !saved.enabled {
                self.save(
                    env,
                    ShareSettings {
                        enabled: true,
                        ..saved
                    },
                )
                .await?;
            }
            return Ok(());
        }
        if let Some(stale) = inner.running.take() {
            stale.stop().await;
        }
        let kept = saved.map(|s| s.port).filter(|p| *p >= LOWEST_PORT);
        let listener = match kept {
            Some(port) => match proxy::bind(port).await {
                Ok(listener) => listener,
                Err(e) => {
                    tracing::warn!(port, "the sharing port could not be opened: {e}");
                    inner.error = Some(port_taken(port));
                    self.save(
                        env,
                        ShareSettings {
                            enabled: true,
                            port,
                        },
                    )
                    .await?;
                    return Ok(());
                }
            },
            None => random_listener().await.ok_or_else(|| {
                AppError::new(
                    "share_no_port",
                    "No free port was found for sharing.",
                    "Close some programs and try again.",
                    String::new(),
                )
            })?,
        };
        let running = self.serve(env, listener)?;
        let port = running.port();
        self.counters.reset_refused();
        inner.running = Some(running);
        inner.error = None;
        self.save(
            env,
            ShareSettings {
                enabled: true,
                port,
            },
        )
        .await?;
        tracing::info!(port, "sharing with other apps is on");
        Ok(())
    }

    /// Chooses another port; 0 picks a free one at random from [`PORT_RANGE`]. When the
    /// port is taken nothing changes and the error says so.
    pub async fn set_port(&self, env: &Env, port: u16) -> AppResult<ShareStatus> {
        let mut inner = self.inner.lock().await;
        if port != 0 && port < LOWEST_PORT {
            return Err(AppError::bad_request("Choose a port from 1024 to 65535."));
        }
        // The port it already listens on: nothing to move.
        if port != 0
            && inner
                .running
                .as_ref()
                .is_some_and(|r| r.port() == port && !r.is_finished())
        {
            return Ok(self.status_of(env, &inner).await);
        }
        let listener = if port == 0 {
            random_listener()
                .await
                .ok_or_else(|| AppError::bad_request("No free port was found. Try again."))?
        } else {
            proxy::bind(port).await.map_err(|e| {
                tracing::warn!(port, "the chosen sharing port could not be opened: {e}");
                AppError::new(
                    "share_port_taken",
                    port_taken(port),
                    "Choose another port.",
                    e.to_string(),
                )
            })?
        };
        let port = listener.local_addr().map_err(AppError::internal)?.port();
        let enabled = self.saved(env).await.is_some_and(|s| s.enabled);
        if enabled {
            if let Some(old) = inner.running.take() {
                old.stop().await;
            }
            inner.running = Some(self.serve(env, listener)?);
            inner.error = None;
        } else {
            drop(listener);
        }
        self.save(env, ShareSettings { enabled, port }).await?;
        Ok(self.status_of(env, &inner).await)
    }

    /// The sharing key, shown only on request (made now if there is none yet).
    pub async fn reveal_key(&self, env: &Env) -> AppResult<String> {
        let _inner = self.inner.lock().await;
        self.ensure_key(env).await
    }

    /// Replaces the sharing key; the old one stops working at once.
    pub async fn replace_key(&self, env: &Env) -> AppResult<ShareStatus> {
        let inner = self.inner.lock().await;
        let key = new_key();
        self.store_key(env, &key).await?;
        self.remember(&key);
        Ok(self.status_of(env, &inner).await)
    }

    /// The setup card for `app`, with the kept port, the key and the loaded model's
    /// context.
    pub async fn setup(&self, env: &Env, app: ShareApp) -> AppResult<ShareSetup> {
        let _inner = self.inner.lock().await;
        let Some(saved) = self.saved(env).await else {
            return Err(AppError::bad_request(
                "Switch sharing on first, so the setup has a port and a key.",
            ));
        };
        let key = self.ensure_key(env).await?;
        let ctx = match env.model.current().await {
            proxy::Upstream::Ready { n_ctx, .. } => Some(n_ctx),
            _ => None,
        };
        Ok(setup_text::build(
            app,
            &setup_text::SetupInput {
                port: saved.port,
                key,
                model: proxy::MODEL_ALIAS.to_owned(),
                ctx,
            },
        ))
    }

    /// At start: listens again if the user left sharing on. Never switches it on.
    pub async fn start_saved(&self, env: &Env) {
        if !self.saved(env).await.is_some_and(|s| s.enabled) {
            return;
        }
        let mut inner = self.inner.lock().await;
        if let Err(e) = self.switch_on(env, &mut inner).await {
            inner.error = Some(e.message.clone());
        }
    }

    /// Stops listening (tests; at exit the process ends and the port closes with it).
    #[cfg(test)]
    pub async fn shutdown(&self) {
        if let Some(running) = self.inner.lock().await.running.take() {
            running.stop().await;
        }
    }
}

/// A listener on a random free port of [`PORT_RANGE`].
async fn random_listener() -> Option<tokio::net::TcpListener> {
    for _ in 0..PORT_TRIES {
        let port = rand::rng().random_range(PORT_RANGE);
        if let Ok(listener) = proxy::bind(port).await {
            return Some(listener);
        }
    }
    None
}

/// Starts the proxy at launch when the user left sharing on.
pub fn start_saved(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let (Some(state), Some(sharing)) =
            (app.try_state::<AppState>(), app.try_state::<Sharing>())
        else {
            return;
        };
        sharing.start_saved(&Env::from_state(&state)).await;
    });
}

// ---- Commands (main window only) ---------------------------------------------------

#[tauri::command]
pub async fn share_status(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    sharing: State<'_, Sharing>,
) -> AppResult<ShareStatus> {
    crate::commands::only_main_window(window.label())?;
    Ok(sharing.status(&Env::from_state(&state)).await)
}

/// Switches sharing on or off.
#[tauri::command]
pub async fn set_share_enabled(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    sharing: State<'_, Sharing>,
    on: bool,
) -> AppResult<ShareStatus> {
    crate::commands::only_main_window(window.label())?;
    sharing.set_enabled(&Env::from_state(&state), on).await
}

/// Chooses another port (0: a free one at random).
#[tauri::command]
pub async fn set_share_port(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    sharing: State<'_, Sharing>,
    port: u16,
) -> AppResult<ShareStatus> {
    crate::commands::only_main_window(window.label())?;
    sharing.set_port(&Env::from_state(&state), port).await
}

/// The sharing key, shown only on request.
#[tauri::command]
pub async fn reveal_share_key(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    sharing: State<'_, Sharing>,
) -> AppResult<String> {
    crate::commands::only_main_window(window.label())?;
    sharing.reveal_key(&Env::from_state(&state)).await
}

/// Replaces the sharing key.
#[tauri::command]
pub async fn new_share_key(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    sharing: State<'_, Sharing>,
) -> AppResult<ShareStatus> {
    crate::commands::only_main_window(window.label())?;
    sharing.replace_key(&Env::from_state(&state)).await
}

/// The setup card for one app.
#[tauri::command]
pub async fn share_setup(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    sharing: State<'_, Sharing>,
    app: ShareApp,
) -> AppResult<ShareSetup> {
    crate::commands::only_main_window(window.label())?;
    sharing.setup(&Env::from_state(&state), app).await
}
