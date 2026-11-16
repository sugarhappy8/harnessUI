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

/// How many times the model may be called in one turn before the turn wraps up (8.2).
pub const STEPS_PER_TURN: &str = "stepsPerTurn";
/// Enough for a task of a few dozen tool calls (installing a toolchain took more than 25
/// on a real PC), and still a stop for a model that goes on without getting anywhere.
pub const DEFAULT_STEPS_PER_TURN: u32 = 50;

/// A stored step limit, if it is a whole number the app accepts.
pub fn parse_steps_per_turn(value: &serde_json::Value) -> Option<u32> {
    value
        .as_u64()
        .filter(|n| (5..=500).contains(n))
        .and_then(|n| u32::try_from(n).ok())
}

/// The step limit: the stored setting, or the default when it is missing or not sensible.
pub fn steps_per_turn(stored: Option<&serde_json::Value>) -> u32 {
    stored
        .and_then(parse_steps_per_turn)
        .unwrap_or(DEFAULT_STEPS_PER_TURN)
}

/// [0.0.7] "Stop when the model makes the same call this many times in a row" (8.2, loop
/// detection). Read when a turn starts; the model is told it repeats itself two calls before
/// the limit (one call before at 3; at 2 there is no room for a warning).
pub const SAME_CALL_LIMIT: &str = "sameCallLimit";

/// [0.0.7] A repeat limit as it may be saved: a whole number from 2 to 100.
pub fn parse_same_call_limit(value: &serde_json::Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| (agent::turn::MIN_REPEAT_LIMIT..=agent::turn::MAX_REPEAT_LIMIT).contains(n))
}

/// [0.0.7] The repeat limit a turn uses: the stored whole number held to 2..=100 (one written
/// some other way outside that range counts as the nearest end, a whole number written with a
/// fraction such as 42.0 as that number), or 5 when it is missing, negative or not a whole
/// number. The settings panel reads it the same way (`storedSameCallLimit`). A stored value can
/// never end every turn at its first call, nor let a loop run on.
pub fn same_call_limit(stored: Option<&serde_json::Value>) -> u32 {
    let whole = stored.and_then(|value| {
        value.as_u64().or_else(|| {
            // 42.0, or a number past u64 such as 2^64: whole, so held to the range.
            value
                .as_f64()
                .filter(|n| n.is_finite() && n.fract() == 0.0 && *n >= 0.0)
                .map(|n| if n >= f64::from(u32::MAX) { u64::from(u32::MAX) } else { n as u64 })
        })
    });
    match whole {
        Some(n) => agent::turn::repeat_limit(u32::try_from(n).unwrap_or(u32::MAX)),
        None => agent::turn::LOOP_STOP_AT,
    }
}

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

/// 0.0.5: without the upgrade rule; loads read `load_settings::read_defaults` instead.
/// No caller is left; the lead may remove it.
#[allow(dead_code, reason = "0.0.6 WP1b: no callers left")]
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

/// Remembers a model file so it appears first in the model picker: its canonical path,
/// with any older spelling of the same file dropped (0.0.6 item 8, `model_options`). The
/// chat that opened it through a link or a mapped drive still sees that spelling there.
pub async fn remember_local_model(state: &AppState, path: &str) {
    crate::model_options::remember(&state.store, path).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_step_limit_is_the_setting_or_the_default() {
        assert_eq!(steps_per_turn(None), DEFAULT_STEPS_PER_TURN);
        assert_eq!(steps_per_turn(Some(&json!(100))), 100);
        assert_eq!(steps_per_turn(Some(&json!(5))), 5);
        assert_eq!(steps_per_turn(Some(&json!(500))), 500);
        // Not a sensible limit: zero would end every turn at once, and a model that
        // loops must still be stopped.
        for bad in [
            json!(0),
            json!(4),
            json!(501),
            json!(-3),
            json!(12.5),
            json!("50"),
            json!(null),
        ] {
            assert_eq!(parse_steps_per_turn(&bad), None, "{bad}");
            assert_eq!(steps_per_turn(Some(&bad)), DEFAULT_STEPS_PER_TURN, "{bad}");
        }
    }

    /// [0.0.7] The repeat limit: 2 to 100 may be saved; what a turn reads is held to that
    /// range, and anything that is not a whole number gives the default, 5.
    #[test]
    fn the_same_call_limit_is_checked_when_saved_and_held_to_its_range_when_read() {
        assert_eq!(same_call_limit(None), 5);
        for n in [2, 3, 5, 42, 100] {
            assert_eq!(parse_same_call_limit(&json!(n)), Some(n), "{n}");
            assert_eq!(same_call_limit(Some(&json!(n))), n, "{n}");
        }
        for bad in [
            json!(0),
            json!(1),
            json!(101),
            json!(-3),
            json!(12.5),
            json!("5"),
            json!(null),
            json!({"limit": 5}),
            json!(u64::MAX),
        ] {
            assert_eq!(parse_same_call_limit(&bad), None, "{bad}");
        }
        // Tampered with in the database: held to the range, or the default.
        for (stored, used) in [
            (json!(0), 2),
            (json!(1), 2),
            (json!(101), 100),
            (json!(u64::MAX), 100),
            (json!(-3), 5),
            (json!(12.5), 5),
            (json!("100"), 5),
            (json!(null), 5),
            // [0.0.7 verification] As the settings panel reads them: a whole number written
            // with a fraction is that number, one past u64 the top of the range.
            (json!(42.0), 42),
            (json!(1.0), 2),
            (json!(-0.0), 2),
            (json!(-3.0), 5),
            (json!(18_446_744_073_709_551_616.0_f64), 100),
            (json!(1e300), 100),
        ] {
            assert_eq!(same_call_limit(Some(&stored)), used, "{stored}");
        }
    }

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
