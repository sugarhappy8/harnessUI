//! The one HTTP client configuration for remote MCP servers (ARCHITECTURE.md 9.2 "HTTP",
//! 13.6).
//!
//! - **Redirects:** only same-origin 307/308 redirects are followed (they keep the method
//!   and body), so the configured headers, which often carry a token, are never replayed to
//!   another host. rmcp applies such a policy only to its private default client.
//! - **Timeouts:** a connect timeout, and a read timeout longer than the per-call timeout
//!   so a slow tool is ended by the call timeout, not by the socket.
//! - **Size:** rmcp caps JSON bodies at 16 MiB; SSE events are capped here at
//!   [`MAX_SSE_EVENT_BYTES`].
//! - **Proxy:** the system proxy, as for all non-loopback traffic.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use http::{HeaderName, HeaderValue};

use crate::error::HostError;
use crate::secrets;

/// Connect timeout for remote servers.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Largest server-sent event accepted from a remote server.
pub const MAX_SSE_EVENT_BYTES: usize = 8 * 1024 * 1024;

/// Most redirects followed for one request.
const MAX_REDIRECTS: usize = 5;

/// Builds the client used for every remote server.
pub(crate) fn build_client(
    call_timeout: Duration,
    user_agent: &str,
) -> Result<reqwest::Client, HostError> {
    reqwest::Client::builder()
        .redirect(same_origin_redirects())
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(call_timeout + Duration::from_secs(30))
        .pool_max_idle_per_host(0)
        .user_agent(user_agent)
        .build()
        .map_err(|e| HostError::Http(e.without_url().to_string()))
}

/// Follows only same-origin 307 and 308 redirects, at most [`MAX_REDIRECTS`] hops.
/// Anything else is handed back to rmcp as the redirect response, which fails the request.
fn same_origin_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let keeps_body = matches!(
            attempt.status(),
            reqwest::StatusCode::TEMPORARY_REDIRECT | reqwest::StatusCode::PERMANENT_REDIRECT
        );
        let same_origin = attempt
            .previous()
            .first()
            .is_some_and(|first| first.origin() == attempt.url().origin());
        if !keeps_body || !same_origin {
            attempt.stop()
        } else if attempt.previous().len() > MAX_REDIRECTS {
            attempt.error(format!("more than {MAX_REDIRECTS} redirects"))
        } else {
            attempt.follow()
        }
    })
}

/// The configured URL with secret placeholders resolved.
pub(crate) fn resolve_url(
    url: &str,
    resolver: &dyn Fn(&str) -> Option<String>,
) -> Result<String, HostError> {
    let resolved = secrets::resolve(url, resolver)?;
    crate::config::validate_url(&resolved).map_err(HostError::InvalidConfig)?;
    Ok(resolved.trim().to_owned())
}

/// The configured headers with secret placeholders resolved. Values are marked sensitive
/// so they never appear in debug output.
pub(crate) fn resolve_headers(
    headers: &BTreeMap<String, String>,
    resolver: &dyn Fn(&str) -> Option<String>,
) -> Result<HashMap<HeaderName, HeaderValue>, HostError> {
    let mut out = HashMap::new();
    for (name, value) in headers {
        crate::config::validate_header(name, value).map_err(HostError::InvalidConfig)?;
        let header = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            HostError::InvalidConfig(format!("\"{name}\" is not a valid header name"))
        })?;
        let resolved = secrets::resolve(value, resolver)?;
        let mut header_value = HeaderValue::from_str(&resolved).map_err(|_| {
            HostError::InvalidConfig(format!(
                "the value of header \"{name}\" contains characters a header cannot carry"
            ))
        })?;
        header_value.set_sensitive(true);
        out.insert(header, header_value);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keychain(name: &str) -> Option<String> {
        match name {
            "tok" => Some("abc123".into()),
            "nl" => Some("a\nb".into()),
            _ => None,
        }
    }

    #[test]
    fn resolves_header_placeholders_and_marks_them_sensitive() {
        let headers = BTreeMap::from([
            (
                "Authorization".to_owned(),
                "Bearer ${secret:tok}".to_owned(),
            ),
            ("X-Plain".to_owned(), "v".to_owned()),
        ]);
        let resolved = resolve_headers(&headers, &keychain).unwrap();
        let auth = &resolved[&HeaderName::from_static("authorization")];
        assert_eq!(auth.to_str().unwrap(), "Bearer abc123");
        assert!(auth.is_sensitive());
        assert!(!format!("{auth:?}").contains("abc123"));
        // A missing secret, or one that cannot go in a header, is an error.
        let missing = BTreeMap::from([("X".to_owned(), "${secret:nope}".to_owned())]);
        assert!(matches!(
            resolve_headers(&missing, &keychain),
            Err(HostError::Secret(_))
        ));
        let bad = BTreeMap::from([("X".to_owned(), "${secret:nl}".to_owned())]);
        assert!(matches!(
            resolve_headers(&bad, &keychain),
            Err(HostError::InvalidConfig(_))
        ));
        let reserved = BTreeMap::from([("Mcp-Session-Id".to_owned(), "x".to_owned())]);
        assert!(matches!(
            resolve_headers(&reserved, &keychain),
            Err(HostError::InvalidConfig(_))
        ));
    }

    #[test]
    fn resolves_url_placeholders() {
        assert_eq!(
            resolve_url("https://h.example/mcp?key=${secret:tok}", &keychain).unwrap(),
            "https://h.example/mcp?key=abc123"
        );
        assert!(matches!(
            resolve_url("https://h.example/${secret:nope}", &keychain),
            Err(HostError::Secret(_))
        ));
    }

    /// A one-shot HTTP/1.1 server on a loopback port that answers every request with
    /// `status` and `location`, or 200 when `location` is empty. Returns its base URL.
    async fn tiny_server(status: u16, location: String) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let response = if location.is_empty() {
                    "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok".to_owned()
                } else {
                    format!(
                        "HTTP/1.1 {status} Redirect\r\nlocation: {location}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    )
                };
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn only_same_origin_body_preserving_redirects_are_followed() {
        let client = reqwest::Client::builder()
            .redirect(same_origin_redirects())
            .no_proxy()
            .build()
            .unwrap();
        // Another origin (a different port): not followed, the 307 comes back.
        let target = tiny_server(200, String::new()).await;
        let cross = tiny_server(307, format!("{target}/x")).await;
        let response = client.post(format!("{cross}/mcp")).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 307);
        // A 302 is never followed, even to the same origin.
        let found = tiny_server(302, "/elsewhere".into()).await;
        let response = client.post(format!("{found}/mcp")).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 302);
        // Same origin and a 307: followed. The server redirects every request, so the
        // hop limit ends it with an error rather than a loop.
        let same = tiny_server(307, "/again".into()).await;
        let error = client.post(format!("{same}/mcp")).send().await.unwrap_err();
        assert!(error.is_redirect(), "{error}");
    }

    #[test]
    fn the_client_builds() {
        build_client(Duration::from_secs(300), "test-agent/1").unwrap();
    }
}
