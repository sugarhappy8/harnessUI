//! Tested settings per model: the profile a capability check saves for one model on one
//! backend, and how a chat's requests use it.
//!
//! A profile only changes request defaults: whether thinking is on, and the temperature.
//! [`apply`] works on [`RequestDefaults`], which holds those two values and nothing else,
//! so a profile cannot reach approval modes, grants, approved folders or tool switches.
//! And it never overrides what the user chose: thinking switched on or off in a chat
//! header, or a temperature set in the request template.
//!
//! Stored in the settings table under `modelProfile:<hash>` (see [`Identity::key`]). The
//! generic `set_setting` command does not accept that key (its allow-list in
//! `models_cmd.rs`); profiles are written only by a capability check the user started
//! and removed by "Forget" ([`super::commands`]).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::state::AppState;

/// The start of every profile's settings key.
pub const KEY_PREFIX: &str = "modelProfile:";
/// The profile format.
pub const VERSION: u32 = 1;

/// Which model on which backend a profile belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Identity {
    /// A model file run by the app's own llama-server, on one backend (`vulkan`, `cpu`…).
    Local {
        file: String,
        size: u64,
        backend: String,
    },
    /// A model of an OpenAI-compatible endpoint.
    Remote { url: String, model: String },
}

impl Identity {
    /// `modelProfile:` and the first 16 hex digits of the SHA-256 of the identity.
    pub fn key(&self) -> String {
        let canonical = match self {
            Identity::Local {
                file,
                size,
                backend,
            } => format!("local\n{file}\n{size}\n{}", backend.to_ascii_lowercase()),
            Identity::Remote { url, model } => {
                format!("remote\n{}\n{model}", url.trim_end_matches('/'))
            }
        };
        let digest = Sha256::digest(canonical.as_bytes());
        let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        format!("{KEY_PREFIX}{hex}")
    }

    /// The name shown in Settings.
    pub fn label(&self) -> String {
        match self {
            Identity::Local { file, backend, .. } => format!("{file} ({backend})"),
            Identity::Remote { model, .. } => model.clone(),
        }
    }
}

/// What a profile sets. `None` leaves the app's own default.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Defaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

/// A saved profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub version: u32,
    pub identity: Identity,
    pub label: String,
    /// Unix ms.
    pub checked_at: i64,
    /// The variant that was chosen, in words; `None` when no variant passed a probe.
    pub chosen: Option<String>,
    /// What chats with this model use; `None` changes nothing.
    pub defaults: Option<Defaults>,
    /// Every variant's results, for the table in Settings.
    pub results: Vec<super::probes::VariantResult>,
}

/// The request settings of one turn that a profile may change. Nothing else is reachable
/// from [`apply`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RequestDefaults {
    pub thinking: bool,
    pub temperature: Option<f32>,
}

/// What the user set explicitly for this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UserChoices {
    /// The user switched thinking on or off in this chat.
    pub thinking: bool,
    /// The request already carries a temperature.
    pub temperature: bool,
}

/// Fills in the profile's defaults where the user chose nothing.
pub fn apply(settings: &mut RequestDefaults, profile: Option<&Defaults>, user: UserChoices) {
    let Some(profile) = profile else {
        return;
    };
    if !user.thinking
        && let Some(thinking) = profile.thinking
    {
        settings.thinking = thinking;
    }
    if !user.temperature && profile.temperature.is_some() {
        settings.temperature = profile.temperature;
    }
}

/// Whether the user chose thinking for a chat. A chat stored before the marker existed
/// counts as chosen when thinking is off: off was only ever stored by the user's switch.
pub fn user_chose_thinking(prefs: &crate::turns::ChatTools) -> bool {
    prefs.thinking_chosen || prefs.thinking_off
}

/// The identity of the model behind a chat's model reference, as it runs now. A local
/// model has one only while it is the loaded model (its backend is known then).
pub async fn identity_for(state: &AppState, model_ref: &str) -> Option<Identity> {
    if let Some(path) = model_ref.strip_prefix("local:") {
        let (_, _, loaded, _) = state.runtime.endpoint().await?;
        if loaded.to_string_lossy() != path {
            return None;
        }
        let backend = state.runtime.status().backend?;
        return local_identity(std::path::Path::new(path), &backend);
    }
    let rest = model_ref.strip_prefix("remote:")?;
    let (id, model) = rest.split_once(':')?;
    let id: i64 = id.parse().ok()?;
    let provider = state
        .store
        .list_providers()
        .await
        .ok()?
        .into_iter()
        .find(|p| p.id.0 == id)?;
    Some(Identity::Remote {
        url: provider.base_url.trim_end_matches('/').to_owned(),
        model: model.to_owned(),
    })
}

