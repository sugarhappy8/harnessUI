//! Small typed accessors over the settings table.

use tauri::{AppHandle, Manager};

use crate::dto::LocalModelSettings;
use crate::state::AppState;

pub const LOCAL_MODEL: &str = "localModel";
pub const RECENT_LOCAL_MODELS: &str = "recentLocalModels";
pub const DEFAULT_SYSTEM_PROMPT: &str = "defaultSystemPrompt";
/// The context size, in tokens, of a remote endpoint whose model reports none (8.4).
pub const REMOTE_CONTEXT_LENGTH: &str = "remoteContextLength";
/// A cautious default: Ollama runs at 4,096 unless told otherwise and drops the oldest
/// messages without saying so when a request is larger.
pub const DEFAULT_REMOTE_CONTEXT: u32 = 4096;

/// A stored remote context size, if it is a sensible whole number of tokens.
pub fn parse_remote_context(value: &serde_json::Value) -> Option<u32> {
    value
        .as_u64()
        .filter(|n| (1024..=1_048_576).contains(n))
        .and_then(|n| u32::try_from(n).ok())
}

/// The remote context size: the stored setting, or the default when it is missing or not
/// a sensible number.
pub fn remote_context_length(stored: Option<&serde_json::Value>) -> u32 {
    stored
        .and_then(parse_remote_context)
        .unwrap_or(DEFAULT_REMOTE_CONTEXT)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 8.4: a remote endpoint that reports no context gets the user's setting, else 4,096.
    #[test]
    fn remote_context_falls_back_to_4096() {
        assert_eq!(remote_context_length(None), 4096);
        assert_eq!(remote_context_length(Some(&json!(16384))), 16384);
        assert_eq!(remote_context_length(Some(&json!(2048))), 2048);
        for bad in [
            json!(null),
            json!("8192"),
            json!(-1),
            json!(0),
            json!(8192.5),
            json!(100),
            json!(10_000_000_000u64),
            json!({"n": 8192}),
        ] {
            assert_eq!(remote_context_length(Some(&bad)), 4096, "{bad}");
        }
    }
}
