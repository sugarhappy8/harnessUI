//! Endpoints the user adds: OpenAI-compatible servers (Ollama, LM Studio, vLLM, others) and
//! (0.0.7) the ready-made cloud companies: Anthropic (Claude's own Messages API), OpenAI,
//! OpenRouter and abliteration.ai. API keys live in the OS keychain; the database keeps
//! only a reference.

use agent::{ClientConfig, EndpointPlace, OpenAiClient, ProviderKind};
use tauri::State;

use crate::dto::{ModelOptionDto, ProviderDto, SaveProvider};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

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
    let stored: Option<serde_json::Value> =
        state.store.get_setting(&context_key(id)).await.ok().flatten();
    stored
        .as_ref()
        .and_then(crate::settings::parse_remote_context)
}

/// [0.0.7 P5] How a model's context is decided, from what is known: what the server
/// reports, held to the size chosen for the endpoint (a lower size spends less on a cloud
/// company's long chats); when the server does not say, the chosen size, else the size the
/// company documents (ready-made cloud choices), else `setting` (the user's size for
/// servers that do not say, 4,096 by default).
pub fn plan_context(
    kind: ProviderKind,
    reported: Option<u32>,
    chosen: Option<u32>,
    setting: u32,
) -> u32 {
    match (reported, chosen) {
        (Some(reported), Some(chosen)) => reported.min(chosen),
        (Some(reported), None) => reported,
        (None, Some(chosen)) => chosen,
        (None, None) => agent::default_context(kind).unwrap_or(setting),
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

/// [0.0.7 P5] The context a turn plans with for a model on `provider` that reported
/// `reported` ([`plan_context`]).
pub async fn context_for(state: &AppState, provider: &store::Provider, reported: Option<u32>) -> u32 {
    plan_context(
        kind_of(&provider.kind),
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
    let chosen = chosen_context(state, provider.id).await;
    let kind = kind_of(&provider.kind);
    (
        reported.map(|r| chosen.map_or(r, |c| r.min(c))),
        plan_context(kind, None, chosen, setting),
    )
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

/// [0.0.7 P5] What the chat header shows for a chat's model: the host its messages go to when
/// the endpoint is on the internet (the "Cloud" mark), and whether the app can switch the
/// model's thinking (Claude).
pub async fn chat_marks(state: &AppState, model_ref: Option<&str>) -> (Option<String>, bool) {
    let Some(provider) = (match model_ref {
        Some(model_ref) => provider_of(state, model_ref).await,
        None => None,
    }) else {
        return (None, false);
    };
    let cloud = (agent::endpoint_place(&provider.base_url) == EndpointPlace::Internet)
        .then(|| agent::endpoint_host(&provider.base_url))
        .flatten();
    (cloud, kind_of(&provider.kind) == ProviderKind::Anthropic)
}

/// Validates a base URL: http(s) only, no credentials embedded in it.
fn check_base_url(url: &str) -> AppResult<()> {
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
    let mut config = ClientConfig::new(provider.base_url.clone(), kind_of(&provider.kind));
    config.api_key = api_key;
    let client = OpenAiClient::new(config).map_err(AppError::from)?;
    Ok((client, provider))
}

#[tauri::command]
pub async fn list_providers(state: State<'_, AppState>) -> AppResult<Vec<ProviderDto>> {
    let mut out = Vec::new();
    for p in state.store.list_providers().await? {
        let has_key = state
            .secrets
            .get(&secret_key(p.id))
            .ok()
            .flatten()
            .is_some();
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
    // [0.0.7 P5] A key never travels over plain http:// to a server on the internet: not a
    // new key, and not one already saved for this endpoint.
    let new_key = provider
        .api_key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty());
    let kept_key = !provider.clear_api_key
        && provider
            .id
            .is_some_and(|id| state.secrets.get(&secret_key(id)).ok().flatten().is_some());
    if new_key || kept_key {
        agent::check_key_transport(&base_url).map_err(AppError::bad_request)?;
    }
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
    if provider.clear_api_key {
        let _ = state.secrets.delete(&key);
    }
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
    let has_key = state.secrets.get(&key).ok().flatten().is_some();
    Ok(dto(&saved, has_key, provider.context_length))
}

#[tauri::command]
pub async fn delete_provider(state: State<'_, AppState>, id: store::ProviderId) -> AppResult<()> {
    let _ = state.secrets.delete(&secret_key(id));
    let _ = state.store.delete_setting(&context_key(id)).await;
    state.store.delete_provider(id).await?;
    let _ = state
        .store
        .set_setting::<Vec<ModelOptionDto>>(&crate::settings::provider_models_key(id), &Vec::new())
        .await;
    Ok(())
}

/// Connects to the endpoint, lists its models and caches them for the model picker
/// (so the picker works offline).
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
        use ProviderKind::{Anthropic, Generic, OpenAi};
        assert_eq!(plan_context(Anthropic, Some(1_000_000), None, 4_096), 1_000_000);
        assert_eq!(plan_context(Anthropic, Some(1_000_000), Some(200_000), 4_096), 200_000);
        assert_eq!(plan_context(Anthropic, Some(100_000), Some(200_000), 4_096), 100_000);
        assert_eq!(plan_context(OpenAi, None, None, 4_096), 128_000);
        assert_eq!(plan_context(OpenAi, None, Some(65_536), 4_096), 65_536);
        assert_eq!(plan_context(Anthropic, None, None, 16_384), 200_000);
        assert_eq!(plan_context(Generic, None, None, 16_384), 16_384);
        assert_eq!(plan_context(Generic, Some(8_192), None, 16_384), 8_192);
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
        let row = dto(&provider("https://openrouter.ai/api/v1"), true, Some(65_536));
        let value = serde_json::to_value(&row).unwrap();
        assert_eq!(value["cloud"], serde_json::json!(true));
        assert_eq!(value["contextLength"], serde_json::json!(65_536));
    }
}