/// A model file's identity: its name and size, and the backend it runs on.
pub fn local_identity(path: &std::path::Path, backend: &str) -> Option<Identity> {
    Some(Identity::Local {
        file: path.file_name()?.to_string_lossy().into_owned(),
        size: std::fs::metadata(path).ok()?.len(),
        backend: backend.to_ascii_lowercase(),
    })
}

/// The saved profile for an identity.
pub async fn load(store: &store::Store, identity: &Identity) -> Option<Profile> {
    let profile: Profile = store.get_setting(&identity.key()).await.ok().flatten()?;
    (profile.version == VERSION && &profile.identity == identity).then_some(profile)
}

/// The profile of the model behind a chat's model reference, if it has one.
pub async fn for_model(state: &AppState, model_ref: &str) -> Option<Profile> {
    let identity = identity_for(state, model_ref).await?;
    load(&state.store, &identity).await
}

/// Applies the model's profile to a chat turn's input: its thinking default (unless the
/// user chose) and its temperature (unless the request has one). One call from
/// `turns::run`, marked `[0.0.4 profile]`.
pub async fn apply_to_turn(
    state: &AppState,
    model_ref: &str,
    prefs: &crate::turns::ChatTools,
    input: &mut agent::turn::TurnInput,
) {
    let Some(profile) = for_model(state, model_ref).await else {
        return;
    };
    apply_to_input(input, profile.defaults.as_ref(), user_chose_thinking(prefs));
}

fn apply_to_input(
    input: &mut agent::turn::TurnInput,
    profile: Option<&Defaults>,
    chose_thinking: bool,
) {
    let mut settings = RequestDefaults {
        thinking: input.thinking,
        temperature: input.template.temperature,
    };
    let user = UserChoices {
        thinking: chose_thinking,
        temperature: input.template.temperature.is_some(),
    };
    apply(&mut settings, profile, user);
    input.thinking = settings.thinking;
    input.template.temperature = settings.temperature;
}

