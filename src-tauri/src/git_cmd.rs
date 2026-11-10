//! Settings → Git (ARCHITECTURE.md 9.3 "Git"): which git the tools use, access tokens
//! per host for the built-in Git library (kept in the keychain), and the name and email
//! for commits.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Setting: hosts that have a token in the keychain (the keychain cannot list entries).
const TOKEN_HOSTS: &str = "gitTokenHosts";
/// Setting: the commit identity.
pub const IDENTITY: &str = "gitIdentity";

fn token_key(host: &str) -> String {
    format!("git-token:{host}")
}

/// The name and email written into commits (users without git usually have no
/// ~/.gitconfig).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitIdentity {
    pub name: String,
    pub email: String,
}

impl GitIdentity {
    /// The environment variables git and libgit2 read for commits.
    pub fn env(&self) -> Vec<(String, String)> {
        if self.name.trim().is_empty() || self.email.trim().is_empty() {
            return Vec::new();
        }
        let (n, e) = (self.name.trim().to_string(), self.email.trim().to_string());
        vec![
            ("GIT_AUTHOR_NAME".into(), n.clone()),
            ("GIT_AUTHOR_EMAIL".into(), e.clone()),
            ("GIT_COMMITTER_NAME".into(), n),
            ("GIT_COMMITTER_EMAIL".into(), e),
        ]
    }
}

/// The git server: system git or the built-in library with its worker and tokens.
pub fn server(
    mut config: tools::GitConfig,
    secrets: Arc<dyn store::secrets::SecretStore>,
    identity: &GitIdentity,
) -> tools::GitServer {
    config.env.extend(identity.env());
    let mut git = tools::GitServer::new(config);
    if let Ok(exe) = std::env::current_exe() {
        git = git.with_worker(exe);
    }
    git.with_tokens(Arc::new(move |host: &str| {
        secrets.get(&token_key(host)).ok().flatten()
    }))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    /// "system" or "libgit2".
    pub backend: String,
    /// Shown while the built-in library is in use.
    pub notice: Option<String>,
    pub token_hosts: Vec<String>,
    pub identity: GitIdentity,
}

/// Normalises a host as the git tools ask for it: lowercase, punycode, port only when
/// not the default.
fn normalise_host(input: &str) -> Option<String> {
    let text = input.trim();
    let with_scheme = if text.contains("://") {
        text.to_string()
    } else {
        format!("https://{text}")
    };
    let url = url::Url::parse(&with_scheme).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    Some(match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    })
}

#[tauri::command]
pub async fn git_info(state: State<'_, AppState>) -> AppResult<GitInfo> {
    let backend = crate::toolset::git_backend();
    Ok(GitInfo {
        notice: (backend == tools::GitBackend::Libgit2)
            .then(|| tools::git::FALLBACK_NOTICE.to_string()),
        backend: match backend {
            tools::GitBackend::System => "system".into(),
            tools::GitBackend::Libgit2 => "libgit2".into(),
        },
        token_hosts: state
            .store
            .get_setting(TOKEN_HOSTS)
            .await?
            .unwrap_or_default(),
        identity: state.store.get_setting(IDENTITY).await?.unwrap_or_default(),
    })
}

/// Saves an access token for a host in the keychain (the value never comes back).
#[tauri::command]
pub async fn set_git_token(
    state: State<'_, AppState>,
    host: String,
    token: String,
) -> AppResult<Vec<String>> {
    let host = normalise_host(&host)
        .ok_or_else(|| AppError::bad_request("That is not a host name, e.g. github.com."))?;
    let token = token.trim();
    if token.is_empty() || token.len() > 4096 || token.chars().any(char::is_control) {
        return Err(AppError::bad_request("The token is empty or not valid."));
    }
    state.secrets.set(&token_key(&host), token).map_err(|e| {
        AppError::new(
            "keychain",
            "The token could not be saved in the system keychain.",
            "Check that the keychain is unlocked and try again.",
            e.to_string(),
        )
    })?;
    let mut hosts: Vec<String> = state
        .store
        .get_setting(TOKEN_HOSTS)
        .await?
        .unwrap_or_default();
    if !hosts.contains(&host) {
        hosts.push(host);
        hosts.sort();
    }
    state.store.set_setting(TOKEN_HOSTS, &hosts).await?;
    Ok(hosts)
}

#[tauri::command]
pub async fn remove_git_token(state: State<'_, AppState>, host: String) -> AppResult<Vec<String>> {
    let _ = state.secrets.delete(&token_key(&host));
    let mut hosts: Vec<String> = state
        .store
        .get_setting(TOKEN_HOSTS)
        .await?
        .unwrap_or_default();
    hosts.retain(|h| h != &host);
    state.store.set_setting(TOKEN_HOSTS, &hosts).await?;
    Ok(hosts)
}

/// Saves the commit identity and rebuilds the git server with it.
#[tauri::command]
pub async fn set_git_identity(state: State<'_, AppState>, identity: GitIdentity) -> AppResult<()> {
    let clean = GitIdentity {
        name: identity
            .name
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect(),
        email: identity
            .email
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect(),
    };
    if !clean.email.is_empty() && !clean.email.contains('@') {
        return Err(AppError::bad_request("That email address is not valid."));
    }
    state.store.set_setting(IDENTITY, &clean).await?;
    crate::toolset::rebuild_git(&state, &clean).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_normalised_like_the_tools_ask() {
        assert_eq!(normalise_host("GitHub.com").as_deref(), Some("github.com"));
        assert_eq!(
            normalise_host("https://git.corp:8443/x").as_deref(),
            Some("git.corp:8443")
        );
        assert_eq!(
            normalise_host("https://gitlab.com:443").as_deref(),
            Some("gitlab.com")
        );
        assert_eq!(
            normalise_host("bücher.example").as_deref(),
            Some("xn--bcher-kva.example")
        );
        assert_eq!(normalise_host(""), None);
    }

    #[test]
    fn identity_needs_both_fields() {
        assert!(
            GitIdentity {
                name: "A".into(),
                email: String::new()
            }
            .env()
            .is_empty()
        );
        let env = GitIdentity {
            name: "A".into(),
            email: "a@b.c".into(),
        }
        .env();
        assert_eq!(env.len(), 4);
    }
}
