//! OpenAI-compatible endpoints the user adds (Ollama, LM Studio, vLLM, others).
//! API keys live in the OS keychain; the database keeps only a reference.

use agent::{ClientConfig, OpenAiClient, ProviderKind};
use tauri::State;

use crate::dto::{ModelOptionDto, ProviderDto, SaveProvider};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

fn kind_of(s: &str) -> ProviderKind {
    match s {
        "ollama" => ProviderKind::Ollama,
        "lmstudio" => ProviderKind::LmStudio,
        "vllm" => ProviderKind::Vllm,
        _ => ProviderKind::Generic,
    }
}

fn secret_key(provider_id: store::ProviderId) -> String {
    format!("provider-api-key:{}", provider_id.0)
}

fn dto(p: &store::Provider, has_key: bool) -> ProviderDto {
    ProviderDto {
        id: p.id,
        name: p.name.clone(),
        kind: p.kind.clone(),
        base_url: p.base_url.clone(),
        has_api_key: has_key,
        enabled: p.enabled,
    }
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
        out.push(dto(&p, has_key));
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
    let saved = state
        .store
        .get_provider(id)
        .await?
        .ok_or_else(|| AppError::not_found("endpoint"))?;
    let has_key = state.secrets.get(&key).ok().flatten().is_some();
    Ok(dto(&saved, has_key))
}

#[tauri::command]
pub async fn delete_provider(state: State<'_, AppState>, id: store::ProviderId) -> AppResult<()> {
    let _ = state.secrets.delete(&secret_key(id));
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
    let models = client.list_models().await?;
    let options: Vec<ModelOptionDto> = models
        .into_iter()
        .map(|m| ModelOptionDto {
            reference: format!("remote:{}:{}", provider.id.0, m.id),
            label: format!("{} · {}", m.id, provider.name),
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
}