/// Whether thinking is on for a chat as its next turn would have it: the user's choice,
/// else the model's profile, else on. For the chat header (`chat::get_chat`).
pub async fn chat_thinking(state: &AppState, chat: &store::Chat) -> bool {
    let prefs: crate::turns::ChatTools =
        serde_json::from_value(chat.tool_prefs.clone()).unwrap_or_default();
    let mut settings = RequestDefaults {
        thinking: !prefs.thinking_off,
        temperature: None,
    };
    if user_chose_thinking(&prefs) {
        return settings.thinking;
    }
    let Some(model_ref) = chat.model_ref.as_deref() else {
        return settings.thinking;
    };
    if let Some(profile) = for_model(state, model_ref).await {
        apply(
            &mut settings,
            profile.defaults.as_ref(),
            UserChoices::default(),
        );
    }
    settings.thinking
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::turn::{ModelCaps, RequestTemplate, TurnInput};

    fn off_cool() -> Defaults {
        Defaults {
            thinking: Some(false),
            temperature: Some(0.2),
        }
    }

    #[test]
    fn the_users_explicit_choice_beats_the_profile() {
        for chosen in [true, false] {
            let mut settings = RequestDefaults {
                thinking: chosen,
                temperature: None,
            };
            let profile = Defaults {
                thinking: Some(!chosen),
                temperature: None,
            };
            apply(
                &mut settings,
                Some(&profile),
                UserChoices {
                    thinking: true,
                    temperature: false,
                },
            );
            assert_eq!(settings.thinking, chosen);
        }
        let mut settings = RequestDefaults {
            thinking: true,
            temperature: Some(0.9),
        };
        apply(
            &mut settings,
            Some(&off_cool()),
            UserChoices {
                thinking: false,
                temperature: true,
            },
        );
        assert_eq!(
            settings,
            RequestDefaults {
                thinking: false,
                temperature: Some(0.9)
            }
        );
    }

    #[test]
    fn without_a_profile_nothing_changes() {
        for thinking in [true, false] {
            for temperature in [None, Some(0.7)] {
                let before = RequestDefaults {
                    thinking,
                    temperature,
                };
                let mut after = before;
                apply(&mut after, None, UserChoices::default());
                assert_eq!(after, before);
            }
        }
        // A profile whose check passed nothing has no defaults either.
        let mut input = TurnInput::new(Vec::new(), ModelCaps::default(), RequestTemplate::default());
        apply_to_input(&mut input, None, false);
        assert!(input.thinking);
        assert_eq!(input.template.temperature, None);
    }

    #[test]
    fn a_profile_fills_in_what_the_user_did_not_choose() {
        let mut input = TurnInput::new(Vec::new(), ModelCaps::default(), RequestTemplate::default());
        apply_to_input(&mut input, Some(&off_cool()), false);
        assert!(!input.thinking);
        assert_eq!(input.template.temperature, Some(0.2));
    }

    /// A profile reaches thinking and temperature and nothing else of the turn.
    #[test]
    fn a_profile_changes_nothing_but_thinking_and_temperature() {
        let mut input = TurnInput::new(
            vec![agent::HistoryMessage::user("hi")],
            ModelCaps {
                thinking: true,
                ..ModelCaps::default()
            },
            RequestTemplate {
                model: "fake-model".into(),
                top_p: Some(0.9),
                ..Default::default()
            },
        );
        input.system_prompt = "system".into();
        input.step_cap = 7;
        input.pinned = vec!["pin".into()];
        let before = format!("{input:?}");
        apply_to_input(&mut input, Some(&off_cool()), false);
        assert!(!input.thinking);
        assert_eq!(input.template.temperature, Some(0.2));
        input.thinking = true;
        input.template.temperature = None;
        assert_eq!(format!("{input:?}"), before);
    }

    /// The marker tells the user's choice from the default; chats stored before it existed
    /// behave exactly as before.
    #[test]
    fn an_old_chat_keeps_its_thinking() {
        let prefs = |value: serde_json::Value| -> crate::turns::ChatTools {
            serde_json::from_value(value).unwrap()
        };
        assert!(!user_chose_thinking(&prefs(serde_json::json!({}))));
        assert!(user_chose_thinking(&prefs(serde_json::json!({"thinkingOff": true}))));
        assert!(user_chose_thinking(&prefs(serde_json::json!({"thinkingChosen": true}))));
        // Not stored when not chosen, so an untouched chat's stored value is unchanged.
        let untouched = serde_json::to_value(prefs(serde_json::json!({"enabled": ["a"]}))).unwrap();
        assert_eq!(untouched, serde_json::json!({"enabled": ["a"]}));
    }

    #[test]
    fn the_key_names_the_model_and_the_backend() {
        let local = |backend: &str, size: u64| Identity::Local {
            file: "m.gguf".into(),
            size,
            backend: backend.into(),
        };
        let key = local("vulkan", 10).key();
        assert!(key.starts_with(KEY_PREFIX) && key.len() == KEY_PREFIX.len() + 16, "{key}");
        assert_eq!(key, local("Vulkan", 10).key());
        assert_ne!(key, local("cpu", 10).key());
        assert_ne!(key, local("vulkan", 11).key());
        let remote = |url: &str| Identity::Remote {
            url: url.into(),
            model: "fake-model".into(),
        };
        assert_eq!(remote("http://h/v1").key(), remote("http://h/v1/").key());
        assert_ne!(remote("http://h/v1").key(), key);
    }

    /// The generic settings command cannot write a profile: its allow-list names plain UI
    /// preferences only.
    #[test]
    fn set_setting_does_not_accept_a_profile_key() {
        let source = include_str!("../models_cmd.rs");
        let start = source.find("const ALLOWED").expect("the allow-list");
        let list = &source[start..start + source[start..].find("];").unwrap()];
        assert!(!list.contains("modelProfile") && !list.contains("profile"), "{list}");
        assert!(!list.contains("KEY_PREFIX"), "{list}");
    }

    #[tokio::test]
    async fn a_profile_is_found_only_for_its_own_identity() {
        let store = store::Store::open_in_memory().unwrap();
        let identity = Identity::Remote {
            url: "http://h/v1".into(),
            model: "fake-model".into(),
        };
        let profile = Profile {
            version: VERSION,
            identity: identity.clone(),
            label: identity.label(),
            checked_at: 1,
            chosen: Some("Default".into()),
            defaults: Some(Defaults::default()),
            results: Vec::new(),
        };
        store.set_setting(&identity.key(), &profile).await.unwrap();
        assert_eq!(load(&store, &identity).await, Some(profile.clone()));
        let other = Identity::Remote {
            url: "http://h/v1".into(),
            model: "other".into(),
        };
        assert_eq!(load(&store, &other).await, None);
        // A value under the right key that names another model is not used.
        store.set_setting(&other.key(), &profile).await.unwrap();
        assert_eq!(load(&store, &other).await, None);
    }
}
