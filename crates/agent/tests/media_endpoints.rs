//! Studio (0.1.0): OpenRouter's picture and video endpoints, and OpenAI's picture endpoints,
//! against a local fake server that answers in the documented formats. The request that
//! creates a run or a job is sent at most once: never again after a timeout, a server error or
//! a cut connection; once more only on a short rate limit or a connection that never opened. A
//! video's result is downloaded with the key from OpenRouter's own host only; a link elsewhere
//! is handed back, not followed. No API key or network needed.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use agent::media::openrouter::place_extra;
use agent::media::{
    Company, Cost, FailureKind, Fetched, JobDone, JobState, LocalIo, MediaError, MediaFailure,
    MediaKind, MediaLimits, MediaProvider, MediaRequest, OpenAiImages, OpenRouterMedia, RefImage,
    RefRole, Routing, SignedUrl, Stage, Started, media_provider, openrouter_image_models,
    parse_catalog,
};
use agent::{ClientConfig, OpenAiClient, ProviderKind};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use common::{MockServer, Reply, StreamEnd};
use serde_json::{Map, Value, json};
use url::Url;

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
    let placed = place_extra(
        &flare(),
        object(json!({"complexity": "high"})),
        Some(&routing),
    );
    assert!(placed.dropped.is_empty());
    MediaRequest {
        extra: placed.top,
        provider_options: placed.options,
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
    // Windows refuses a closed loopback port only after about 2 s, so the limit must cover
    // the first connect, the 1 s retry wait and the second connect; a timeout would be
    // Unknown, never NotSent.
    let media =
        openrouter_at(&format!("http://127.0.0.1:{port}/api/v1")).with_limits(MediaLimits {
            picture_timeout: Duration::from_secs(15),
            ..quick()
        });
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
    let placed = place_extra(&flare(), object(json!({"moderation": "low"})), None);
    let request = MediaRequest {
        extra: placed.top,
        provider_options: placed.options,
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

// ---- video -----------------------------------------------------------------------------------

const VEO: &str = "google/veo-3.1";
/// A small file that starts like an MP4 video (an `ftyp` box).
const MP4_BYTES: &[u8] = b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom video";

/// A Veo 3.1 request with a start frame, written into `pictures`.
fn video_req(pictures: &Path) -> MediaRequest {
    let path = pictures.join("first.png");
    std::fs::write(&path, PNG_A_BYTES).expect("write the start frame");
    MediaRequest {
        kind: MediaKind::Video,
        model: VEO.into(),
        prompt: "waves rolling onto a beach".into(),
        params: object(json!({
            "duration": 4,
            "resolution": "720p",
            "aspect_ratio": "16:9",
            "generate_audio": false,
            "seed": 81723
        })),
        extra: Map::new(),
        refs: vec![RefImage {
            role: RefRole::StartFrame,
            mime: "image/png".into(),
            path,
            sha256: "3f2a9c41d0b7e6a5f4c3b2a1908f7e6d5c4b3a291807f6e5d4c3b2a190817263".into(),
            size_bytes: PNG_A_BYTES.len() as u64,
        }],
        routing: None,
        provider_options: BTreeMap::new(),
    }
}

/// A finished job with one result and no signed links.
fn done() -> JobDone {
    JobDone {
        outputs: 1,
        signed: Vec::new(),
        cost: None,
    }
}

fn submitted() -> Reply {
    Reply::json(
        202,
        &json!({
            "id": "vid_1",
            "polling_url": "https://openrouter.ai/api/v1/videos/vid_1",
            "status": "pending"
        }),
    )
}

fn mp4(body: &[u8]) -> Reply {
    Reply::Raw {
        status: 200,
        content_type: "video/mp4".into(),
        body: body.to_vec(),
    }
}

/// A server whose job check always gives `answer`.
async fn checked(answer: Reply) -> MockServer {
    MockServer::start(move |request| match request.path() {
        "/api/v1/videos/vid_1" => answer.clone(),
        _ => Reply::not_found(),
    })
    .await
}

/// A server whose content endpoint always gives `reply`.
async fn content(reply: Reply) -> MockServer {
    MockServer::start(move |request| match request.path() {
        "/api/v1/videos/vid_1/content" => reply.clone(),
        _ => Reply::not_found(),
    })
    .await
}

fn with(server: &MockServer, limits: MediaLimits) -> OpenRouterMedia {
    openrouter(server).with_limits(limits)
}

#[tokio::test]
async fn a_video_job_starts_checks_and_downloads() {
    let checks = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&checks);
    let server = MockServer::start(move |request| {
        match (request.method.as_str(), request.path()) {
            ("POST", "/api/v1/videos") => submitted(),
            ("GET", "/api/v1/videos/vid_1") => match seen.fetch_add(1, Ordering::SeqCst) {
                0 => Reply::json(200, &json!({"id": "vid_1", "status": "pending"})),
                1 => Reply::json(
                    200,
                    &json!({"id": "vid_1", "status": "in_progress", "progress": 0.4}),
                ),
                _ => Reply::json(
                    200,
                    &json!({
                        "id": "vid_1",
                        "generation_id": "gen-1234567890-abcdef",
                        "status": "completed",
                        "unsigned_urls": ["https://openrouter.ai/api/v1/videos/vid_1/content?index=0"],
                        "usage": {"cost": 0.36, "is_byok": false}
                    }),
                ),
            },
            ("GET", "/api/v1/videos/vid_1/content") => mp4(MP4_BYTES),
            _ => Reply::not_found(),
        }
    })
    .await;
    let media = openrouter(&server);
    let pictures = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();

    assert_eq!(
        media
            .start(&video_req(pictures.path()), dir.path())
            .await
            .unwrap(),
        Started::Job {
            remote_id: "vid_1".into()
        }
    );
    assert_eq!(files_in(dir.path()), 0, "the answer's own file is gone");
    assert_eq!(media.check("vid_1").await.unwrap(), JobState::Pending);
    assert_eq!(
        media.check("vid_1").await.unwrap(),
        JobState::Running {
            progress: Some(0.4)
        }
    );
    let JobState::Done(job) = media.check("vid_1").await.unwrap() else {
        panic!("the third check finds it done")
    };
    assert_eq!(
        job,
        JobDone {
            outputs: 1,
            signed: Vec::new(),
            cost: Some(Cost {
                usd: 0.36,
                reported: true
            }),
        }
    );

    let dest = dir.path().join("v.mp4");
    let Fetched::Saved(saved) = media.download("vid_1", &job, 0, &dest).await.unwrap() else {
        panic!("the result is on OpenRouter's own host")
    };
    assert_eq!(saved.path, dest);
    assert_eq!(saved.mime, "video/mp4");
    assert_eq!(saved.size_bytes, MP4_BYTES.len() as u64);
    assert_eq!(std::fs::read(&dest).unwrap(), MP4_BYTES);

    let requests = server.requests();
    let bearer = format!("Bearer {KEY}");
    let trail: Vec<String> = requests
        .iter()
        .map(|r| format!("{} {}", r.method, r.target))
        .collect();
    assert_eq!(
        trail,
        [
            "POST /api/v1/videos",
            "GET /api/v1/videos/vid_1",
            "GET /api/v1/videos/vid_1",
            "GET /api/v1/videos/vid_1",
            "GET /api/v1/videos/vid_1/content?index=0",
        ]
    );
    for request in &requests {
        assert_eq!(request.header("authorization"), Some(bearer.as_str()));
    }
    let sent = requests[0].json();
    assert_eq!(sent["model"], VEO);
    assert_eq!(sent["prompt"], "waves rolling onto a beach");
    assert_eq!(sent["duration"], 4);
    assert_eq!(sent["resolution"], "720p");
    assert_eq!(sent["aspect_ratio"], "16:9");
    assert_eq!(sent["generate_audio"], false);
    assert_eq!(sent["seed"], 81723);
    assert_eq!(sent["frame_images"][0]["frame_type"], "first_frame");
    let url = sent["frame_images"][0]["image_url"]["url"]
        .as_str()
        .unwrap();
    assert_eq!(
        STANDARD
            .decode(url.strip_prefix("data:image/png;base64,").unwrap())
            .unwrap(),
        PNG_A_BYTES
    );
}

#[tokio::test]
async fn a_failed_job_says_whether_it_was_charged() {
    let server = checked(Reply::json(
        200,
        &json!({
            "id": "vid_1",
            "generation_id": "gen-77",
            "status": "failed",
            "error": "The provider could not make this video"
        }),
    ))
    .await;
    let JobState::Failed(f) = openrouter(&server).check("vid_1").await.unwrap() else {
        panic!("a failed job is a failed state")
    };
    assert_eq!(f.kind, FailureKind::Other);
    assert_eq!(
        f.charged,
        Some(false),
        "OpenRouter does not bill a failed job"
    );
    assert_eq!(f.message, "The provider could not make this video");
    assert_eq!(f.request_id.as_deref(), Some("gen-77"));

    // A job whose result has expired: OpenRouter reported it done before.
    let server = checked(Reply::json(
        200,
        &json!({"id": "vid_1", "status": "expired"}),
    ))
    .await;
    let JobState::Failed(f) = openrouter(&server).check("vid_1").await.unwrap() else {
        panic!("an expired job is a failed state")
    };
    assert_eq!(f.kind, FailureKind::Expired);
    assert_eq!(f.charged, None);
}

#[tokio::test]
async fn a_job_the_company_no_longer_has_is_gone() {
    let server = checked(Reply::not_found()).await;
    assert_eq!(
        openrouter(&server).check("vid_1").await.unwrap(),
        JobState::Gone
    );
}

#[tokio::test]
async fn a_check_that_times_out_is_an_error_not_a_state() {
    let server = checked(Reply::Silence(Duration::from_secs(3))).await;
    let media = with(
        &server,
        MediaLimits {
            check_timeout: Duration::from_secs(1),
            ..quick()
        },
    );
    let begun = Instant::now();
    let result = media.check("vid_1").await;
    assert!(matches!(result, Err(MediaError::Unknown(_))), "{result:?}");
    assert!(begun.elapsed() < Duration::from_millis(2500));
}

#[tokio::test]
async fn content_gone_after_done_is_expired() {
    for status in [404, 410] {
        let server = content(Reply::json(
            status,
            &json!({"error": {"code": status, "message": "Video not found"}}),
        ))
        .await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("v.mp4");
        let result = openrouter(&server)
            .download("vid_1", &done(), 0, &dest)
            .await;
        let Err(MediaError::Answered(f)) = result else {
            panic!("{status}: expected an answered failure, got {result:?}")
        };
        assert_eq!(f.kind, FailureKind::Expired, "{status}");
        assert_eq!(f.status, Some(status));
        assert!(!dest.exists());
    }
}

#[tokio::test]
async fn a_download_over_the_cap_removes_the_partial_file() {
    let body = vec![7u8; 4096];
    // Once with its length stated, once streamed in pieces without it.
    let streamed = Reply::Stream {
        status: 200,
        content_type: "video/mp4".into(),
        chunks: body.chunks(1024).map(<[u8]>::to_vec).collect(),
        delay: Duration::ZERO,
        end: StreamEnd::Clean,
    };
    for reply in [mp4(&body), streamed] {
        let server = content(reply).await;
        let media = with(
            &server,
            MediaLimits {
                video_cap: 1024,
                ..quick()
            },
        );
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("v.mp4");
        let result = media.download("vid_1", &done(), 0, &dest).await;
        assert!(
            matches!(result, Err(MediaError::Local(LocalIo::TooLarge(1024)))),
            "{result:?}"
        );
        assert!(!dest.exists());
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_full_disk_while_downloading_is_named() {
    let server = content(mp4(MP4_BYTES)).await;
    let dir = tempfile::tempdir().unwrap();
    // /dev/full accepts the open and refuses every write with ENOSPC. A link to it, so that
    // removing the partial file removes only the link.
    let dest = dir.path().join("v.mp4");
    std::os::unix::fs::symlink("/dev/full", &dest).unwrap();
    let result = openrouter(&server)
        .download("vid_1", &done(), 0, &dest)
        .await;
    assert!(
        matches!(result, Err(MediaError::Local(LocalIo::DiskFull))),
        "{result:?}"
    );
    assert!(
        std::fs::symlink_metadata(&dest).is_err(),
        "the link is gone"
    );
    assert!(Path::new("/dev/full").exists());
}

#[tokio::test]
async fn a_stalled_download_stops_after_the_idle_limit() {
    let server = content(Reply::Stream {
        status: 200,
        content_type: "video/mp4".into(),
        chunks: vec![vec![0u8; 1024]],
        delay: Duration::ZERO,
        end: StreamEnd::HoldOpen,
    })
    .await;
    let media = with(
        &server,
        MediaLimits {
            download_idle: Duration::from_millis(200),
            ..quick()
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("v.mp4");
    let begun = Instant::now();
    let result = media.download("vid_1", &done(), 0, &dest).await;
    assert!(matches!(result, Err(MediaError::Unknown(_))), "{result:?}");
    assert!(begun.elapsed() < Duration::from_secs(1));
    assert!(!dest.exists());
}

#[tokio::test]
async fn the_video_body_matches_openrouters_documented_shape() {
    let server = MockServer::start(|_| submitted()).await;
    let pictures = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    // Routing, extra settings and company settings are never sent for video.
    let request = MediaRequest {
        extra: object(json!({"watermark": false})),
        routing: Some(Routing {
            order: vec!["google-vertex".into()],
            only: true,
            allow_fallbacks: false,
            zdr: true,
        }),
        provider_options: BTreeMap::from([(
            "google-vertex".to_owned(),
            object(json!({"parameters": {"personGeneration": "allow"}})),
        )]),
        ..video_req(pictures.path())
    };
    openrouter(&server)
        .start(&request, dir.path())
        .await
        .unwrap();
    let sent = server.requests()[0].json();
    assert_same_shape(
        &sent["frame_images"][0],
        &example("video_frame_images")["frame_images"][0],
    );
    assert!(sent.get("provider").is_none());
    let mut keys: Vec<&str> = sent
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "aspect_ratio",
            "duration",
            "frame_images",
            "generate_audio",
            "model",
            "prompt",
            "resolution",
            "seed"
        ],
        "no other key"
    );
    assert_eq!(server.requests()[0].path(), "/api/v1/videos");
}

#[tokio::test]
async fn a_video_size_sends_neither_resolution_nor_shape() {
    let server = MockServer::start(|_| submitted()).await;
    let pictures = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut request = video_req(pictures.path());
    request.params.insert("size".to_owned(), json!("1280x720"));
    openrouter(&server)
        .start(&request, dir.path())
        .await
        .unwrap();
    let sent = server.requests()[0].json();
    assert_eq!(sent["size"], "1280x720");
    assert!(sent.get("resolution").is_none(), "{sent}");
    assert!(sent.get("aspect_ratio").is_none(), "{sent}");
    assert_eq!(sent["duration"], 4);
}

#[tokio::test]
async fn a_video_create_is_sent_at_most_twice_on_rate_limits() {
    let server = MockServer::start(|_| Reply::WithHeaders {
        status: 429,
        headers: vec![("retry-after".into(), "1".into())],
        content_type: "application/json".into(),
        body: br#"{"error":{"code":429,"message":"Rate limit exceeded"}}"#.to_vec(),
    })
    .await;
    let pictures = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let f = answered(
        openrouter(&server)
            .start(&video_req(pictures.path()), dir.path())
            .await,
    );
    assert_eq!(f.kind, FailureKind::RateLimited);
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn a_link_to_another_host_is_handed_back_not_followed() {
    let other = MockServer::start(|_| mp4(MP4_BYTES)).await;
    let link = format!("{}/v.mp4", other.url());
    let server = content(Reply::Redirect {
        status: 302,
        location: link.clone(),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("v.mp4");
    assert_eq!(
        openrouter(&server)
            .download("vid_1", &done(), 0, &dest)
            .await
            .unwrap(),
        Fetched::Elsewhere(Url::parse(&link).unwrap())
    );
    assert!(other.requests().is_empty(), "{:?}", other.requests());
    assert_eq!(server.requests().len(), 1);
    assert!(!dest.exists());
}

#[tokio::test]
async fn signed_urls_are_kept_for_the_guarded_fetcher() {
    let server = checked(Reply::json(
        200,
        &json!({
            "id": "vid_1",
            "status": "completed",
            "urls": [{"url": "https://cdn.example/v.mp4", "expires_at": 1_794_900_000}],
            "usage": {"cost": 0.5}
        }),
    ))
    .await;
    let media = openrouter(&server);
    let JobState::Done(job) = media.check("vid_1").await.unwrap() else {
        panic!("a completed job is done")
    };
    assert_eq!(job.outputs, 1);
    assert_eq!(
        job.signed[0],
        SignedUrl {
            url: Url::parse("https://cdn.example/v.mp4").unwrap(),
            expires_at: Some(1_794_900_000),
        }
    );
    // Only a signed link: it goes to the guarded fetcher, without the key.
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        media
            .download("vid_1", &job, 0, &dir.path().join("v.mp4"))
            .await
            .unwrap(),
        Fetched::Elsewhere(Url::parse("https://cdn.example/v.mp4").unwrap())
    );
    assert_eq!(
        server.requests().len(),
        1,
        "the content endpoint was not asked"
    );
}

#[tokio::test]
async fn the_video_preview_shows_frames_by_their_hash() {
    let server = MockServer::start(|_| submitted()).await;
    let pictures = tempfile::tempdir().unwrap();
    let p = openrouter(&server).preview(&video_req(pictures.path()));
    assert_eq!(p.url, format!("{}/api/v1/videos", server.url()));
    assert_eq!(p.body["model"], VEO);
    assert_eq!(p.body["duration"], 4);
    let frame = &p.body["frame_images"][0];
    assert_eq!(frame["frame_type"], "first_frame");
    let shown = frame["image_url"]["url"].as_str().unwrap();
    assert!(shown.starts_with("picture 1 (sha 3f2a9c41d0b7"), "{shown}");
    assert!(server.requests().is_empty(), "a preview sends nothing");
}

// ---- OpenAI pictures -------------------------------------------------------------------------

const OPENAI_KEY: &str = "sk-proj-studio-test-key-0123456789";
const OPENAI_FLARE: &str = "gpt-image-2.6-flare";
const OPENAI_MODELS: &str = include_str!("fixtures/media/openai-models.json");
const OPENAI_ANSWER: &str = include_str!("fixtures/media/openai-images-answer.json");
const OPENAI_BLOCKED: &str = include_str!("fixtures/media/openai-moderation-blocked.json");
const OPENAI_UNVERIFIED: &str = include_str!("fixtures/media/openai-org-verification.json");

fn openai(server: &MockServer) -> OpenAiImages {
    let mut config = ClientConfig::new(format!("{}/v1", server.url()), ProviderKind::OpenAi);
    config.api_key = Some(OPENAI_KEY.into());
    OpenAiImages::new(OpenAiClient::new(config).expect("client")).with_limits(quick())
}

/// A low-quality square GPT Image 2.6 Flare picture.
fn openai_req() -> MediaRequest {
    MediaRequest {
        model: OPENAI_FLARE.into(),
        params: object(json!({"n": 1, "size": "1024x1024", "quality": "low"})),
        ..req()
    }
}

/// [`openai_req`] with one added picture, written into `pictures`.
fn openai_req_with_ref(pictures: &Path) -> MediaRequest {
    MediaRequest {
        model: OPENAI_FLARE.into(),
        params: object(json!({"n": 1, "size": "1024x1024", "quality": "low"})),
        ..req_with_ref(pictures)
    }
}

fn openai_error(status: u16, body: &str) -> Reply {
    Reply::WithHeaders {
        status,
        headers: vec![("x-request-id".into(), "req_8f2".into())],
        content_type: "application/json".into(),
        body: body.as_bytes().to_vec(),
    }
}

#[tokio::test]
async fn an_openai_endpoint_has_a_media_provider() {
    let mut config = ClientConfig::new("https://api.openai.com/v1", ProviderKind::OpenAi);
    config.api_key = Some(OPENAI_KEY.into());
    let media = media_provider(OpenAiClient::new(config).unwrap()).expect("OpenAI makes pictures");
    assert_eq!(media.company(), Company::OpenAi);
    assert_eq!(media.host(), "api.openai.com");
    // The prices are in the bundled settings: nothing to fetch.
    assert_eq!(media.fetch_image_price(OPENAI_FLARE).await, Ok(Value::Null));
}

#[tokio::test]
async fn the_openai_list_keeps_picture_models_only() {
    let server = MockServer::start(|request| match request.path() {
        "/v1/models" => Reply::raw_json(200, OPENAI_MODELS.as_bytes()),
        _ => Reply::not_found(),
    })
    .await;
    let raw = openai(&server).fetch_catalog().await.expect("model list");
    assert_eq!(
        raw.images,
        serde_json::from_str::<Value>(OPENAI_MODELS).unwrap()
    );
    assert_eq!(raw.videos, None);
    let models = parse_catalog(Company::OpenAi, &raw, &BTreeMap::new());
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["gpt-image-2.6-flare", "gpt-image-2.6-sunburst"]);
    assert_eq!(models[0].display_name, "GPT Image 2.6 Flare");
    assert_eq!(models[0].created, Some(1_793_835_351));

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].header("authorization"),
        Some(format!("Bearer {OPENAI_KEY}").as_str())
    );
}

#[tokio::test]
async fn an_openai_picture_is_one_json_post() {
    let server = MockServer::start(|_| Reply::raw_json(200, OPENAI_ANSWER.as_bytes())).await;
    let dir = tempfile::tempdir().unwrap();
    let Started::Done { outputs, cost } = openai(&server)
        .start(&openai_req(), dir.path())
        .await
        .unwrap()
    else {
        panic!("pictures come back done")
    };
    assert_eq!(outputs.len(), 1);
    assert_eq!(std::fs::read(&outputs[0].path).unwrap(), PNG_A_BYTES);
    assert_eq!(outputs[0].mime, "image/png");
    // 50 text tokens at $5 and 272 picture tokens at $30, per million.
    let cost = cost.expect("worked out from the usage");
    assert!((cost.usd - 0.008_41).abs() < 1e-9, "{cost:?}");
    assert!(!cost.reported);
    assert_eq!(files_in(dir.path()), 1, "the answer's own file is gone");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path(), "/v1/images/generations");
    assert_eq!(
        requests[0].header("authorization"),
        Some(format!("Bearer {OPENAI_KEY}").as_str())
    );
    assert_eq!(
        requests[0].json(),
        json!({
            "model": OPENAI_FLARE,
            "prompt": "a lighthouse at dusk",
            "n": 1,
            "size": "1024x1024",
            "quality": "low"
        })
    );
}

#[tokio::test]
async fn an_openai_edit_is_one_multipart_post() {
    let server = MockServer::start(|_| Reply::raw_json(200, OPENAI_ANSWER.as_bytes())).await;
    let pictures = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let started = openai(&server)
        .start(&openai_req_with_ref(pictures.path()), dir.path())
        .await;
    assert!(matches!(started, Ok(Started::Done { .. })), "{started:?}");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path(), "/v1/images/edits");
    let kind = requests[0].header("content-type").expect("a content type");
    let boundary = kind
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap_or_else(|| panic!("not a form: {kind}"));
    let text = |name: &str, value: &str| {
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
    };
    let mut expected = Vec::new();
    for (name, value) in [
        ("model", OPENAI_FLARE),
        ("prompt", "a lighthouse at dusk"),
        ("n", "1"),
        ("quality", "low"),
        ("size", "1024x1024"),
    ] {
        expected.extend(text(name, value).into_bytes());
    }
    expected.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"image[]\"; \
             filename=\"picture-1.png\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .into_bytes(),
    );
    expected.extend(PNG_A_BYTES);
    expected.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
    assert_eq!(
        String::from_utf8_lossy(&requests[0].body),
        String::from_utf8_lossy(&expected)
    );
}

