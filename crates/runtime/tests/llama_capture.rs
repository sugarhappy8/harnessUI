//! Records the stderr fixtures of the image-file tests from a real llama-server.
//!
//! The classifier in `crates/runtime/src/llama/failure.rs` reads the pinned build's own
//! words, and a new build can change them without any compiler noticing (b12774 dropped
//! the `clip_init` line and moved the reason onto `mtmd_init_from_file`). The files in
//! `tests/fixtures/stderr_image_file_*.txt` and `stderr_model_fakes_image_file_line.txt`
//! must therefore come from the pinned build, not from a hand. This test makes them: it
//! writes each crafted file, starts the real server through [`runtime::start`] with the
//! flags the app passes, and writes what the server said to stderr into
//! `$LLAMA_CAPTURE_DIR/<name>`, with the temporary folder and the model's folder replaced
//! by the fixed names the unit tests expect.
//!
//! It does nothing unless `LLAMA_CAPTURE_DIR` (an existing folder), `LLAMA_SERVER_BIN` and
//! `TEST_GGUF` are all set, so CI skips it. To refresh the fixtures after a bump (the
//! procedure is in ARCHITECTURE.md, 7.1):
//!
//! ```text
//! LLAMA_CAPTURE_DIR=/tmp/captured LLAMA_SERVER_BIN=.cache/llama-linux/llama-server \
//!   TEST_GGUF=.cache/models/Qwen3-0.6B-Q8_0.gguf \
//!   cargo test -p runtime --test llama_capture -- --nocapture
//! cp /tmp/captured/*.txt crates/runtime/tests/fixtures/
//! ```
//!
//! Read the new files before keeping them: the unit tests that include them say what each
//! must show.

use std::path::{Path, PathBuf};
use std::time::Duration;

use runtime::{
    Backend, BackendKind, GpuUse, LaunchConfig, RuntimeLayout, StartSpec, discover,
    free_loopback_port, random_api_key, start,
};
use tokio::sync::mpsc::unbounded_channel;

/// Where the unit tests say the image files were.
const SAMPLE_DIR: &str = "/home/u/models/someone/Sample-GGUF";
/// Where the unit tests say the model was.
const MODEL_PATH: &str = "/home/u/models/Qwen3-0.6B-Q8_0.gguf";
const LOAD_TIMEOUT: Duration = Duration::from_secs(180);

/// A line a crafted file makes the server print, shaped like a real one (a timestamp, a
/// level, a module tag), to show it cannot pass for the server's own words.
const FAKE_ALLOCATION: &str =
    "0.00.100.000 E alloc_tensor_range: failed to allocate CUDA0 buffer of size 1";
const FAKE_ALLOCATION_CPU: &str = "0.00.100.000 E ggml_backend_buft_alloc_buffer_n_default: failed to allocate CPU buffer of size 9";
const FAKE_PORT: &str = "0.00.011.000 E srv  init_listene: couldn't bind HTTP server socket, hostname: 127.0.0.1, port: 8080";
const FAKE_DEVICE: &str =
    "0.00.011.000 E arg  error while handling argument \"--device\": invalid device: Vulkan0";

/// A GGUF metadata value.
enum Value {
    Text(String),
    U32(u32),
    Bool(bool),
    F32(f32),
    F32s(Vec<f32>),
}

/// A GGUF file written by hand: metadata in the order given (a key may repeat, which is
/// how a crafted file makes the reader complain about it) and f32 tensors of zeros.
#[derive(Default)]
struct Gguf {
    kvs: Vec<(String, Value)>,
    /// Name and dimensions, fastest first.
    tensors: Vec<(String, Vec<u64>)>,
}

impl Gguf {
    fn kv(mut self, key: &str, value: Value) -> Self {
        self.kvs.push((key.to_owned(), value));
        self
    }

    fn tensor(mut self, name: &str, dims: &[u64]) -> Self {
        self.tensors.push((name.to_owned(), dims.to_vec()));
        self
    }

