//! Redirects (ARCHITECTURE.md 7.5 and 13.6, 0.0.7 P5): a client for a server that is not on
//! this machine follows a redirect only within the endpoint's own origin, so the chat and the
//! API key never reach a host the user did not add. This file holds a single test because it
//! changes process environment variables (no proxy, so the requests reach the local fakes),
//! which must happen before any other thread exists.

mod common;

use agent::{ChatMessage, ChatRequest, ClientConfig, OpenAiClient, ProviderError, ProviderKind};
use common::{MockServer, Reply};
use serde_json::json;
use tokio_util::sync::CancellationToken;

const KEY: &str = "sk-ant-api03-redirect-test-key";

fn remote(url: String, kind: ProviderKind) -> OpenAiClient {
    let mut config = ClientConfig::new(url, kind);
    config.api_key = Some(KEY.to_owned());
    // As for a server on the internet; the address is still this computer's, so the key may
    // go over http://.
    config.loopback = false;
    OpenAiClient::new(config).unwrap()
}

#[test]
fn redirects_stay_with_the_endpoint() {
    // SAFETY: this is the only test in this binary and no other thread has started yet, so
    // nothing reads the environment concurrently.
    unsafe {
        for key in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
            "REQUEST_METHOD",
        ] {
            std::env::remove_var(key);
        }
        for key in ["NO_PROXY", "no_proxy"] {
            std::env::set_var(key, "*");
        }
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let elsewhere =
            MockServer::start(|_| Reply::json(200, &json!({"data": [{"id": "elsewhere"}]}))).await;
        let other = elsewhere.url();
        // Every request goes on to another port of this computer: another origin.
        let bouncing = MockServer::start(move |request| Reply::Redirect {
            status: 307,
            location: format!("{other}{}", request.target),
        })
        .await;
        for kind in [ProviderKind::Anthropic, ProviderKind::OpenAi, ProviderKind::Generic] {
            let err = remote(bouncing.url(), kind).list_models().await.unwrap_err();
            assert!(
                matches!(err, ProviderError::Http { status: 307, .. }),
                "{kind:?}: {err:?}"
            );
            assert!(
                err.user_message()
                    .starts_with("The server sent the request on to another address"),
                "{err:?}"
            );
            assert!(!err.to_string().contains(KEY), "{err}");
        }
        assert_eq!(bouncing.requests().len(), 3);
        // The streamed reply (POST) and Claude's model lookup (GET /v1/models/{id}) are not
        // sent elsewhere either.
        for kind in [ProviderKind::Anthropic, ProviderKind::Generic] {
            let mut request =
                ChatRequest::new("claude-opus-5-5", vec![ChatMessage::user("Hi")]);
            request.max_tokens = Some(100);
            let err = remote(bouncing.url(), kind)
                .stream_chat(&request, CancellationToken::new())
                .await
                .unwrap_err();
            assert!(
                matches!(err, ProviderError::Http { status: 307, .. }),
                "{kind:?}: {err:?}"
            );
        }
        let paths: Vec<String> = bouncing.requests()[3..]
            .iter()
            .map(|r| format!("{} {}", r.method, r.path()))
            .collect();
        assert_eq!(
            paths,
            [
                "GET /v1/models/claude-opus-5-5",
                "POST /v1/messages",
                "POST /v1/chat/completions"
            ]
        );
        assert!(
            elsewhere.requests().is_empty(),
            "a redirect to another origin was followed: {:?}",
            elsewhere.requests()
        );

        // Within the endpoint's own origin a redirect is followed, with the key.
        let moving = MockServer::start(|request| {
            if request.path().starts_with("/moved") {
                Reply::json(
                    200,
                    &json!({"data": [{"id": "claude-moved", "type": "model", "display_name": "Moved"}], "has_more": false}),
                )
            } else {
                Reply::Redirect {
                    status: 307,
                    location: format!("/moved{}", request.target),
                }
            }
        })
        .await;
        let models = remote(moving.url(), ProviderKind::Anthropic)
            .list_models()
            .await
            .unwrap();
        assert_eq!(models[0].id, "claude-moved");
        let requests = moving.requests();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert!(requests[1].path().starts_with("/moved/v1/models"));
        assert_eq!(requests[1].header("x-api-key"), Some(KEY));
    });
}
