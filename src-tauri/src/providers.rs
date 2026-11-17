//! Endpoints the user adds: OpenAI-compatible servers (Ollama, LM Studio, vLLM, others) and
//! (0.0.7) the ready-made cloud companies: Anthropic (Claude's own Messages API), OpenAI,
//! OpenRouter and abliteration.ai. API keys live in the OS keychain; the database keeps
//! only a reference.

use agent::{ClientConfig, EndpointPlace, OpenAiClient, ProviderKind};
use tauri::State;

use crate::dto::{ModelOptionDto, ProviderDto, SaveProvider};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::studio::catalog;

pub(crate) fn kind_of(s: &str) -> ProviderKind {
    match s {
        "ollama" => ProviderKind::Ollama,
        "lmstudio" => ProviderKind::LmStudio,
        "vllm" => ProviderKind::Vllm,
        // [0.0.7 P5] The ready-made cloud choices.
        "anthropic" => ProviderKind::Anthropic,
        "openai" => ProviderKind::OpenAi,
        "openrouter" => ProviderKind::OpenRouter,
        "abliteration" => ProviderKind::Abliteration,
        _ => ProviderKind::Generic,
    }
}

fn secret_key(provider_id: store::ProviderId) -> String {
    format!("provider-api-key:{}", provider_id.0)
}

/// [0.0.7 P5] The settings key of the context size chosen for one endpoint.
pub fn context_key(provider_id: store::ProviderId) -> String {
    format!("endpointContext:{}", provider_id.0)
}

fn dto(p: &store::Provider, has_key: bool, context_length: Option<u32>) -> ProviderDto {
    ProviderDto {
        id: p.id,
        name: p.name.clone(),
        kind: p.kind.clone(),
        base_url: p.base_url.clone(),
        has_api_key: has_key,
        enabled: p.enabled,
        cloud: agent::endpoint_place(&p.base_url) == EndpointPlace::Internet,
        context_length,
    }
}

/// [0.0.7 P5] The context size chosen for an endpoint in Settings → Endpoints, if one was.
async fn chosen_context(state: &AppState, id: store::ProviderId) -> Option<u32> {
    let stored: Option<serde_json::Value> = state
        .store
        .get_setting(&context_key(id))
        .await
        .ok()
        .flatten();
    stored
        .as_ref()
        .and_then(crate::settings::parse_remote_context)
}

/// [0.0.7 P5] How a model's context is decided, from what is known. A size chosen for the
/// endpoint in Settings → Endpoints: that, at most what the server reports. "Automatic":
/// what the server reports, at most the company's size for a cloud company
/// ([`agent::default_context`]: a long chat sends up to this much with every message, and
/// Claude's current models report 1,000,000), and when the server does not say, the
/// company's size, else `setting` (the user's size for servers that do not say, 4,096 by
/// default).
pub fn plan_context(
    kind: ProviderKind,
    model: &str,
    reported: Option<u32>,
    chosen: Option<u32>,
    setting: u32,
) -> u32 {
    let company = agent::default_context(kind, model);
    match (reported, chosen) {
        (Some(reported), Some(chosen)) => reported.min(chosen),
        (None, Some(chosen)) => chosen,
        (Some(reported), None) => company.map_or(reported, |size| reported.min(size)),
        (None, None) => company.unwrap_or(setting),
    }
}

async fn global_context_setting(state: &AppState) -> u32 {
    let stored: Option<serde_json::Value> = state
        .store
        .get_setting(crate::settings::REMOTE_CONTEXT_LENGTH)
        .await
        .ok()
        .flatten();
    crate::settings::remote_context_length(stored.as_ref())
}

/// [0.0.7 P5] The context a turn plans with for `model` on `provider` that reported
/// `reported` ([`plan_context`]).
pub async fn context_for(
    state: &AppState,
    provider: &store::Provider,
    model: &str,
    reported: Option<u32>,
) -> u32 {
    plan_context(
        kind_of(&provider.kind),
        model,
        reported,
        chosen_context(state, provider.id).await,
        global_context_setting(state).await,
    )
}

