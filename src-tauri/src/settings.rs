//! Small typed accessors over the settings table.

use tauri::{AppHandle, Manager};

use crate::dto::LocalModelSettings;
use crate::state::AppState;

pub const LOCAL_MODEL: &str = "localModel";
pub const RECENT_LOCAL_MODELS: &str = "recentLocalModels";
pub const DEFAULT_SYSTEM_PROMPT: &str = "defaultSystemPrompt";

pub fn provider_models_key(provider_id: store::ProviderId) -> String {
    format!("providerModels:{}", provider_id.0)
}

pub async fn local_model_settings(app: &AppHandle) -> LocalModelSettings {
    let state = app.state::<AppState>();
    state
        .store
        .get_setting::<LocalModelSettings>(LOCAL_MODEL)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// Remembers a model file so it appears in the model picker.
pub async fn remember_local_model(state: &AppState, path: &str) {
    let mut recent: Vec<String> = state
        .store
        .get_setting(RECENT_LOCAL_MODELS)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    recent.retain(|p| p != path);
    recent.insert(0, path.to_string());
    recent.truncate(20);
    if let Err(e) = state.store.set_setting(RECENT_LOCAL_MODELS, &recent).await {
        tracing::warn!("could not save recent models: {e}");
    }
}
