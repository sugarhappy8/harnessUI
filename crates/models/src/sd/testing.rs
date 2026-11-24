//! Builders for tests of the tensor index (here and, with `test-support`, in other crates).

use serde_json::{Value, json};

/// A safetensors file with `header` as its JSON header and no tensor data: the header's
/// byte length as a little-endian `u64`, then the JSON.
pub fn safetensors(header: &Value) -> Vec<u8> {
    let json = serde_json::to_vec(header).expect("a JSON value serializes");
    let mut out = (json.len() as u64).to_le_bytes().to_vec();
    out.extend(json);
    out
}

/// A one-tensor header whose JSON is exactly `json_bytes` bytes, padded in `__metadata__`.
///
/// # Panics
/// When `json_bytes` is smaller than the header without padding.
pub fn big_header(json_bytes: usize) -> Value {
    let build = |pad: usize| {
        json!({
            "t": {"dtype": "F16", "shape": [1], "data_offsets": [0, 2]},
            "__metadata__": {"pad": "x".repeat(pad)},
        })
    };
    let base = serde_json::to_vec(&build(0))
        .expect("a JSON value serializes")
        .len();
    assert!(json_bytes >= base, "a header needs at least {base} bytes");
    build(json_bytes - base)
}

// ---- Remote header reads (Task R3) ------------------------------------------------------

/// A GGUF file whose tensor table ends exactly at byte `end` (the table starts at byte 24:
/// no metadata) and whose total length is at least `total`. Names are long, so a big table
/// stays under the tensor-count limit.
///
/// # Panics
/// When `end` is too small for the table to hold even a few tensors.
pub fn gguf_with_table_ending_at(end: u64, total: u64) -> Vec<u8> {
    const NAME: u64 = 256;
    // An entry is: name length, name, dimension count, one dimension, type, offset.
    const FIXED: u64 = 8 + 4 + 8 + 4 + 8;
    let remaining = end - 24;
    let n = remaining.div_ceil(NAME + FIXED);
    let excess = n * (NAME + FIXED) - remaining;
    assert!(n > excess, "a table ending at {end} is too small");
    let mut w = crate::gguf::testing::Writer::new();
    for i in 0..n {
        let len = if i < excess { NAME - 1 } else { NAME } as usize;
        let name = format!("{i:08}{}", "x".repeat(len - 8));
        let data = if i + 1 == n {
            total.saturating_sub(end)
        } else {
            32
        };
        w = w.tensor(&name, data.max(32));
    }
    w.build()
}

/// A GGUF file whose tensor table is `table_bytes` long, inside a file of `total` bytes.
pub fn gguf_claiming_table_bytes(table_bytes: u64, total: u64) -> Vec<u8> {
    gguf_with_table_ending_at(24 + table_bytes, total)
}

/// What a [`RangeServer`] serves: `prefix`, then zeros up to `total` bytes, generated as
/// they are sent (so a 300 MiB body costs no memory).
#[derive(Clone)]
pub struct Body {
    pub prefix: Vec<u8>,
    pub total: u64,
}

impl Body {
    /// A whole file in memory.
    pub fn file(bytes: Vec<u8>) -> Self {
        let total = bytes.len() as u64;
        Body {
            prefix: bytes,
            total,
        }
    }

    fn chunk(&self, from: u64, to: u64) -> Vec<u8> {
        (from..to)
            .map(|i| self.prefix.get(i as usize).copied().unwrap_or(0))
            .collect()
    }
}

