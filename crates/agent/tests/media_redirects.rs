//! Studio (0.1.0): a video's result is downloaded with the key from the endpoint's own origin
//! only. When OpenRouter's content endpoint sends the download on to another origin (a file
//! host), the link is handed back for the guarded fetcher, which goes without the key; the
//! client never follows it. This file holds a single test because it changes process
//! environment variables (no proxy, so the requests reach the local fakes), which must happen
//! before any other thread exists, as in `tests/redirects.rs`.

mod common;

use agent::media::{Fetched, JobDone, MediaProvider, OpenRouterMedia};
use agent::{ClientConfig, OpenAiClient, ProviderKind};
use common::{MockServer, Reply};
use url::Url;

const KEY: &str = "sk-or-v1-media-redirect-test-key";

#[test]
fn a_redirect_to_another_origin_carries_no_key() {
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
        let elsewhere = MockServer::start(|_| Reply::Raw {
            status: 200,
            content_type: "video/mp4".into(),
            body: b"\0\0\0\x18ftypmp42 video".to_vec(),
        })
        .await;
        let link = format!("{}/v.mp4", elsewhere.url());
        let location = link.clone();
        // The content endpoint sends the download on to another port of this computer:
        // another origin.
        let bouncing = MockServer::start(move |_| Reply::Redirect {
            status: 307,
            location: location.clone(),
        })
        .await;

        let mut config = ClientConfig::new(
            format!("{}/api/v1", bouncing.url()),
            ProviderKind::OpenRouter,
        );
        config.api_key = Some(KEY.to_owned());
        // As for a server on the internet, whose client follows redirects within its own
        // origin; the address is still this computer's, so the key may go over http://.
        config.loopback = false;
        let media = OpenRouterMedia::new(OpenAiClient::new(config).unwrap());

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("v.mp4");
        let job = JobDone {
            outputs: 1,
            signed: Vec::new(),
            cost: None,
        };
        let fetched = media.download("vid_1", &job, 0, &dest).await.unwrap();
        assert_eq!(fetched, Fetched::Elsewhere(Url::parse(&link).unwrap()));
        assert!(
            elsewhere.requests().is_empty(),
            "a redirect to another origin was followed: {:?}",
            elsewhere.requests()
        );
        let asked = bouncing.requests();
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert_eq!(asked[0].target, "/api/v1/videos/vid_1/content?index=0");
        assert_eq!(
            asked[0].header("authorization"),
            Some(format!("Bearer {KEY}").as_str())
        );
        assert!(!dest.exists());
    });
}