/// [0.0.7 P5] For the Tools panel's guess of a chat's context (`mcp_cmd`): what the server
/// reported, held to the endpoint's chosen size, and the size used when it reports nothing.
pub async fn context_parts(
    state: &AppState,
    model_ref: &str,
    reported: Option<u32>,
) -> (Option<u32>, u32) {
    let setting = global_context_setting(state).await;
    let Some(provider) = provider_of(state, model_ref).await else {
        return (reported, setting);
    };
    let model = model_of(model_ref);
    let chosen = chosen_context(state, provider.id).await;
    let kind = kind_of(&provider.kind);
    (
        reported.map(|r| plan_context(kind, model, Some(r), chosen, setting)),
        plan_context(kind, model, None, chosen, setting),
    )
}

/// The model of a `remote:<id>:<model>` reference.
fn model_of(model_ref: &str) -> &str {
    model_ref
        .strip_prefix("remote:")
        .and_then(|rest| rest.split_once(':'))
        .map_or("", |(_, model)| model)
}

/// The endpoint a `remote:<id>:<model>` reference names, if it still exists.
async fn provider_of(state: &AppState, model_ref: &str) -> Option<store::Provider> {
    let (id, _) = model_ref.strip_prefix("remote:")?.split_once(':')?;
    let id: i64 = id.parse().ok()?;
    state
        .store
        .get_provider(store::ProviderId(id))
        .await
        .ok()
        .flatten()
}

/// [0.0.7 P5] What the chat header shows for a chat's model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatMarks {
    /// Where the chat's messages go when the endpoint is on the internet (the "Cloud" mark):
    /// its host, or the address itself when no host can be read from it.
    pub cloud_host: Option<String>,
    /// The app switches the model's thinking on and off (Claude).
    pub thinking: bool,
    /// The model's thinking cannot stop, so "off" asks it to think little (Claude Opus 5.5,
    /// Fable, Mythos).
    pub thinking_low: bool,
}

/// [0.0.7 P5] The marks of an endpoint at `base_url` of `kind`, for `model`.
pub fn marks_for(kind: ProviderKind, base_url: &str, model: &str) -> ChatMarks {
    let cloud_host = (agent::endpoint_place(base_url) == EndpointPlace::Internet)
        .then(|| agent::endpoint_host(base_url).unwrap_or_else(|| base_url.trim().to_owned()));
    let thinking = kind == ProviderKind::Anthropic;
    ChatMarks {
        cloud_host,
        thinking,
        thinking_low: thinking && !agent::thinking_can_stop(base_url, model),
    }
}

/// [0.0.7 P5] The marks of a chat's model ([`marks_for`]); none without an endpoint.
pub async fn chat_marks(state: &AppState, model_ref: Option<&str>) -> ChatMarks {
    let Some(model_ref) = model_ref else {
        return ChatMarks::default();
    };
    match provider_of(state, model_ref).await {
        Some(provider) => marks_for(
            kind_of(&provider.kind),
            &provider.base_url,
            model_of(model_ref),
        ),
        None => ChatMarks::default(),
    }
}

/// [0.0.7 P5] Whether a key saved for an endpoint at `old_base_url` may stay with it at
/// `new_base_url`: only to the same scheme, host and port, as the client reads them.
pub fn key_may_stay(old_base_url: &str, new_base_url: &str) -> bool {
    matches!(
        (agent::endpoint_origin(old_base_url), agent::endpoint_origin(new_base_url)),
        (Some(old), Some(new)) if old == new
    )
}

/// [0.0.7 P5] Whether a key may be saved for the endpoint: anything but "the keychain has
/// none" (a keychain that cannot be read may still hold one), so no check passes by default.
pub(crate) fn may_hold_key(
    secrets: &dyn store::secrets::SecretStore,
    id: store::ProviderId,
) -> bool {
    !matches!(secrets.get(&secret_key(id)), Ok(None))
}

