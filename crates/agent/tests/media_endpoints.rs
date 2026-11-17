//! Studio (0.1.0): OpenRouter's picture endpoints against a local fake server that answers in
//! the documented formats. The request that creates a run is sent at most once: never again
//! after a timeout, a server error or a cut connection; once more only on a short rate limit
//! or a connection that never opened. No API key or network needed.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use agent::media::openrouter::place_extra;
use agent::media::{
    Company, Cost, FailureKind, LocalIo, MediaError, MediaFailure, MediaKind, MediaLimits,
    MediaProvider, MediaRequest, OpenRouterMedia, RefImage, RefRole, Routing, Started,
    media_provider, openrouter_image_models,
};
use agent::{ClientConfig, OpenAiClient, ProviderKind};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use common::{MockServer, Reply, StreamEnd};
use serde_json::{Map, Value, json};

const KEY: &str = "sk-or-v1-studio-test-key-0123456789";
const FLARE: &str = "openai/gpt-image-2.6-flare";
const IMAGES: &str = include_str!("fixtures/media/openrouter-images-2026-11-17.json");
const VIDEOS: &str = include_str!("fixtures/media/openrouter-videos-2026-11-17.json");
const ENDPOINTS: &str = include_str!("fixtures/media/openrouter-image-endpoints-2026-11-17.json");
const EXAMPLES: &str = include_str!("fixtures/media/openrouter-request-examples-2026-11-17.json");

/// Two small files that start like PNG pictures (the signature and an IHDR chunk header).
const PNG_A_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR picture a";
const PNG_B_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR picture b";

// ---- helpers ---------------------------------------------------------------------------------

fn openrouter(server: &MockServer) -> OpenRouterMedia {
    openrouter_at(&format!("{}/api/v1", server.url()))
}

fn openrouter_at(base_url: &str) -> OpenRouterMedia {
    let mut config = ClientConfig::new(base_url, ProviderKind::OpenRouter);
    config.api_key = Some(KEY.into());
    OpenRouterMedia::new(OpenAiClient::new(config).expect("client")).with_limits(quick())
}

