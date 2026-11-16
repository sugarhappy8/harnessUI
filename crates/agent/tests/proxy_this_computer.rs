//! Proxy rules for the addresses of this computer and the local network (0.0.7 P5,
//! ARCHITECTURE.md 13.6): every address the key check and the "Cloud" mark count as this
//! computer (`0.0.0.0`, `0`, `[::]`, `localhost.` and the like) or as the local network is
//! reached directly, so a system proxy never sees its plain-http traffic and key. One test in
//! its own binary, because it sets the proxy environment before any other thread exists.

mod common;

use std::time::Duration;

use agent::{ClientConfig, EndpointPlace, OpenAiClient, ProviderKind};
use common::{MockServer, Reply};
use serde_json::json;

const KEY: &str = "sk-local-SECRET-0123456789";

#[test]
fn no_system_proxy_sees_a_local_endpoints_key() {
    // Bind the proxy's port and set the environment before any other thread exists.
    let proxy_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_url = format!("http://{}", proxy_listener.local_addr().unwrap());
    // SAFETY: this is the only test in this binary and no other thread has started yet, so
    // nothing reads the environment concurrently.
    unsafe {
        for key in ["ALL_PROXY", "all_proxy", "REQUEST_METHOD"] {
            std::env::remove_var(key);
        }
        for key in ["NO_PROXY", "no_proxy"] {
            std::env::set_var(key, "no-such-host.invalid");
        }
        for key in ["HTTP_PROXY", "http_proxy"] {
            std::env::set_var(key, &proxy_url);
        }
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let proxy = MockServer::start_on(proxy_listener, |_| {
            Reply::json(502, &json!({"error": "the proxy saw this request"}))
        });
        let target = MockServer::start(|_| Reply::json(200, &json!({"data": [{"id": "m"}]}))).await;
        let port = target.addr.port();
        let local = [
            format!("http://127.0.0.1:{port}"),
            format!("http://127.0.0.2:{port}"),
            format!("http://0.0.0.0:{port}"),
            format!("http://0:{port}"),
            format!("http://[::]:{port}"),
            format!("http://[::ffff:0.0.0.0]:{port}"),
            format!("http://localhost.:{port}"),
            format!("http://LOCALHOST:{port}"),
            // The local network: not this computer, but never through the proxy either.
            "http://192.168.255.254:9".to_owned(),
            "http://[fd00::1]:9".to_owned(),
        ];
        for raw in &local {
            assert_ne!(agent::endpoint_place(raw), EndpointPlace::Internet, "{raw}");
            let mut config = ClientConfig::new(raw.as_str(), ProviderKind::Vllm);
            config.api_key = Some(KEY.into());
            config.connect_timeout = Duration::from_millis(500);
            config.request_timeout = Some(Duration::from_secs(2));
            if agent::endpoint_place(raw) == EndpointPlace::ThisComputer {
                assert!(config.loopback, "{raw} is this computer");
            }
            // Reached or not (0.0.0.0 and [::] are not reachable everywhere), never through
            // the proxy.
            let _ = OpenAiClient::new(config).unwrap().list_models().await;
            assert!(
                proxy.requests().is_empty(),
                "{raw} went through the proxy: {:?}",
                proxy.requests()
            );
        }
        // The proxy is really in effect: a server on the internet (no key) goes through it.
        let mut config =
            ClientConfig::new(format!("http://far-away.invalid:{port}"), ProviderKind::Vllm);
        config.connect_timeout = Duration::from_millis(500);
        assert!(!config.loopback);
        let _ = OpenAiClient::new(config).unwrap().list_models().await;
        let seen = proxy.requests();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(seen[0].target.starts_with("http://far-away.invalid:"));
        assert!(seen[0].header("authorization").is_none());
    });
}