/// [0.0.7 P5] What saving an endpoint at `base_url` does with its key before the endpoint is
/// saved. A key, new or saved, may go to `base_url` only as `agent::check_key_transport`
/// allows; a saved key never follows the endpoint to another scheme, host or port (the save
/// is refused unless a new key comes with the new address or the key is removed). A saved
/// key that must go (removal asked for, or a new key for another address) is removed here,
/// so it can never be left with the new address; when the keychain cannot remove it,
/// nothing is saved.
fn prepare_key(
    secrets: &dyn store::secrets::SecretStore,
    id: Option<store::ProviderId>,
    saved_base_url: Option<&str>,
    base_url: &str,
    new_key: bool,
    clear: bool,
) -> AppResult<()> {
    let may_hold = id.is_some_and(|id| may_hold_key(secrets, id));
    let kept = may_hold && !clear;
    if new_key || kept {
        agent::check_key_transport(base_url).map_err(AppError::bad_request)?;
    }
    let moves = saved_base_url.is_some_and(|saved| !key_may_stay(saved, base_url));
    if kept && moves && !new_key {
        return Err(AppError::bad_request(PASTE_THE_NEW_KEY));
    }
    if let Some(id) = id
        && may_hold
        && (clear || (new_key && moves))
    {
        secrets.delete(&secret_key(id)).map_err(|e| {
            AppError::new(
                "keychain",
                "The saved API key could not be removed from the system keychain, so nothing \
                 was saved.",
                "Unlock the keychain and try again.",
                e.to_string(),
            )
        })?;
    }
    Ok(())
}

/// [0.0.7 P5] What a saved key that would follow an endpoint to another address gets.
const PASTE_THE_NEW_KEY: &str = "The saved API key is not sent to a new address. Paste the API \
     key for the new address, or remove the saved key.";

/// Validates a base URL: http(s) only, written in full (`http://` or `https://`, so every
/// reader of it agrees where it leads), no credentials embedded in it.
fn check_base_url(url: &str) -> AppResult<()> {
    let lower = url.trim().to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err(AppError::bad_request(
            "The address must start with http:// or https://.",
        ));
    }
    let parsed = url::Url::parse(url).map_err(|_| {
        AppError::bad_request(
            "The address is not a valid URL (for example http://127.0.0.1:11434).",
        )
    })?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(AppError::bad_request(
            "The address must start with http:// or https://.",
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AppError::bad_request(
            "Put the API key in the API key field, not in the address.",
        ));
    }
    Ok(())
}

pub async fn client_for(
    state: &AppState,
    id: store::ProviderId,
) -> AppResult<(OpenAiClient, store::Provider)> {
    let provider = state
        .store
        .get_provider(id)
        .await?
        .ok_or_else(|| AppError::not_found("endpoint"))?;
    let api_key = state.secrets.get(&secret_key(id)).map_err(|e| {
        AppError::new(
            "keychain",
            "The API key could not be read from the system keychain.",
            "Unlock the keychain or re-enter the key in Settings → Endpoints.",
            e.to_string(),
        )
    })?;
    let client = client_from(&provider, api_key)?;
    Ok((client, provider))
}

/// [0.1.0] The client of an endpoint, with `key` (its saved key, read by the caller): the
/// same for chat and for Studio.
pub(crate) fn client_from(
    provider: &store::Provider,
    key: Option<String>,
) -> AppResult<OpenAiClient> {
    let mut config = ClientConfig::new(provider.base_url.clone(), kind_of(&provider.kind));
    config.api_key = key;
    OpenAiClient::new(config).map_err(AppError::from)
}

#[tauri::command]
pub async fn list_providers(state: State<'_, AppState>) -> AppResult<Vec<ProviderDto>> {
    let mut out = Vec::new();
    for p in state.store.list_providers().await? {
        let has_key = may_hold_key(state.secrets.as_ref(), p.id);
        let context = chosen_context(&state, p.id).await;
        out.push(dto(&p, has_key, context));
    }
    Ok(out)
}