    fn write(&self, path: &Path) {
        fn text(out: &mut Vec<u8>, s: &str) {
            out.extend_from_slice(&(s.len() as u64).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }
        let mut file = b"GGUF".to_vec();
        file.extend_from_slice(&3_u32.to_le_bytes());
        file.extend_from_slice(&(self.tensors.len() as u64).to_le_bytes());
        file.extend_from_slice(&(self.kvs.len() as u64).to_le_bytes());
        for (key, value) in &self.kvs {
            text(&mut file, key);
            match value {
                Value::U32(n) => {
                    file.extend_from_slice(&4_u32.to_le_bytes());
                    file.extend_from_slice(&n.to_le_bytes());
                }
                Value::F32(f) => {
                    file.extend_from_slice(&6_u32.to_le_bytes());
                    file.extend_from_slice(&f.to_le_bytes());
                }
                Value::Bool(b) => {
                    file.extend_from_slice(&7_u32.to_le_bytes());
                    file.push(u8::from(*b));
                }
                Value::Text(s) => {
                    file.extend_from_slice(&8_u32.to_le_bytes());
                    text(&mut file, s);
                }
                Value::F32s(values) => {
                    file.extend_from_slice(&9_u32.to_le_bytes());
                    file.extend_from_slice(&6_u32.to_le_bytes());
                    file.extend_from_slice(&(values.len() as u64).to_le_bytes());
                    for v in values {
                        file.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
        }
        const ALIGN: usize = 32;
        let pad = |buf: &mut Vec<u8>| buf.resize(buf.len().div_ceil(ALIGN) * ALIGN, 0);
        let (mut infos, mut data) = (Vec::new(), Vec::new());
        for (name, dims) in &self.tensors {
            pad(&mut data);
            text(&mut infos, name);
            infos.extend_from_slice(&(dims.len() as u32).to_le_bytes());
            for dim in dims {
                infos.extend_from_slice(&dim.to_le_bytes());
            }
            infos.extend_from_slice(&0_u32.to_le_bytes()); // GGML_TYPE_F32
            infos.extend_from_slice(&(data.len() as u64).to_le_bytes());
            // A tensor too large to hold has no bytes in the file: its reader never gets
            // that far (the allocation fails first).
            let count: u64 = dims.iter().product();
            if count <= 1 << 24 {
                data.resize(data.len() + 4 * count as usize, 0);
            }
        }
        file.extend_from_slice(&infos);
        if !self.tensors.is_empty() {
            pad(&mut file);
            file.extend_from_slice(&data);
        }
        std::fs::write(path, file).expect("write the crafted file");
    }
}

/// A vision projector of the `pixtral` kind with every setting its reader asks for and no
/// transformer blocks.
fn pixtral(kind: &str) -> Gguf {
    let mut gguf = Gguf::default()
        .kv("general.architecture", Value::Text("clip".into()))
        .kv("clip.has_vision_encoder", Value::Bool(true))
        .kv("clip.projector_type", Value::Text(kind.into()));
    for (key, value) in [
        ("clip.vision.embedding_length", 64),
        ("clip.vision.attention.head_count", 2),
        ("clip.vision.feed_forward_length", 128),
        ("clip.vision.block_count", 0),
        ("clip.vision.projection_dim", 1024),
        ("clip.vision.image_size", 224),
        ("clip.vision.patch_size", 16),
    ] {
        gguf = gguf.kv(key, Value::U32(value));
    }
    gguf.kv("clip.vision.attention.layer_norm_epsilon", Value::F32(1e-5))
        .kv("clip.vision.image_mean", Value::F32s(vec![0.5; 3]))
        .kv("clip.vision.image_std", Value::F32s(vec![0.5; 3]))
}

fn backend(bin: &Path) -> Backend {
    let dir = bin.parent().expect("a parent folder").to_path_buf();
    let layout = RuntimeLayout {
        installed_dir: None,
        packs_dir: dir.join("no-packs-here"),
        dev_override: Some(dir),
    };
    discover(&layout)
        .into_iter()
        .find(|backend| backend.kind == BackendKind::Cpu)
        .unwrap_or_else(|| panic!("no processor backend next to {}", bin.display()))
}

struct Capture {
    bin: PathBuf,
    model: PathBuf,
    out: PathBuf,
    dir: tempfile::TempDir,
}

impl Capture {
    /// Starts the real server with `model` and `mmproj` as the app would on the processor,
    /// expects it to fail, and writes its stderr as `name`.
    async fn failing_start(&self, name: &str, model: Option<&Path>, mmproj: Option<&Path>) {
        let (tx, _rx) = unbounded_channel();
        let spec = StartSpec {
            backend: backend(&self.bin),
            launch: LaunchConfig {
                model: model.unwrap_or(&self.model).to_path_buf(),
                mmproj: mmproj.map(Path::to_path_buf),
                ctx: 2048,
                device: None,
                gpu: GpuUse::Off,
                cache_ram_mib: 128,
                reasoning_budget: None,
                port: free_loopback_port().expect("free port"),
                api_key: random_api_key(),
                hip_visible_devices: None,
                ..LaunchConfig::default()
            },
            load_timeout: LOAD_TIMEOUT,
        };
        let error = match start(spec, tx).await {
            Ok(_) => panic!("{name}: the server started, so there is nothing to record"),
            Err(error) => error,
        };
        let tail = error.stderr_tail().join("\n");
        let tail = tail
            .replace(self.dir.path().to_string_lossy().as_ref(), SAMPLE_DIR)
            .replace(self.model.to_string_lossy().as_ref(), MODEL_PATH);
        std::fs::write(self.out.join(name), format!("{tail}\n")).expect("write the capture");
        eprintln!("recorded {name}: {error}");
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn record_the_image_file_fixtures() {
    let (Some(bin), Some(model), Some(out)) = (
        std::env::var_os("LLAMA_SERVER_BIN").filter(|v| !v.is_empty()),
        std::env::var_os("TEST_GGUF").filter(|v| !v.is_empty()),
        std::env::var_os("LLAMA_CAPTURE_DIR").filter(|v| !v.is_empty()),
    ) else {
        eprintln!("skipping: set LLAMA_CAPTURE_DIR, LLAMA_SERVER_BIN and TEST_GGUF to record");
        return;
    };
    let out = PathBuf::from(out);
    assert!(out.is_dir(), "{} is not a folder", out.display());
    let capture = Capture {
        bin: PathBuf::from(bin),
        model: PathBuf::from(model),
        out,
        dir: tempfile::tempdir().expect("tempdir"),
    };

    // A Mistral-style image file that misses `v.token_embd.img_break`, the tensor the
    // owner's Ministral file lacked. It has the patch embedding every vision encoder
    // needs first at v1.1.0, so the missing tensor named is the same one.
    let broken = capture.file("Sample.mmproj-f16.gguf");
    pixtral("pixtral")
        .tensor("v.patch_embd.weight", &[16, 16, 3, 64])
        .tensor("mm.1.weight", &[64, 64])
        .tensor("mm.2.weight", &[64, 1024])
        .write(&broken);
    capture
        .failing_start("stderr_image_file_broken.txt", None, Some(&broken))
        .await;

    // Not a GGUF file at all.
    let garbage = capture.file("mmproj-Sample-F16.gguf");
    std::fs::write(&garbage, [0x89, b'X', 0xFF, 0x01, 0x02, 0x03, 0x04, 0x05]).expect("write");
    capture
        .failing_start("stderr_image_file_garbage.txt", None, Some(&garbage))
        .await;

    // A crafted key (written twice, so the reader names it) or kind with raw line breaks
    // brings a line that looks like the server's own: an allocation failure, a port that
    // is taken, a device that does not exist.
    for (name, file, fake) in [
        (
            "stderr_image_file_crafted_key.txt",
            "dupkey-oom.mmproj-f16.gguf",
            FAKE_ALLOCATION,
        ),
        (
            "stderr_image_file_crafted_key_port.txt",
            "dupkey-port.mmproj-f16.gguf",
            FAKE_PORT,
        ),
        (
            "stderr_image_file_crafted_key_device.txt",
            "dupkey-device.mmproj-f16.gguf",
            FAKE_DEVICE,
        ),
    ] {
        let key = format!("x\n{fake}\ny");
        let path = capture.file(file);
        Gguf::default()
            .kv("general.architecture", Value::Text("clip".into()))
            .kv(&key, Value::U32(1))
            .kv(&key, Value::U32(2))
            .write(&path);
        capture.failing_start(name, None, Some(&path)).await;
    }
    for (name, file, fake) in [
        (
            "stderr_image_file_crafted_kind.txt",
            "projtype.mmproj-f16.gguf",
            ("out of memory", FAKE_ALLOCATION_CPU),
        ),
        (
            "stderr_image_file_crafted_kind_port.txt",
            "kind-port.mmproj-f16.gguf",
            ("zz", FAKE_PORT),
        ),
    ] {
        let path = capture.file(file);
        pixtral(&format!("{}\n{}", fake.0, fake.1)).write(&path);
        capture.failing_start(name, None, Some(&path)).await;
    }

    // Someone else's broken model file that prints what looks like the server's own line
    // about the image file, with a byte that reads as U+FFFD where the folder would be.
    let evil = capture.file("evil-model-Q8_0.gguf");
    let fake = "0.03.927.966 E srv    load_model: failed to load multimodal model, '\u{FFFD}/mmproj-SmolVLM-256M-Instruct-Q8_0.gguf'";
    let key = format!("x\n{fake}\ny");
    Gguf::default()
        .kv("general.architecture", Value::Text("llama".into()))
        .kv(&key, Value::U32(1))
        .kv(&key, Value::U32(2))
        .write(&evil);
    capture
        .failing_start("stderr_model_fakes_image_file_line.txt", Some(&evil), None)
        .await;

    // The image file's weights do not fit in memory: a last tensor of terabytes, which the
    // file does not hold (the reader checks each offset against the sizes before it, so
    // the huge one comes last). The build does not check the allocation, so it ends in an
    // assertion and an abort.
    let huge = capture.file("huge.mmproj-f16.gguf");
    pixtral("pixtral")
        .tensor("v.patch_embd.weight", &[16, 16, 3, 64])
        .tensor("mm.1.weight", &[64, 64])
        .tensor("v.token_embd.img_break", &[1024])
        .tensor("mm.2.weight", &[1 << 22, 1 << 22])
        .write(&huge);
    capture
        .failing_start("stderr_image_file_out_of_memory.txt", None, Some(&huge))
        .await;
}