/// A safetensors file claiming a `header_len`-byte header, `total` bytes long, mostly zeros.
pub fn streamed_safetensors(header_len: u64, total: u64) -> Body {
    let mut prefix = header_len.to_le_bytes().to_vec();
    prefix.push(b'{');
    Body { prefix, total }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RangeMode {
    /// Answer `Range` with 206 and just the range.
    Honour,
    /// Answer 200 with the whole body.
    Ignore,
}

/// A Hub on this computer: answers `HEAD` on a resolve path with the file's size and the
/// ranged `GET`, and records every ranged request's header and the body bytes it sent.
pub struct RangeServer {
    url: String,
    ranges: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    sent: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl RangeServer {
    pub fn client(&self) -> crate::hf::HfClient {
        crate::hf::HfClient::for_test(&self.url)
    }
    /// The `Range` header of each `GET`, in order.
    pub fn ranges(&self) -> Vec<String> {
        self.ranges.lock().unwrap().clone()
    }
    /// Body bytes written to sockets so far.
    pub fn bytes_sent(&self) -> u64 {
        self.sent.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Runs `answer` for each connection's request line and headers; it returns what to send.
async fn serve<F>(handler: F) -> String
where
    F: Fn(String, Vec<String>) -> Vec<u8> + Send + Sync + 'static,
{
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handler = std::sync::Arc::new(handler);
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            let handler = handler.clone();
            tokio::spawn(async move {
                let (r, mut w) = sock.into_split();
                let mut r = BufReader::new(r);
                let mut line = String::new();
                if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let mut headers = Vec::new();
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).await.unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                    headers.push(h.trim().to_string());
                }
                let _ = w.write_all(&handler(line, headers)).await;
            });
        }
    });
    url
}

/// A server for `body` that honours or ignores `Range`.
pub async fn range_server(body: impl Into<BodyArg>, mode: RangeMode) -> RangeServer {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let body = body.into().0;
    // A small send buffer, so `bytes_sent` is close to what the client read.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_send_buffer_size(64 * 1024).unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let listener = socket.listen(16).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let ranges = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sent = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (ranges2, sent2) = (ranges.clone(), sent.clone());
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            let (body, ranges, sent) = (body.clone(), ranges2.clone(), sent2.clone());
            tokio::spawn(async move {
                let (r, mut w) = sock.into_split();
                let mut r = BufReader::new(r);
                let mut line = String::new();
                if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let head = line.starts_with("HEAD");
                let mut range = None;
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).await.unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                    if h.to_ascii_lowercase().starts_with("range:") {
                        range = Some(h["range:".len()..].trim().to_string());
                    }
                }
                if head {
                    let resp = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", body.total);
                    let _ = w.write_all(resp.as_bytes()).await;
                    return;
                }
                if let Some(h) = &range {
                    ranges.lock().unwrap().push(h.clone());
                }
                let last = range
                    .as_deref()
                    .and_then(|h| h.strip_prefix("bytes=0-"))
                    .and_then(|n| n.parse::<u64>().ok());
                let (status, end) = match (mode, last) {
                    (RangeMode::Honour, Some(n)) => {
                        ("206 Partial Content", (n + 1).min(body.total))
                    }
                    _ => ("200 OK", body.total),
                };
                let mut resp = format!("HTTP/1.1 {status}\r\ncontent-length: {end}\r\n");
                if status.starts_with("206") {
                    resp.push_str(&format!(
                        "content-range: bytes 0-{}/{}\r\n",
                        end - 1,
                        body.total
                    ));
                }
                resp.push_str("\r\n");
                if w.write_all(resp.as_bytes()).await.is_err() {
                    return;
                }
                let mut at = 0;
                while at < end {
                    let to = (at + 64 * 1024).min(end);
                    if w.write_all(&body.chunk(at, to)).await.is_err() {
                        return;
                    }
                    sent.fetch_add(to - at, std::sync::atomic::Ordering::SeqCst);
                    at = to;
                }
            });
        }
    });
    RangeServer { url, ranges, sent }
}

/// A server that answers every request with `status` and a gated-repo error code.
pub async fn status_server(status: u16) -> RangeServer {
    let url = serve(move |_, _| {
        format!("HTTP/1.1 {status} X\r\nx-error-code: GatedRepo\r\ncontent-length: 0\r\n\r\n")
            .into_bytes()
    })
    .await;
    RangeServer {
        url,
        ranges: Default::default(),
        sent: Default::default(),
    }
}

/// A server that redirects every request to `to`.
pub async fn redirect_server(to: &str) -> RangeServer {
    let to = to.to_string();
    let url = serve(move |_, _| {
        format!("HTTP/1.1 302 Found\r\nlocation: {to}\r\ncontent-length: 0\r\n\r\n").into_bytes()
    })
    .await;
    RangeServer {
        url,
        ranges: Default::default(),
        sent: Default::default(),
    }
}