#[tauri::command]
pub async fn save_provider(
    state: State<'_, AppState>,
    provider: SaveProvider,
) -> AppResult<ProviderDto> {
    let base_url = provider.base_url.trim().trim_end_matches('/').to_string();
    check_base_url(&base_url)?;
    let new_key = provider
        .api_key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty());
    if let Some(n) = provider.context_length
        && crate::settings::parse_remote_context(&serde_json::json!(n)).is_none()
    {
        return Err(AppError::bad_request(
            "The context size must be a whole number of tokens from 1,024 to 1,048,576.",
        ));
    }
    let name = if provider.name.trim().is_empty() {
        base_url.clone()
    } else {
        provider.name.trim().to_string()
    };
    // [0.0.7 P5] The key's checks, and its removal when it must go, before anything is
    // saved ([`prepare_key`]).
    let saved_base_url = match provider.id {
        Some(id) => state.store.get_provider(id).await?.map(|p| p.base_url),
        None => None,
    };
    prepare_key(
        state.secrets.as_ref(),
        provider.id,
        saved_base_url.as_deref(),
        &base_url,
        new_key,
        provider.clear_api_key,
    )?;
    let id = state
        .store
        .upsert_provider(store::ProviderInput {
            id: provider.id,
            name,
            kind: provider.kind.clone(),
            base_url,
            secret_ref: None,
            enabled: true,
        })
        .await?;
    let key = secret_key(id);
    if let Some(api_key) = provider.api_key.filter(|k| !k.trim().is_empty()) {
        state.secrets.set(&key, api_key.trim()).map_err(|e| {
            AppError::new(
                "keychain",
                "The API key could not be saved to the system keychain.",
                "Unlock the keychain and try again.",
                e.to_string(),
            )
        })?;
    }
    // [0.0.7 P5] The context size chosen for this endpoint; none means Automatic.
    match provider.context_length {
        Some(n) => state.store.set_setting(&context_key(id), &n).await?,
        None => {
            state.store.delete_setting(&context_key(id)).await?;
        }
    }
    let saved = state
        .store
        .get_provider(id)
        .await?
        .ok_or_else(|| AppError::not_found("endpoint"))?;
    let has_key = may_hold_key(state.secrets.as_ref(), id);
    Ok(dto(&saved, has_key, provider.context_length))
}

#[tauri::command]
pub async fn delete_provider(state: State<'_, AppState>, id: store::ProviderId) -> AppResult<()> {
    let _ = state.secrets.delete(&secret_key(id));
    let _ = state.store.delete_setting(&context_key(id)).await;
    state.store.delete_provider(id).await?;
    // [0.1.0] After the endpoint is gone, so lists still on their way are not saved.
    catalog::forget(&state.store, id).await;
    let _ = state
        .store
        .set_setting::<Vec<ModelOptionDto>>(&crate::settings::provider_models_key(id), &Vec::new())
        .await;
    Ok(())
}