#[tokio::test]
async fn moderation_blocked_is_explained() {
    let server = MockServer::start(|_| openai_error(400, OPENAI_BLOCKED)).await;
    let dir = tempfile::tempdir().unwrap();
    let f = answered(openai(&server).start(&openai_req(), dir.path()).await);
    assert_eq!(f.kind, FailureKind::Blocked);
    assert_eq!(f.company_code.as_deref(), Some("moderation_blocked"));
    assert_eq!(f.request_id.as_deref(), Some("req_8f2"));
    assert_eq!(f.stage, Some(Stage::Prompt), "moderation_stage: input");
    assert_eq!(f.charged, None, "OpenAI says nothing about the charge");
    assert_eq!(f.status, Some(400));
    assert!(
        f.message.starts_with("Your request was rejected"),
        "{}",
        f.message
    );
    assert_eq!(server.requests().len(), 1, "never sent again");
}

#[tokio::test]
async fn organization_verification_is_named() {
    let server = MockServer::start(|_| openai_error(403, OPENAI_UNVERIFIED)).await;
    let dir = tempfile::tempdir().unwrap();
    let f = answered(openai(&server).start(&openai_req(), dir.path()).await);
    assert_eq!(f.kind, FailureKind::OrgVerification);
    assert_eq!(f.charged, Some(false));
    assert_eq!(f.request_id.as_deref(), Some("req_8f2"));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn an_openai_500_is_never_sent_again() {
    let server = MockServer::start(|_| {
        openai_error(
            500,
            r#"{"error": {"message": "The server had an error", "code": null}}"#,
        )
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let result = openai(&server).start(&openai_req(), dir.path()).await;
    assert!(matches!(result, Err(MediaError::Unknown(_))), "{result:?}");
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn the_openai_preview_names_the_endpoint_and_hides_pictures() {
    let server = MockServer::start(|_| Reply::not_found()).await;
    let media = openai(&server);
    let p = media.preview(&openai_req());
    assert_eq!(p.method, "POST");
    assert_eq!(p.url, format!("{}/v1/images/generations", server.url()));
    assert_eq!(p.host, "127.0.0.1");
    assert_eq!(p.body["n"], 1);

    let pictures = tempfile::tempdir().unwrap();
    let p = media.preview(&openai_req_with_ref(pictures.path()));
    assert_eq!(p.url, format!("{}/v1/images/edits", server.url()));
    assert_eq!(p.body["model"], OPENAI_FLARE);
    assert_eq!(p.body["n"], "1", "a form sends text");
    let shown = p.body["image[]"][0].as_str().unwrap();
    assert!(shown.starts_with("picture 1 (sha 3f2a9c41d0b7"), "{shown}");
    assert!(!p.body.to_string().contains(OPENAI_KEY));
    assert!(server.requests().is_empty(), "a preview sends nothing");
}