/// A file's bytes or a [`Body`], for [`range_server`].
pub struct BodyArg(Body);

impl From<Vec<u8>> for BodyArg {
    fn from(v: Vec<u8>) -> Self {
        BodyArg(Body::file(v))
    }
}

impl From<Body> for BodyArg {
    fn from(b: Body) -> Self {
        BodyArg(b)
    }
}

// ---- Detection fixtures (Task R4) -------------------------------------------------------

/// Where a fixture's tensor table was read: the pinned file it is a trimmed copy of.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureSource {
    pub repo: String,
    pub revision: String,
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// A date, or `"hand-built"`.
    pub captured_on: String,
}

#[derive(serde::Deserialize)]
struct FixtureFile {
    source: FixtureSource,
    format: crate::sd::index::FileFormat,
    metadata: std::collections::BTreeMap<String, String>,
    tensors: Vec<(String, String, Vec<u64>)>,
    parameters: u64,
}

const FIXTURE_FILES: &[(&str, &str)] = &[
    ("sd15", include_str!("../../tests/fixtures/sd/sd15.json")),
    ("sdxl", include_str!("../../tests/fixtures/sd/sdxl.json")),
    (
        "sd35-medium",
        include_str!("../../tests/fixtures/sd/sd35-medium.json"),
    ),
    (
        "flux1-dev",
        include_str!("../../tests/fixtures/sd/flux1-dev.json"),
    ),
    (
        "flux1-kontext",
        include_str!("../../tests/fixtures/sd/flux1-kontext.json"),
    ),
    (
        "chroma",
        include_str!("../../tests/fixtures/sd/chroma.json"),
    ),
    (
        "qwen-image",
        include_str!("../../tests/fixtures/sd/qwen-image.json"),
    ),
    (
        "qwen-image-edit-2509",
        include_str!("../../tests/fixtures/sd/qwen-image-edit-2509.json"),
    ),
    (
        "z-image-turbo",
        include_str!("../../tests/fixtures/sd/z-image-turbo.json"),
    ),
    (
        "flux2-klein-4b",
        include_str!("../../tests/fixtures/sd/flux2-klein-4b.json"),
    ),
    (
        "wan21-t2v-1.3b",
        include_str!("../../tests/fixtures/sd/wan21-t2v-1.3b.json"),
    ),
    (
        "wan21-i2v-14b",
        include_str!("../../tests/fixtures/sd/wan21-i2v-14b.json"),
    ),
    (
        "wan22-ti2v-5b",
        include_str!("../../tests/fixtures/sd/wan22-ti2v-5b.json"),
    ),
    (
        "wan22-t2v-a14b-high",
        include_str!("../../tests/fixtures/sd/wan22-t2v-a14b-high.json"),
    ),
    (
        "wan22-t2v-a14b-low",
        include_str!("../../tests/fixtures/sd/wan22-t2v-a14b-low.json"),
    ),
    (
        "hunyuan-video-15",
        include_str!("../../tests/fixtures/sd/hunyuan-video-15.json"),
    ),
    ("ltx23", include_str!("../../tests/fixtures/sd/ltx23.json")),
    (
        "sdxl-lora-pixel-art",
        include_str!("../../tests/fixtures/sd/sdxl-lora-pixel-art.json"),
    ),
    (
        "flux-lora-realism",
        include_str!("../../tests/fixtures/sd/flux-lora-realism.json"),
    ),
    (
        "lumina2",
        include_str!("../../tests/fixtures/sd/lumina2.json"),
    ),
    (
        "sd14-diffusers-unet",
        include_str!("../../tests/fixtures/sd/sd14-diffusers-unet.json"),
    ),
    (
        "sd14-diffusers-text-encoder",
        include_str!("../../tests/fixtures/sd/sd14-diffusers-text-encoder.json"),
    ),
    (
        "ae-flux1",
        include_str!("../../tests/fixtures/sd/ae-flux1.json"),
    ),
    (
        "clip-l",
        include_str!("../../tests/fixtures/sd/clip-l.json"),
    ),
    (
        "umt5-xxl-q5km",
        include_str!("../../tests/fixtures/sd/umt5-xxl-q5km.json"),
    ),
];