/// Connects to the endpoint, lists its models and caches them for the model picker
/// (so the picker works offline). [0.1.0] For an endpoint that makes pictures or video, its
/// picture and video lists are fetched and saved too ([`catalog::refresh`]).
#[tauri::command]
pub async fn test_provider(
    state: State<'_, AppState>,
    id: store::ProviderId,
) -> AppResult<Vec<ModelOptionDto>> {
    let (client, provider) = client_for(&state, id).await?;
    // [0.0.7 P5] With the names the list gives (Anthropic, OpenRouter, abliteration.ai), and
    // without OpenAI's models that cannot chat.
    let models = client.list_model_entries().await?;
    let options: Vec<ModelOptionDto> = models
        .into_iter()
        .map(|m| ModelOptionDto {
            reference: format!("remote:{}:{}", provider.id.0, m.id),
            label: format!(
                "{} · {}",
                m.display_name.as_deref().unwrap_or(&m.id),
                provider.name
            ),
            source: "provider".to_string(),
            context_length: m.context_length,
        })
        .collect();
    state
        .store
        .set_setting(&crate::settings::provider_models_key(id), &options)
        .await?;
    // [0.1.0] A failure here is only logged: it never fails Test, and Studio offers to get
    // the lists again.
    if let Some(media) = agent::media::media_provider(client.clone()) {
        let now = jiff::Timestamp::now().as_millisecond();
        if let Err(err) = catalog::refresh(&state.store, id, &*media, now).await {
            tracing::warn!(
                "the picture and video lists of endpoint {} could not be fetched: {err}",
                id.0
            );
        }
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_rules() {
        assert!(check_base_url("http://127.0.0.1:11434").is_ok());
        assert!(check_base_url("https://example.com/v1").is_ok());
        assert!(check_base_url("ftp://example.com").is_err());
        assert!(check_base_url("https://user:pw@example.com").is_err());
        assert!(check_base_url("not a url").is_err());
        // [0.0.7 P5] Written in full, so the key check, the "Cloud" mark and the requests
        // read it alike: `http:/host`, `http:host` and `host:port` are refused.
        for half in [
            "http:/api.example.com",
            "http:api.example.com",
            "https:/api.anthropic.com",
            "http:\\\\api.example.com/v1",
            "api.example.com:8080",
        ] {
            assert!(check_base_url(half).is_err(), "{half}");
        }
        assert!(check_base_url("HTTPS://api.anthropic.com").is_ok());
    }

    /// [0.0.7 P5] A saved key stays with an edited endpoint only at the same scheme, host
    /// and port.
    #[test]
    fn a_saved_key_stays_with_its_address_only() {
        assert!(key_may_stay(
            "https://api.openai.com/v1",
            "https://api.openai.com/v1/"
        ));
        assert!(key_may_stay(
            "https://api.anthropic.com",
            "https://API.anthropic.com:443/v1"
        ));
        assert!(key_may_stay(
            "http://192.168.1.5:8000",
            "http://192.168.1.5:8000/v1"
        ));
        // Another company, host, port or scheme.
        assert!(!key_may_stay(
            "https://api.openai.com/v1",
            "https://openrouter.ai/api/v1"
        ));
        assert!(!key_may_stay(
            "http://192.168.1.5:8000",
            "http://192.168.1.6:8000"
        ));
        assert!(!key_may_stay(
            "http://192.168.1.5:8000",
            "http://192.168.1.5:8001"
        ));
        assert!(!key_may_stay("https://example.com", "http://example.com"));
        assert!(!key_may_stay("not a url", "not a url"));
        assert!(PASTE_THE_NEW_KEY.contains("Paste the API key for the new address"));
    }

    /// A keychain double: what `get` answers, whether `delete` works, and what was deleted.
    struct Keychain {
        get: Result<Option<String>, store::secrets::SecretError>,
        delete_fails: bool,
        deleted: std::sync::Mutex<Vec<String>>,
    }

    impl Keychain {
        fn new(
            get: Result<Option<String>, store::secrets::SecretError>,
            delete_fails: bool,
        ) -> Self {
            Self {
                get,
                delete_fails,
                deleted: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn deleted(&self) -> Vec<String> {
            self.deleted.lock().unwrap().clone()
        }
    }

    impl store::secrets::SecretStore for Keychain {
        fn get(&self, _: &str) -> Result<Option<String>, store::secrets::SecretError> {
            self.get.clone()
        }
        fn set(&self, _: &str, _: &str) -> Result<(), store::secrets::SecretError> {
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), store::secrets::SecretError> {
            if self.delete_fails {
                return Err(store::secrets::SecretError::NoAccess("locked".into()));
            }
            self.deleted.lock().unwrap().push(key.to_owned());
            Ok(())
        }
    }

    /// [0.0.7 P5] The key rules hold when the keychain fails: a keychain that cannot be read
    /// counts as holding a key, and a key that cannot be removed stops the save.
    #[test]
    fn the_key_rules_hold_when_the_keychain_fails() {
        let locked = || Err(store::secrets::SecretError::NoAccess("locked".into()));
        let id = Some(store::ProviderId(3));
        let openai = Some("https://api.openai.com/v1");
        let code = |r: AppResult<()>| r.unwrap_err().message;
        // Unreadable: a moved endpoint without a new key is refused, as with a saved key.
        let keychain = Keychain::new(locked(), false);
        let refused = prepare_key(
            &keychain,
            id,
            openai,
            "https://openrouter.ai/api/v1",
            false,
            false,
        );
        assert_eq!(code(refused), PASTE_THE_NEW_KEY);
        // Unreadable: no key over plain http to the internet either.
        let refused = prepare_key(
            &keychain,
            id,
            Some("http://api.example.com/v1"),
            "http://api.example.com/v1",
            false,
            false,
        );
        assert_eq!(code(refused), agent::cloud::KEY_OVER_HTTP);
        // Removal asked for but the keychain refuses: nothing is saved.
        let failing = Keychain::new(Ok(Some("sk-old".into())), true);
        let refused = prepare_key(
            &failing,
            id,
            openai,
            "https://api.openai.com/v1",
            false,
            true,
        );
        assert!(code(refused).starts_with("The saved API key could not be removed"));
        // Unreadable and removal asked for: removed before anything else.
        let keychain = Keychain::new(locked(), false);
        prepare_key(
            &keychain,
            id,
            openai,
            "https://openrouter.ai/api/v1",
            false,
            true,
        )
        .unwrap();
        assert_eq!(keychain.deleted(), ["provider-api-key:3"]);
        // A new key for a new address: the old key goes first, so it is never left with the
        // new address; a keychain that cannot remove it stops the save.
        let keychain = Keychain::new(Ok(Some("sk-old".into())), false);
        prepare_key(
            &keychain,
            id,
            openai,
            "https://openrouter.ai/api/v1",
            true,
            false,
        )
        .unwrap();
        assert_eq!(keychain.deleted(), ["provider-api-key:3"]);
        let failing = Keychain::new(Ok(Some("sk-old".into())), true);
        assert!(
            prepare_key(
                &failing,
                id,
                openai,
                "https://openrouter.ai/api/v1",
                true,
                false
            )
            .is_err()
        );
        // No key saved: a move needs nothing; the same address keeps a saved key.
        let empty = Keychain::new(Ok(None), true);
        prepare_key(
            &empty,
            id,
            openai,
            "https://openrouter.ai/api/v1",
            false,
            false,
        )
        .unwrap();
        let saved = Keychain::new(Ok(Some("sk-old".into())), true);
        prepare_key(
            &saved,
            id,
            openai,
            "https://api.openai.com/v1/",
            false,
            false,
        )
        .unwrap();
        assert!(saved.deleted().is_empty());
        // A new endpoint has no saved key.
        prepare_key(&saved, None, None, "https://api.openai.com/v1", true, false).unwrap();
        assert!(may_hold_key(
            &Keychain::new(locked(), false),
            store::ProviderId(3)
        ));
        assert!(!may_hold_key(
            &Keychain::new(Ok(None), false),
            store::ProviderId(3)
        ));
    }

    /// [0.0.7 P5] The chat header's marks: "Cloud" with the host for the internet, and how
    /// Claude's Thinking switch reads.
    #[test]
    fn chat_marks_for_endpoints() {
        use ProviderKind::{Anthropic, Generic, OpenAi};
        let claude = marks_for(Anthropic, "https://api.anthropic.com", "claude-sonnet-5-5");
        assert_eq!(claude.cloud_host.as_deref(), Some("api.anthropic.com"));
        assert!(claude.thinking && !claude.thinking_low);
        // Claude Opus 5.5 cannot stop thinking: "off" asks for less.
        let opus = marks_for(Anthropic, "https://api.anthropic.com", "claude-opus-5-5");
        assert!(opus.thinking && opus.thinking_low);
        assert!(marks_for(Anthropic, "https://api.anthropic.com", "claude-fable-5-2").thinking_low);
        let openai = marks_for(OpenAi, "https://api.openai.com/v1", "gpt-5.5");
        assert_eq!(openai.cloud_host.as_deref(), Some("api.openai.com"));
        assert!(!openai.thinking && !openai.thinking_low);
        assert_eq!(
            marks_for(Generic, "http://127.0.0.1:11434", "m"),
            ChatMarks::default()
        );
        assert_eq!(
            marks_for(Generic, "http://192.168.1.5:8000", "m"),
            ChatMarks::default()
        );
        // A name other than localhost can lead anywhere: marked.
        assert_eq!(
            marks_for(Generic, "http://nas.local:11434", "m")
                .cloud_host
                .as_deref(),
            Some("nas.local")
        );
        // An address no host can be read from is marked with the address itself.
        assert_eq!(
            marks_for(Generic, "http://", "m").cloud_host.as_deref(),
            Some("http://")
        );
        assert_eq!(model_of("remote:3:claude-opus-5-5"), "claude-opus-5-5");
        assert_eq!(model_of("remote:3:org/model:free"), "org/model:free");
        assert_eq!(model_of("local:/m.gguf"), "");
    }

    /// [0.0.7 P5] The ready-made choices map to their protocols; unknown kinds stay generic.
    #[test]
    fn cloud_kinds() {
        assert_eq!(kind_of("anthropic"), ProviderKind::Anthropic);
        assert_eq!(kind_of("openai"), ProviderKind::OpenAi);
        assert_eq!(kind_of("openrouter"), ProviderKind::OpenRouter);
        assert_eq!(kind_of("abliteration"), ProviderKind::Abliteration);
        assert_eq!(kind_of("generic"), ProviderKind::Generic);
        assert_eq!(kind_of("something-new"), ProviderKind::Generic);
    }

    /// [0.0.7 P5] What the server reports, held to the endpoint's chosen size; without a
    /// report, the chosen size, else the company's documented size, else the user's setting.
    #[test]
    fn context_plans() {
        use ProviderKind::{Abliteration, Anthropic, Generic, OpenAi, OpenRouter};
        let opus = "claude-opus-5-5";
        // "Automatic" for Claude: at most 200,000 of the 1,000,000 the model takes, so a long
        // chat does not send close to a million tokens with every message.
        assert_eq!(
            plan_context(Anthropic, opus, Some(1_000_000), None, 4_096),
            200_000
        );
        // Claude Haiku 5.5 costs five times as much per token above 100,000.
        assert_eq!(
            plan_context(Anthropic, "claude-haiku-5-5", Some(1_000_000), None, 4_096),
            100_000
        );
        assert_eq!(
            plan_context(Anthropic, "claude-haiku-4-5", Some(200_000), None, 4_096),
            200_000
        );
        // A larger size is the user's choice, held to what the model takes.
        assert_eq!(
            plan_context(Anthropic, opus, Some(1_000_000), Some(1_000_000), 4_096),
            1_000_000
        );
        assert_eq!(
            plan_context(Anthropic, opus, Some(1_000_000), Some(400_000), 4_096),
            400_000
        );
        assert_eq!(
            plan_context(Anthropic, opus, Some(100_000), Some(200_000), 4_096),
            100_000
        );
        assert_eq!(plan_context(Anthropic, opus, None, None, 16_384), 200_000);
        assert_eq!(plan_context(OpenAi, "gpt-5.5", None, None, 4_096), 128_000);
        assert_eq!(
            plan_context(OpenAi, "gpt-5.5", None, Some(65_536), 4_096),
            65_536
        );
        assert_eq!(
            plan_context(OpenRouter, "x/y", Some(1_048_576), None, 4_096),
            128_000
        );
        assert_eq!(
            plan_context(OpenRouter, "x/y", Some(32_768), None, 4_096),
            32_768
        );
        assert_eq!(
            plan_context(Abliteration, "m", Some(131_072), None, 4_096),
            32_768
        );
        // Servers that are not a company's: what they report, else the user's setting.
        assert_eq!(plan_context(Generic, "m", None, None, 16_384), 16_384);
        assert_eq!(plan_context(Generic, "m", Some(8_192), None, 16_384), 8_192);
        assert_eq!(
            plan_context(Generic, "m", Some(262_144), None, 16_384),
            262_144
        );
    }

    /// [0.1.0] Chat and Studio build an endpoint's client alike: its address, its kind and
    /// the key given, under the key-transport rule.
    #[test]
    fn a_client_is_built_from_the_endpoint_and_its_key() {
        let provider = |kind: &str, base_url: &str| store::Provider {
            id: store::ProviderId(4),
            name: "x".into(),
            kind: kind.into(),
            base_url: base_url.into(),
            secret_ref: None,
            enabled: true,
            created_at: 0,
        };
        let openrouter = provider("openrouter", "https://openrouter.ai/api/v1");
        let client = client_from(&openrouter, Some("sk-or-test".into())).unwrap();
        assert_eq!(client.config().kind, ProviderKind::OpenRouter);
        assert_eq!(client.config().base_url, "https://openrouter.ai/api/v1");
        assert_eq!(client.config().api_key.as_deref(), Some("sk-or-test"));
        let local = client_from(&provider("ollama", "http://127.0.0.1:11434"), None).unwrap();
        assert_eq!(local.config().kind, ProviderKind::Ollama);
        assert_eq!(local.config().api_key, None);
        // A key never goes over plain http to the internet.
        let plain = provider("generic", "http://api.example.com/v1");
        assert!(client_from(&plain, Some("sk-test".into())).is_err());
    }

    /// [0.0.7 P5] The endpoint list says which endpoints are on the internet.
    #[test]
    fn the_list_marks_cloud_endpoints() {
        let provider = |base_url: &str| store::Provider {
            id: store::ProviderId(1),
            name: "x".into(),
            kind: "generic".into(),
            base_url: base_url.into(),
            secret_ref: None,
            enabled: true,
            created_at: 0,
        };
        assert!(dto(&provider("https://api.anthropic.com"), true, None).cloud);
        assert!(!dto(&provider("http://127.0.0.1:11434"), false, None).cloud);
        assert!(!dto(&provider("http://192.168.1.4:8000/v1"), false, None).cloud);
        let row = dto(
            &provider("https://openrouter.ai/api/v1"),
            true,
            Some(65_536),
        );
        let value = serde_json::to_value(&row).unwrap();
        assert_eq!(value["cloud"], serde_json::json!(true));
        assert_eq!(value["contextLength"], serde_json::json!(65_536));
    }
}
