//! Which URLs the app's own webviews may navigate to.
//!
//! The webview never loads remote content. The only allowed origins are the ones
//! Tauri serves the bundled frontend from, plus the Vite dev server in debug builds.

use url::Url;

/// Returns true when a webview may navigate to `url`.
///
/// * macOS and Linux serve the app from `tauri://localhost`.
/// * Windows serves it from `http://tauri.localhost` (or `https://` when configured).
/// * Debug builds also load from the Vite dev server on `http://localhost:1420`.
pub fn is_allowed(url: &Url, dev: bool) -> bool {
    match (url.scheme(), url.host_str()) {
        ("tauri", Some("localhost")) => true,
        ("http" | "https", Some("tauri.localhost")) => url.port().is_none(),
        ("http", Some("localhost")) if dev => url.port() == Some(1420),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(s: &str, dev: bool) -> bool {
        is_allowed(&Url::parse(s).unwrap(), dev)
    }

    #[test]
    fn app_origins_are_allowed() {
        assert!(allowed("tauri://localhost/index.html", false));
        assert!(allowed("http://tauri.localhost/index.html", false));
        assert!(allowed("https://tauri.localhost/overlay.html", false));
    }

    #[test]
    fn dev_server_only_in_dev() {
        assert!(allowed("http://localhost:1420/", true));
        assert!(!allowed("http://localhost:1420/", false));
        assert!(!allowed("http://localhost:8080/", true));
    }

    #[test]
    fn remote_and_lookalike_origins_are_blocked() {
        for url in [
            "https://example.com/",
            "http://tauri.localhost.evil.com/",
            "http://evil.com/?tauri.localhost",
            "http://tauri.localhost:9931/",
            "file:///etc/passwd",
            "tauri://evil/",
            "javascript:alert(1)",
            "data:text/html,hi",
            "http://127.0.0.1:9931/",
        ] {
            assert!(!allowed(url, true), "{url} must be blocked");
        }
    }
}
