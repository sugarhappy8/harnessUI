//! Proxy rules (ARCHITECTURE.md 13.6): a loopback client ignores the system proxy, any other
//! client uses it. This file holds a single test because it changes process environment
//! variables, which must happen before any other thread exists.

mod common;

use agent::{ClientConfig, OpenAiClient, ProviderKind};
use common::{MockServer, Reply};
use serde_json::json;

#[test]
fn loopback_clients_bypass_the_system_proxy() {
    // Bind the proxy's port and set the environment before any other thread exists.
    let proxy_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_url = format!("http://{}", proxy_listener.local_addr().unwrap());
    // SAFETY: this is the only test in this binary and no other thread has started yet, so
    // nothing reads the environment concurrently.
    // NO_PROXY is set to a name that matches nothing, rather than removed, so that system
    // proxy exceptions on macOS and Windows (which fill in only an empty value) stay out.
    unsafe {
        for key in ["ALL_PROXY", "all_proxy", "REQUEST_METHOD"] {
            std::env::remove_var(key);
        }
        for key in ["NO_PROXY", "no_proxy"] {
            std::env::set_var(key, "no-such-host.invalid");
        }
        std::env::set_var("HTTP_PROXY", &proxy_url);
        std::env::set_var("http_proxy", &proxy_url);
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let proxy = MockServer::start_on(proxy_listener, |_| {
            Reply::json(200, &json!({"data": [{"id": "via-proxy"}]}))
        });
        let target =
            MockServer::start(|_| Reply::json(200, &json!({"data": [{"id": "direct"}]}))).await;

        let loopback =
            OpenAiClient::new(ClientConfig::new(target.url(), ProviderKind::Generic)).unwrap();
        assert!(loopback.config().loopback);
        let models = loopback.list_models().await.unwrap();
        assert_eq!(models[0].id, "direct");
        assert!(
            proxy.requests().is_empty(),
            "the loopback client used the proxy"
        );

        let mut config = ClientConfig::new(target.url(), ProviderKind::Generic);
        config.loopback = false;
        let remote = OpenAiClient::new(config).unwrap();
        let models = remote.list_models().await.unwrap();
        assert_eq!(
            models[0].id, "via-proxy",
            "the non-loopback client ignored the proxy"
        );
        assert_eq!(
            proxy.requests()[0].target,
            format!("{}/v1/models", target.url()),
            "a proxied request uses the absolute URL"
        );
    });
}