fn quick() -> MediaLimits {
    MediaLimits {
        picture_timeout: Duration::from_secs(1),
        response_cap: 64 * 1024,
        ..Default::default()
    }
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

fn req() -> MediaRequest {
    MediaRequest {
        kind: MediaKind::Image,
        model: FLARE.into(),
        prompt: "a lighthouse at dusk".into(),
        params: object(json!({"n": 2})),
        extra: Map::new(),
        refs: Vec::new(),
        routing: None,
        provider_options: BTreeMap::new(),
    }
}

/// [`req`] with one added picture, written into `pictures`.
fn req_with_ref(pictures: &Path) -> MediaRequest {
    let path = pictures.join("ref.png");
    std::fs::write(&path, PNG_A_BYTES).expect("write the added picture");
    MediaRequest {
        refs: vec![RefImage {
            role: RefRole::Reference,
            mime: "image/png".into(),
            path,
            sha256: "3f2a9c41d0b7e6a5f4c3b2a1908f7e6d5c4b3a291807f6e5d4c3b2a190817263".into(),
            size_bytes: PNG_A_BYTES.len() as u64,
        }],
        ..req()
    }
}

/// Flare with its recorded `/endpoints` answer: one company, `openai`, which takes the extra
/// settings `moderation` and `complexity`.
fn flare() -> agent::media::MediaModel {
    let images: Value = serde_json::from_str(IMAGES).expect("images fixture");
    let endpoints: Value = serde_json::from_str(ENDPOINTS).expect("endpoints fixture");
    let answers: BTreeMap<String, Value> = endpoints["answers"]
        .as_object()
        .expect("answers")
        .iter()
        .map(|(id, answer)| (id.clone(), answer.clone()))
        .collect();
    openrouter_image_models(&images, &answers)
        .into_iter()
        .find(|model| model.id == FLARE)
        .expect("flare is in the fixture")
}

/// [`req_with_ref`] pinned to OpenAI, with an extra setting placed as Studio places it.
fn req_with_ref_routing_and_extra(pictures: &Path) -> MediaRequest {
    let routing = Routing {
        order: vec!["openai".into()],
        only: true,
        allow_fallbacks: false,
        zdr: false,
    };
    let (extra, provider_options) = place_extra(
        &flare(),
        object(json!({"complexity": "high"})),
        Some(&routing),
    );
    MediaRequest {
        extra,
        provider_options,
        routing: Some(routing),
        ..req_with_ref(pictures)
    }
}

fn pictures_answer() -> Reply {
    Reply::json(
        200,
        &json!({
            "created": 1_748_372_400,
            "data": [
                {"b64_json": STANDARD.encode(PNG_A_BYTES), "media_type": "image/png"},
                {"b64_json": STANDARD.encode(PNG_B_BYTES)}
            ],
            "usage": {"cost": 0.08}
        }),
    )
}

fn example(name: &str) -> Value {
    let examples: Value = serde_json::from_str(EXAMPLES).expect("examples fixture");
    examples[name].clone()
}

/// The same key paths with the same JSON types; values may differ. Every item of an array
/// has the shape of the other array's first item.
fn assert_same_shape(sent: &Value, documented: &Value) {
    fn walk(sent: &Value, documented: &Value, path: &str) {
        match (sent, documented) {
            (Value::Object(a), Value::Object(b)) => {
                let keys = |m: &Map<String, Value>| m.keys().cloned().collect::<Vec<_>>();
                assert_eq!(keys(a), keys(b), "keys at {path}");
                for (key, value) in a {
                    walk(value, &b[key], &format!("{path}.{key}"));
                }
            }
            (Value::Array(a), Value::Array(b)) => {
                let first = b
                    .first()
                    .unwrap_or_else(|| panic!("empty example at {path}"));
                assert!(!a.is_empty(), "empty array at {path}");
                for (i, item) in a.iter().enumerate() {
                    walk(item, first, &format!("{path}[{i}]"));
                }
            }
            (Value::String(_), Value::String(_))
            | (Value::Number(_), Value::Number(_))
            | (Value::Bool(_), Value::Bool(_))
            | (Value::Null, Value::Null) => {}
            (a, b) => panic!("at {path}: sent {a}, documented {b}"),
        }
    }
    walk(sent, documented, "$");
}

fn answered(result: Result<Started, MediaError>) -> MediaFailure {
    match result {
        Err(MediaError::Answered(failure)) => failure,
        other => panic!("expected an answered failure, got {other:?}"),
    }
}

fn files_in(dir: &Path) -> usize {
    std::fs::read_dir(dir).expect("read the folder").count()
}

// ---- model lists and prices ------------------------------------------------------------------

#[tokio::test]
async fn the_catalogues_come_from_the_endpoints_own_client() {
    let server = MockServer::start(|request| match request.path() {
        "/api/v1/images/models" => Reply::raw_json(200, IMAGES.as_bytes()),
        "/api/v1/videos/models" => Reply::raw_json(200, VIDEOS.as_bytes()),
        _ => Reply::not_found(),
    })
    .await;
    let raw = openrouter(&server)
        .fetch_catalog()
        .await
        .expect("catalogue");
    assert_eq!(raw.images, serde_json::from_str::<Value>(IMAGES).unwrap());
    assert_eq!(
        raw.videos,
        Some(serde_json::from_str::<Value>(VIDEOS).unwrap())
    );

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    let bearer = format!("Bearer {KEY}");
    for request in &requests {
        assert_eq!(request.method, "GET");
        assert_eq!(request.header("authorization"), Some(bearer.as_str()));
    }
}

#[tokio::test]
async fn the_price_answer_comes_from_the_models_endpoints() {
    let server = MockServer::start(|request| match request.path() {
        "/api/v1/images/models/openai/gpt-image-2.6-flare/endpoints" => {
            let endpoints: Value = serde_json::from_str(ENDPOINTS).unwrap();
            Reply::json(200, &endpoints["answers"][FLARE])
        }
        _ => Reply::not_found(),
    })
    .await;
    let media = openrouter(&server);
    let answer = media.fetch_image_price(FLARE).await.expect("price answer");
    assert_eq!(answer["endpoints"][0]["provider_slug"], "openai");

    let missing = media.fetch_image_price("openai/no-such-model").await;
    assert!(
        matches!(&missing, Err(MediaError::Answered(f)) if f.kind == FailureKind::NotFound),
        "{missing:?}"
    );
}

#[tokio::test]
async fn an_openrouter_endpoint_has_a_media_provider() {
    let mut config = ClientConfig::new("https://openrouter.ai/api/v1", ProviderKind::OpenRouter);
    config.api_key = Some(KEY.into());
    let media =
        media_provider(OpenAiClient::new(config).unwrap()).expect("OpenRouter makes pictures");
    assert_eq!(media.company(), Company::OpenRouter);
    assert_eq!(media.host(), "openrouter.ai");

    let generic = ClientConfig::new("https://example.com/v1", ProviderKind::Generic);
    assert!(media_provider(OpenAiClient::new(generic).unwrap()).is_none());
}

// ---- pictures --------------------------------------------------------------------------------

#[tokio::test]
async fn a_picture_run_decodes_every_picture() {
    let server = MockServer::start(|_| pictures_answer()).await;
    let media = openrouter(&server);
    let pictures = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();

    let Started::Done { outputs, cost } = media
        .start(&req_with_ref(pictures.path()), dir.path())
        .await
        .unwrap()
    else {
        panic!("pictures come back done")
    };
    assert_eq!(outputs.len(), 2);
    assert_eq!(std::fs::read(&outputs[0].path).unwrap(), PNG_A_BYTES);
    assert_eq!(std::fs::read(&outputs[1].path).unwrap(), PNG_B_BYTES);
    assert_eq!(outputs[0].mime, "image/png");
    assert_eq!(outputs[1].mime, "image/png", "read from the bytes");
    assert_eq!(outputs[1].size_bytes, PNG_B_BYTES.len() as u64);
    assert_eq!(
        cost,
        Some(Cost {
            usd: 0.08,
            reported: true
        })
    );
    assert_eq!(files_in(dir.path()), 2, "the answer's own file is gone");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let sent = requests[0].json();
    assert_eq!(sent["model"], "openai/gpt-image-2.6-flare");
    assert_eq!(sent["prompt"], "a lighthouse at dusk");
    assert_eq!(sent["n"], 2);
    let url = sent["input_references"][0]["image_url"]["url"]
        .as_str()
        .unwrap();
    assert!(url.starts_with("data:image/png;base64,"));
    assert_eq!(
        STANDARD
            .decode(&url["data:image/png;base64,".len()..])
            .unwrap(),
        PNG_A_BYTES
    );
    assert!(sent.get("provider").is_none(), "no routing, no options");
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path(), "/api/v1/images");
    assert_eq!(
        requests[0].header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
}

#[tokio::test]
async fn a_500_is_never_sent_again() {
    let server = MockServer::start(|_| {
        Reply::json(
            500,
            &json!({"error": {"code": 500, "message": "Internal Server Error"}}),
        )
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let result = openrouter(&server).start(&req(), dir.path()).await;
    assert!(matches!(result, Err(MediaError::Unknown(_))), "{result:?}");
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn a_timeout_is_never_sent_again() {
    let server = MockServer::start(|_| Reply::Silence(Duration::from_secs(3))).await;
    let dir = tempfile::tempdir().unwrap();
    let result = openrouter(&server).start(&req(), dir.path()).await;
    assert!(matches!(result, Err(MediaError::Unknown(_))), "{result:?}");
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn a_cut_connection_is_never_sent_again() {
    let server = MockServer::start(|_| Reply::Stream {
        status: 200,
        content_type: "application/json".into(),
        chunks: vec![b"{\"data\":[{\"b64_json\":\"iVBORw0KGgo".to_vec()],
        delay: Duration::ZERO,
        end: StreamEnd::Abort,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let result = openrouter(&server).start(&req(), dir.path()).await;
    assert!(matches!(result, Err(MediaError::Unknown(_))), "{result:?}");
    assert_eq!(server.requests().len(), 1);
    assert_eq!(files_in(dir.path()), 0, "the half answer is removed");
}

#[tokio::test]
async fn a_429_with_a_short_wait_is_sent_once_more() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let server = MockServer::start(move |_| {
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply::WithHeaders {
                status: 429,
                headers: vec![("retry-after".into(), "1".into())],
                content_type: "application/json".into(),
                body: br#"{"error":{"code":429,"message":"Rate limit exceeded"}}"#.to_vec(),
            }
        } else {
            pictures_answer()
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let result = openrouter(&server).start(&req(), dir.path()).await;
    assert!(matches!(result, Ok(Started::Done { .. })), "{result:?}");
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn a_429_with_a_long_wait_is_not_sent_again() {
    let server = MockServer::start(|_| Reply::WithHeaders {
        status: 429,
        headers: vec![("retry-after".into(), "31".into())],
        content_type: "application/json".into(),
        body: br#"{"error":{"code":429,"message":"Rate limit exceeded"}}"#.to_vec(),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let f = answered(openrouter(&server).start(&req(), dir.path()).await);
    assert_eq!(f.kind, FailureKind::RateLimited);
    assert_eq!(f.retry_after, Some(Duration::from_secs(31)));
    assert_eq!(f.status, Some(429));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn a_refused_connection_was_not_sent() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let media = openrouter_at(&format!("http://127.0.0.1:{port}/api/v1"));
    let dir = tempfile::tempdir().unwrap();
    let result = media.start(&req(), dir.path()).await;
    assert!(matches!(result, Err(MediaError::NotSent(_))), "{result:?}");
}

#[tokio::test]
async fn a_502_was_not_charged() {
    let server = MockServer::start(|_| {
        Reply::json(
            502,
            &json!({"error": {"code": 502, "message": "The provider returned an error"}}),
        )
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let f = answered(openrouter(&server).start(&req(), dir.path()).await);
    assert!(
        matches!(
            f,
            MediaFailure {
                kind: FailureKind::Server,
                charged: Some(false),
                status: Some(502),
                ..
            }
        ),
        "{f:?}"
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn a_moderation_error_is_blocked() {
    let server = MockServer::start(|_| {
        Reply::json(
            403,
            &json!({"error": {
                "code": 403,
                "message": "Your prompt was flagged by moderation",
                "metadata": {"reasons": ["violence"]}
            }}),
        )
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let f = answered(openrouter(&server).start(&req(), dir.path()).await);
    assert_eq!(f.kind, FailureKind::Blocked);
    assert_eq!(f.charged, Some(false));
    assert_eq!(f.status, Some(403));
    assert_eq!(f.message, "Your prompt was flagged by moderation");
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn the_key_never_appears_in_an_error() {
    let server = MockServer::start(|_| {
        let message = format!("The key {KEY} may not use this model");
        Reply::json(400, &json!({"error": {"code": 400, "message": message}}))
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let result = openrouter(&server).start(&req(), dir.path()).await;
    assert!(!format!("{result:?}").contains(KEY));
    let f = answered(result);
    assert!(f.message.contains("[API key]"), "{}", f.message);
    assert!(!f.message.contains(KEY));
    assert_eq!(f.kind, FailureKind::BadRequest);
}

#[tokio::test]
async fn a_body_over_the_cap_is_refused_and_its_file_removed() {
    let body = format!(
        "{{\"data\":[{{\"b64_json\":\"{}\"}}]}}",
        "A".repeat(100 * 1024)
    );
    // Once with its length stated, once streamed in pieces without it.
    let stated = Reply::raw_json(200, body.as_bytes());
    let streamed = Reply::Stream {
        status: 200,
        content_type: "application/json".into(),
        chunks: body
            .as_bytes()
            .chunks(8 * 1024)
            .map(<[u8]>::to_vec)
            .collect(),
        delay: Duration::ZERO,
        end: StreamEnd::Clean,
    };
    for reply in [stated, streamed] {
        let server = MockServer::start(move |_| reply.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let media = openrouter(&server);
        assert!(matches!(
            media.start(&req(), dir.path()).await,
            Err(MediaError::Local(LocalIo::TooLarge(65_536)))
        ));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        assert_eq!(server.requests().len(), 1);
    }
}

#[tokio::test]
async fn the_picture_body_matches_openrouters_documented_shape() {
    let server = MockServer::start(|_| pictures_answer()).await;
    let media = openrouter(&server);
    let pictures = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    media
        .start(&req_with_ref_routing_and_extra(pictures.path()), dir.path())
        .await
        .unwrap();
    let sent = server.requests()[0].json();
    assert_same_shape(
        &sent["input_references"][0],
        &example("image_references")["input_references"][0],
    );
    assert_eq!(sent["provider"]["only"], json!(["openai"]));
    assert_eq!(sent["provider"]["allow_fallbacks"], false);
    let routing = json!({
        "only": sent["provider"]["only"],
        "allow_fallbacks": sent["provider"]["allow_fallbacks"],
    });
    assert_same_shape(&routing, &example("image_routing")["provider"]);
    assert_same_shape(
        &sent["provider"]["options"]["openai"],
        &json!({"complexity": "high"}),
    );
    assert!(sent["provider"].get("order").is_none());
    assert!(sent["provider"].get("zdr").is_none(), "zdr only when asked");
}

#[tokio::test]
async fn extra_settings_go_under_provider_options_for_each_company_that_lists_them() {
    let server = MockServer::start(|_| pictures_answer()).await;
    let media = openrouter(&server);
    let dir = tempfile::tempdir().unwrap();
    let (extra, provider_options) =
        place_extra(&flare(), object(json!({"moderation": "low"})), None);
    let request = MediaRequest {
        extra,
        provider_options,
        ..req()
    };
    media.start(&request, dir.path()).await.unwrap();
    let sent = server.requests()[0].json();
    assert_eq!(
        sent["provider"]["options"]["openai"],
        json!({"moderation": "low"})
    );
    assert!(sent.get("moderation").is_none());
}

#[tokio::test]
async fn the_preview_hides_pictures_and_the_key() {
    let server = MockServer::start(|_| pictures_answer()).await;
    let media = openrouter(&server);
    let pictures = tempfile::tempdir().unwrap();
    let p = media.preview(&req_with_ref(pictures.path()));
    assert_eq!(p.host, "127.0.0.1");
    assert_eq!(p.method, "POST");
    assert_eq!(p.url, format!("{}/api/v1/images", server.url()));
    let shown = p.body["input_references"][0].as_str().unwrap();
    assert!(shown.starts_with("picture 1 (sha "), "{shown}");
    assert!(shown.ends_with(" B)"), "{shown}");
    assert_eq!(p.body["model"], FLARE);
    assert_eq!(p.body["n"], 2);
    assert!(!p.body.to_string().contains(KEY));
    assert!(server.requests().is_empty(), "a preview sends nothing");
}