fn fixture_file(name: &str) -> FixtureFile {
    let (_, json) = FIXTURE_FILES
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no detection fixture {name}"));
    serde_json::from_str(json).unwrap_or_else(|e| panic!("fixture {name} does not read: {e}"))
}

/// The trimmed tensor table of a real file, with `parameters_hint` set to the whole file's
/// parameter count.
///
/// # Panics
/// For a name that is not a fixture.
pub fn fixture(name: &str) -> crate::sd::index::TensorIndex {
    let f = fixture_file(name);
    crate::sd::index::TensorIndex {
        format: f.format,
        tensors: f
            .tensors
            .into_iter()
            .map(|(name, dtype, shape)| crate::sd::index::TensorInfo { name, dtype, shape })
            .collect(),
        metadata: f.metadata,
        parameters_hint: Some(f.parameters),
    }
}

/// The pinned file the fixture was captured from.
///
/// # Panics
/// For a name that is not a fixture.
pub fn fixture_source(name: &str) -> FixtureSource {
    fixture_file(name).source
}

/// A GGUF index from a 0.1.x `fixtures/gguf/<name>-tensors.txt` (`"klein4b"`), in file order
/// with no metadata, as those files have none. The text files carry no types, so every
/// dtype is `"f16"`.
///
/// # Panics
/// For a name that is not one of `gguf::testing::TENSOR_FIXTURES`.
pub fn gguf_txt_fixture(name: &str) -> crate::sd::index::TensorIndex {
    crate::sd::index::TensorIndex {
        format: crate::sd::index::FileFormat::Gguf,
        tensors: crate::gguf::testing::shapes(&format!("{name}-tensors.txt"))
            .into_iter()
            .map(|t| crate::sd::index::TensorInfo {
                name: t.name,
                dtype: "f16".into(),
                shape: t.dims,
            })
            .collect(),
        metadata: Default::default(),
        parameters_hint: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: [&str; 25] = [
        "sd15",
        "sdxl",
        "sd35-medium",
        "flux1-dev",
        "flux1-kontext",
        "chroma",
        "qwen-image",
        "qwen-image-edit-2509",
        "z-image-turbo",
        "flux2-klein-4b",
        "wan21-t2v-1.3b",
        "wan21-i2v-14b",
        "wan22-ti2v-5b",
        "wan22-t2v-a14b-high",
        "wan22-t2v-a14b-low",
        "hunyuan-video-15",
        "ltx23",
        "sdxl-lora-pixel-art",
        "flux-lora-realism",
        "lumina2",
        "sd14-diffusers-unet",
        "sd14-diffusers-text-encoder",
        "ae-flux1",
        "clip-l",
        "umt5-xxl-q5km",
    ];

    #[test]
    fn a_0_1_x_text_table_becomes_a_gguf_index() {
        let ix = gguf_txt_fixture("klein4b");
        assert_eq!(ix.format, crate::sd::index::FileFormat::Gguf);
        assert_eq!(
            ix.tensors[0].name,
            "double_blocks.0.img_attn.norm.query_norm.scale"
        );
        assert_eq!(ix.tensors[2].shape, [3072, 3072]);
        assert!(ix.metadata.is_empty());
    }

    #[test]
    fn a_fixture_reports_the_whole_files_parameters() {
        assert_eq!(fixture("sd15").parameters(), 1_066_247_386);
    }

    #[test]
    fn every_fixture_names_its_pinned_source() {
        for name in FIXTURES {
            let s = fixture_source(name);
            assert!(
                s.revision.len() == 40 && s.revision.bytes().all(|b| b.is_ascii_hexdigit()),
                "{name}"
            );
            assert!(s.sha256.len() == 64 && s.size > 0, "{name}");
            assert!(!fixture(name).tensors.is_empty(), "{name}");
        }
        assert_eq!(
            fixture_source("lumina2").sha256,
            "736e7a524186c943568dfe688d87ce3556cfbd97f7026125065b0aa44c351f6a"
        );
    }
}
