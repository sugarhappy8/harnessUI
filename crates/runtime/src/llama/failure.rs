//! Classifying why llama-server failed to load (ARCHITECTURE.md 7.4 and 15).
//!
//! The signatures come from llama.cpp's source at the pinned build and from failures
//! captured from the pinned binary (`tests/fixtures/stderr_*.txt`). Log wording can change
//! in any release; the contract tests re-check it on every bump.

use serde::{Deserialize, Serialize};

use super::backend::BackendKind;
use super::devices::{STATUS_DLL_NOT_FOUND, STATUS_INVALID_IMAGE_FORMAT};

/// Why a load failed, in terms the UI can explain.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "code", content = "device", rename_all = "snake_case")]
pub enum LoadFailure {
    /// The model's weights did not fit in the device's (or the host's) memory.
    OutOfMemory,
    /// The device named with `--device` does not exist in this backend.
    InvalidDevice(String),
    /// A runtime library is missing or broken (Windows loader failure, no output at all).
    MissingRuntimeDll,
    /// The chosen port was taken.
    PortInUse,
    /// The model file is missing, truncated or not a GGUF file.
    ModelFileInvalid,
    /// The weights loaded but the context (KV cache, compute buffers) did not fit.
    ContextTooLarge,
    /// 0.0.7: the model and its context loaded, then the image file given with `--mmproj`
    /// (the projector) could not be read: a damaged file, one made for another model, or
    /// one this build cannot use. JSON `{"code":"image_file_invalid"}`. It fails the same
    /// on every backend; the supervisor loads the model again without it
    /// ([`LoadStage::RetryingWithoutImageFile`](super::stage::LoadStage)).
    ImageFileInvalid,
    /// None of the known signatures matched.
    Unknown,
}

impl LoadFailure {
    /// A short plain-language explanation.
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::OutOfMemory => "This model needs more memory than is free.",
            Self::InvalidDevice(_) => {
                "The selected graphics device isn't available to this backend."
            }
            Self::MissingRuntimeDll => "A file the model runner needs is missing or damaged.",
            Self::PortInUse => "Another program is using the network port the model runner needs.",
            Self::ModelFileInvalid => {
                "The model file couldn't be read. It may be incomplete, damaged, not a GGUF model, or a kind of model this version can't run."
            }
            Self::ContextTooLarge => {
                "The model loaded, but there isn't enough memory left for how much it should remember."
            }
            Self::ImageFileInvalid => "The image file next to this model couldn't be read.",
            Self::Unknown => "The model runner stopped while loading the model.",
        }
    }

    /// What the user can do about it.
    ///
    /// For the memory failures it names only what is left for the user: before one is
    /// shown, the supervisor has already put less of the model on the graphics card and
    /// tried the processor where it could (0.0.6, see
    /// [`Supervisor`](super::server::Supervisor)).
    pub fn next_step(&self) -> &'static str {
        match self {
            Self::OutOfMemory => {
                "Close other programs or pick a smaller download. Lowering How much it can remember in Model settings also helps."
            }
            Self::InvalidDevice(_) => {
                "Choose another device or backend in Settings, or switch to CPU for now."
            }
            Self::MissingRuntimeDll => {
                "Reinstall the app. For a downloaded backend, remove it in Settings and download it again."
            }
            Self::PortInUse => "Restart the app.",
            Self::ModelFileInvalid => "Download the model again, or choose a different file.",
            Self::ContextTooLarge => {
                "In Model settings, lower How much it can remember, or turn on Save memory if it is off."
            }
            Self::ImageFileInvalid => {
                "The model can load without it. To use images, download its image file again in Models → Installed."
            }
            // By the time an unknown failure is shown, the supervisor has already tried the
            // next backends down to the processor (0.0.6), so another backend is no advice:
            // see [`Self::next_step_on`] for a load that ended on a graphics card.
            Self::Unknown => {
                "Copy the details and check the logs. If other models load, try a different download of this model."
            }
        }
    }

    /// [`Self::next_step`] for a load that ended on `backend` (0.0.7). An unknown failure
    /// that ended on a graphics backend never reached the processor (the supervisor steps
    /// down to it whenever its build passed its check), so choosing it may still help; on
    /// the processor no other backend would. Every other failure says the same wherever it
    /// ended: a backend change cannot fix it, or the app already tried one.
    pub fn next_step_on(&self, backend: BackendKind) -> &'static str {
        match self {
            Self::Unknown if backend != BackendKind::Cpu => {
                "Copy the details and check the logs, or choose Processor only in Settings → Local models."
            }
            _ => self.next_step(),
        }
    }

    /// Whether trying the next backend down could help (ARCHITECTURE.md 7.4, "Stepping
    /// down"). A broken model file, a port problem and an image file that cannot be read
    /// fail the same way on any backend.
    pub fn may_succeed_on_another_backend(&self) -> bool {
        !matches!(
            self,
            Self::ModelFileInvalid | Self::PortInUse | Self::ImageFileInvalid
        )
    }

    /// Whether Save memory (flash attention with a q8_0 conversation memory) may have
    /// caused the failure, so a retry with it off is worth one attempt (0.0.6 D2c).
    ///
    /// A device or driver that cannot do it shows as a failed context (llama.cpp refuses a
    /// q8_0 memory without flash attention) or as a crash. A failure to allocate the
    /// weights ([`LoadFailure::OutOfMemory`]) happens before the context exists, so Save
    /// memory cannot cause it; turning it off would only need more memory. The supervisor
    /// asks this only while a graphics card is in use; anywhere else it retries without
    /// Save memory only when the log shows the model refusing it ([`save_memory_refused`]).
    pub fn save_memory_may_be_the_cause(&self) -> bool {
        matches!(self, Self::ContextTooLarge | Self::Unknown)
    }

    /// Whether a device ran out of memory, for the weights or for the context, so putting
    /// less of the model on the graphics card may help.
    pub fn is_out_of_memory(&self) -> bool {
        matches!(self, Self::OutOfMemory | Self::ContextTooLarge)
    }
}

impl std::fmt::Display for LoadFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.user_message())
    }
}

/// Lines that only appear when creating the context (KV cache, compute buffers) failed,
/// after the weights loaded.
const CONTEXT_SIGNATURES: &[&str] = &[
    "failed to create context",
    "failed to create_context",
    "failed to initialize the context",
    "buffer for kv cache",
    "compute pp buffers",
];

/// Allocation failures on any backend.
const ALLOCATION_SIGNATURES: &[&str] = &[
    "cudamalloc failed",
    "out of memory",
    "outofdevicememory",
    "failed to allocate",
    "unable to allocate",
    "insufficient memory",
];

/// The model file itself could not be opened or parsed, which fails the same way on every
/// backend. Every GGUF parse error is printed by a `gguf_init_from_*` function; the others
/// come from llama.cpp's model loader (all present as strings in the pinned build).
///
/// The generic wrappers `error loading model: ...` and `failed to load model` are **not**
/// here: llama.cpp prints them for any exception thrown while loading, including a GPU
/// backend's own errors, and treating those as a broken file would stop the step-down to
/// the next backend (ARCHITECTURE.md 7.4). Without a file signature they classify as
/// [`LoadFailure::Unknown`].
const MODEL_FILE_SIGNATURES: &[&str] = &[
    "failed to open gguf file",
    "invalid magic characters",
    "gguf_init_from",
    "not within the file bounds",
    "model is corrupted or incomplete",
    "unknown model architecture",
];

/// The image file (`--mmproj`) could not be read after the model and its context loaded
/// (0.0.7). At the pinned build (b12110, `tools/server/server-context.cpp`,
/// `tools/mtmd/mtmd.cpp`, `tools/mtmd/clip.cpp`) a failed `mtmd_init_from_file` prints
/// "mtmd_init_from_file: error: …", the server then "failed to load multimodal model,
/// '<file>'" and exits; an exception inside `clip_init` (a missing tensor, a key, a
/// mismatch) is printed first as "clip_init: failed to load model '<file>': …" (captured in
/// `tests/fixtures/stderr_image_file_broken.txt`; an older build printed the exception on
/// the `mtmd_init_from_file` line itself, `stderr_image_file_owner_windows.txt`).
///
/// Only lines that end a load are here. "mtmd_get_memory_usage: error: …" and "Failed to
/// load CLIP model" also appear when the server measures the file before the model loads,
/// and the load goes on after them (the model may still fail for another reason, or the
/// file may still load); `clip_init` is silenced while it measures.
const IMAGE_FILE_SIGNATURES: &[&str] = &[
    "failed to load multimodal model",
    "mtmd_init_from_file: error",
    "clip_init: failed to load model",
];

/// Classifies a failed load from the last lines of stderr and the exit code.
///
/// The order matters: a context or allocation failure comes first, also while the image
/// file loads (less of the model on the graphics card or another backend may make room
/// for it); then the image file, before the model file, because a damaged image file is
/// read with the same GGUF reader whose errors name a broken model file, and the server
/// reads the image file only after the model loaded.
pub fn classify(stderr_tail: &[String], exit_code: Option<i32>) -> LoadFailure {
    if matches!(exit_code, Some(code) if code == STATUS_DLL_NOT_FOUND || code == STATUS_INVALID_IMAGE_FORMAT)
    {
        return LoadFailure::MissingRuntimeDll;
    }
    let lower: Vec<String> = stderr_tail
        .iter()
        .map(|line| line.to_ascii_lowercase())
        .collect();
    let any = |needles: &[&str]| {
        lower
            .iter()
            .any(|line| needles.iter().any(|needle| line.contains(needle)))
    };

    if any(&["couldn't bind http server socket"]) {
        return LoadFailure::PortInUse;
    }
    if let Some(device) = stderr_tail.iter().find_map(|line| invalid_device(line)) {
        return LoadFailure::InvalidDevice(device);
    }
    if any(CONTEXT_SIGNATURES) {
        return LoadFailure::ContextTooLarge;
    }
    if any(ALLOCATION_SIGNATURES) {
        return LoadFailure::OutOfMemory;
    }
    if any(IMAGE_FILE_SIGNATURES) {
        return LoadFailure::ImageFileInvalid;
    }
    if any(MODEL_FILE_SIGNATURES) {
        return LoadFailure::ModelFileInvalid;
    }
    LoadFailure::Unknown
}

/// Lines the pinned build logs when the **model** cannot take Save memory's settings
/// (`-fa on -ctk q8_0 -ctv q8_0`), on every backend, the processor included
/// (`llama_init_from_model` and `llama_context` in `src/llama-context.cpp` at b12110):
/// - a q8_0 block of 32 values does not divide the model's attention head size (80 in
///   some model families): "K cache type q8_0 with block size 32 does not divide
///   n_embd_head_k=80", and the same for V;
/// - the model cannot use flash attention, which a quantized V memory needs: "flash_attn
///   is not compatible with Grok - forcing off" followed by "quantized V cache requires
///   flash_attn to be enabled", or "quantized V cache was requested, but this requires
///   Flash Attention".
///
/// Each is followed by "failed to create context", so [`classify`] still says
/// [`LoadFailure::ContextTooLarge`]. Not here: "enabling flash_attn since it is required
/// for quantized V cache", which only `-fa auto` prints.
const SAVE_MEMORY_REFUSALS: &[&str] = &[
    "does not divide n_embd_head",
    "quantized v cache requires flash_attn",
    "quantized v cache was requested, but this requires flash attention",
];

/// Whether the last lines of a failed load show the model refusing Save memory
/// ([`SAVE_MEMORY_REFUSALS`]). Then a retry without it can load the model on any backend,
/// while for a plain memory failure on the processor it would only need more memory
/// (0.0.6 D2c, see [`Supervisor`](super::server::Supervisor)).
pub fn save_memory_refused(stderr_tail: &[String]) -> bool {
    stderr_tail.iter().any(|line| {
        let lower = line.to_ascii_lowercase();
        SAVE_MEMORY_REFUSALS
            .iter()
            .any(|needle| lower.contains(needle))
    })
}

/// Extracts `X` from `... invalid device: X`.
fn invalid_device(line: &str) -> Option<String> {
    let at = line.find("invalid device: ")?;
    let word = line[at + "invalid device: ".len()..]
        .split_whitespace()
        .next()
        .unwrap_or_default();
    let name = word.trim_matches(['"', '\'', ',', '.']);
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(bytes: &[u8]) -> Vec<String> {
        String::from_utf8_lossy(bytes)
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    #[test]
    fn captured_missing_model_is_model_file_invalid() {
        let tail = fixture(include_bytes!(
            "../../tests/fixtures/stderr_missing_model.txt"
        ));
        assert_eq!(classify(&tail, Some(1)), LoadFailure::ModelFileInvalid);
    }

    #[test]
    fn captured_garbage_model_is_model_file_invalid() {
        // Contains raw bytes from the file, which are not valid UTF-8.
        let tail = fixture(include_bytes!(
            "../../tests/fixtures/stderr_invalid_magic.txt"
        ));
        assert_eq!(classify(&tail, Some(1)), LoadFailure::ModelFileInvalid);
    }

    #[test]
    fn captured_invalid_device() {
        let tail = fixture(include_bytes!(
            "../../tests/fixtures/stderr_invalid_device.txt"
        ));
        assert_eq!(
            classify(&tail, Some(1)),
            LoadFailure::InvalidDevice("Vulkan0".into())
        );
    }

    #[test]
    fn captured_port_in_use() {
        let tail = fixture(include_bytes!(
            "../../tests/fixtures/stderr_port_in_use.txt"
        ));
        assert_eq!(classify(&tail, Some(1)), LoadFailure::PortInUse);
    }

    #[test]
    fn captured_oversized_context() {
        let tail = fixture(include_bytes!(
            "../../tests/fixtures/stderr_context_too_large.txt"
        ));
        assert_eq!(classify(&tail, Some(1)), LoadFailure::ContextTooLarge);
    }

    #[test]
    fn weight_allocation_failures_are_out_of_memory() {
        // Signatures from llama.cpp source (CUDA, Metal, generic); not captured on hardware.
        let cuda = lines(
            "ggml_backend_cuda_buffer_type_alloc_buffer: allocating 9000.00 MiB on device 0: cudaMalloc failed: out of memory\n\
             alloc_tensor_range: failed to allocate CUDA0 buffer of size 9437184000\n\
             llama_model_load: error loading model: unable to allocate CUDA0 buffer\n\
             llama_model_load_from_file_impl: failed to load model\n\
             srv    load_model: failed to load model, 'm.gguf'\n\
             srv  server_run_i: exiting due to model loading error",
        );
        assert_eq!(classify(&cuda, Some(1)), LoadFailure::OutOfMemory);
        let metal =
            lines("ggml_metal_buffer_init: error: failed to allocate buffer, size = 12000.00 MiB");
        assert_eq!(classify(&metal, Some(1)), LoadFailure::OutOfMemory);
        let vulkan = lines(
            "ggml_vulkan: Device memory allocation of size 1 failed.\nvk::Device::allocateMemory: ErrorOutOfDeviceMemory",
        );
        assert_eq!(classify(&vulkan, Some(1)), LoadFailure::OutOfMemory);
    }

    #[test]
    fn fit_params_warning_alone_is_not_out_of_memory() {
        let tail = lines(
            "E common_fit_params: encountered an error while trying to fit params to free device memory: failed to load model",
        );
        assert_eq!(classify(&tail, Some(1)), LoadFailure::Unknown);
    }

    #[test]
    fn a_backend_error_while_loading_is_not_a_broken_file() {
        // A GPU backend that throws during load is wrapped in the same generic lines as a
        // broken file. Without a file signature it must stay Unknown, so the supervisor
        // still steps down to the next backend.
        let tail = lines(
            "E llama_model_load: error loading model: vk::Queue::submit: ErrorDeviceLost\n\
             E llama_model_load_from_file_impl: failed to load model\n\
             E srv    load_model: failed to load model, 'm.gguf'\n\
             E srv  server_run_i: exiting due to model loading error",
        );
        let failure = classify(&tail, Some(1));
        assert_eq!(failure, LoadFailure::Unknown);
        assert!(failure.may_succeed_on_another_backend());
    }

    #[test]
    fn file_signatures_from_the_model_loader_are_model_file_invalid() {
        for line in [
            "E llama_model_load: error loading model: tensor 'blk.0.attn_q.weight' data is not within the file bounds, model is corrupted or incomplete",
            "E llama_model_load: error loading model: error loading model architecture: unknown model architecture: 'newarch'",
            "E gguf_init_from_file_impl: failed to read header",
            "E gguf_init_from_reader: failed to read tensor info",
        ] {
            assert_eq!(
                classify(&lines(line), Some(1)),
                LoadFailure::ModelFileInvalid,
                "{line}"
            );
        }
    }

    /// 0.0.7: the image file failed after the model loaded, captured from the pinned build
    /// with an image file that misses a tensor its kind needs (`v.token_embd.img_break`,
    /// as in the owner's report) and with one that is not a GGUF file at all. The second
    /// log also holds the GGUF reader's "invalid magic characters", which alone names a
    /// broken model file: here it is about the image file, and the model is not blamed.
    #[test]
    fn captured_broken_image_files_are_image_file_invalid() {
        for tail in [
            fixture(include_bytes!(
                "../../tests/fixtures/stderr_image_file_broken.txt"
            )),
            fixture(include_bytes!(
                "../../tests/fixtures/stderr_image_file_garbage.txt"
            )),
        ] {
            let failure = classify(&tail, Some(1));
            assert_eq!(failure, LoadFailure::ImageFileInvalid, "{tail:?}");
            assert!(!failure.may_succeed_on_another_backend());
            assert!(!failure.save_memory_may_be_the_cause());
            assert!(!failure.is_out_of_memory());
        }
    }

    /// The owner's log (Windows, every backend failing the same way): the exception is on
    /// the `mtmd_init_from_file` line itself, without a `clip_init` line. The user folder is
    /// replaced and the folder the report left out is the one downloads use.
    #[test]
    fn the_owners_log_is_image_file_invalid() {
        let tail = fixture(include_bytes!(
            "../../tests/fixtures/stderr_image_file_owner_windows.txt"
        ));
        assert_eq!(classify(&tail, Some(1)), LoadFailure::ImageFileInvalid);
    }

    /// The later attempt in the owner's report: `clip_init` names the missing tensor, then
    /// `mtmd_init_from_file` says the file could not be loaded. Each line that ends a load
    /// counts on its own.
    #[test]
    fn the_clip_init_variant_is_image_file_invalid() {
        let path = r"C:\Users\me\harnessUI\models\x\M.mmproj-f16.gguf";
        let both = lines(&format!(
            "E clip_init: failed to load model '{path}': operator(): unable to find tensor v.token_embd.img_break\n\
             E mtmd_init_from_file: error: Failed to load CLIP model from {path}\n\
             E srv    load_model: failed to load multimodal model, '{path}'\n\
             E srv  server_run_i: exiting due to model loading error"
        ));
        assert_eq!(classify(&both, Some(1)), LoadFailure::ImageFileInvalid);
        for line in [
            format!(
                "E clip_init: failed to load model '{path}': operator(): unable to find tensor v.token_embd.img_break"
            ),
            format!("E mtmd_init_from_file: error: Failed to load CLIP model from {path}"),
            format!("E srv    load_model: failed to load multimodal model, '{path}'"),
        ] {
            assert_eq!(
                classify(&lines(&line), Some(1)),
                LoadFailure::ImageFileInvalid,
                "{line}"
            );
        }
    }

    /// Running out of memory while the image file loads is still running out of memory:
    /// less of the model on the graphics card, or another backend, may make room for it.
    #[test]
    fn running_out_of_memory_while_the_image_file_loads_is_out_of_memory() {
        let path = "/m/mmproj-F16.gguf";
        let tail = |alloc: &str| {
            lines(&format!(
                "I srv    load_model: loading model '/m/model.gguf'\n\
                 {alloc}\n\
                 E clip_init: failed to load model '{path}': load_tensors: failed to allocate the weights\n\
                 E mtmd_init_from_file: error: Failed to load CLIP model from {path}\n\
                 E srv    load_model: failed to load multimodal model, '{path}'\n\
                 E srv  server_run_i: exiting due to model loading error"
            ))
        };
        for alloc in [
            "E alloc_tensor_range: failed to allocate Vulkan0 buffer of size 851443712",
            "E ggml_vulkan: Device memory allocation of size 851443712 failed.\nE vk::Device::allocateMemory: ErrorOutOfDeviceMemory",
            "E ggml_backend_cuda_buffer_type_alloc_buffer: allocating 812.00 MiB on device 0: cudaMalloc failed: out of memory",
            "E ggml_metal_buffer_init: error: failed to allocate buffer, size = 812.00 MiB",
        ] {
            let failure = classify(&tail(alloc), Some(1));
            assert_eq!(failure, LoadFailure::OutOfMemory, "{alloc}");
            assert!(failure.may_succeed_on_another_backend());
        }
    }

    /// The server measures the image file before the model loads and goes on when that
    /// fails: such a line alone never makes the failure the image file's. What failed after
    /// it decides.
    #[test]
    fn measuring_the_image_file_is_not_a_failure_of_it() {
        let measured = "E mtmd_get_memory_usage: error: Failed to load CLIP model from /m/mmproj.gguf\n\
                        E mtmd_get_cap_from_file: failed to get capabilities from file '/m/mmproj.gguf': clip_model_loader: failed to load CLIP model from /m/mmproj.gguf. Does this file exist?\n\
                        E srv    load_model: [mtmd] failed to get memory usage of mmproj";
        assert_eq!(classify(&lines(measured), Some(1)), LoadFailure::Unknown);
        let weights = lines(&format!(
            "{measured}\nE alloc_tensor_range: failed to allocate CUDA0 buffer of size 9437184000\n\
             E llama_model_load_from_file_impl: failed to load model"
        ));
        assert_eq!(classify(&weights, Some(1)), LoadFailure::OutOfMemory);
        let model = lines(&format!(
            "{measured}\nE gguf_init_from_file_impl: failed to read header\n\
             E llama_model_load_from_file_impl: failed to load model"
        ));
        assert_eq!(classify(&model, Some(1)), LoadFailure::ModelFileInvalid);
        let context = lines(&format!(
            "{measured}\nE common_init_result: failed to create context with model 'm.gguf'"
        ));
        assert_eq!(classify(&context, Some(1)), LoadFailure::ContextTooLarge);
    }

    /// Logs that say nothing about an image file stay as they were.
    #[test]
    fn logs_without_an_image_file_are_unchanged() {
        for (name, bytes, want) in [
            (
                "missing model",
                &include_bytes!("../../tests/fixtures/stderr_missing_model.txt")[..],
                LoadFailure::ModelFileInvalid,
            ),
            (
                "invalid magic",
                &include_bytes!("../../tests/fixtures/stderr_invalid_magic.txt")[..],
                LoadFailure::ModelFileInvalid,
            ),
            (
                "context too large",
                &include_bytes!("../../tests/fixtures/stderr_context_too_large.txt")[..],
                LoadFailure::ContextTooLarge,
            ),
            (
                "port in use",
                &include_bytes!("../../tests/fixtures/stderr_port_in_use.txt")[..],
                LoadFailure::PortInUse,
            ),
        ] {
            assert_eq!(classify(&fixture(bytes), Some(1)), want, "{name}");
        }
        let unrelated = lines(
            "I srv    load_model: loading model '/m/model.gguf'\n\
             I srv    load_model: loaded multimodal model, '/m/mmproj.gguf'\n\
             E llama_model_load: error loading model: vk::Queue::submit: ErrorDeviceLost",
        );
        assert_eq!(classify(&unrelated, Some(1)), LoadFailure::Unknown);
    }

    /// An unknown failure is shown after the supervisor tried every backend it could, down
    /// to the processor: its advice names no other backend. Only a load that ended on a
    /// graphics backend (the processor was not reached) still suggests Processor only, in
    /// the words of Settings. Other failures say the same wherever they ended.
    #[test]
    fn the_unknown_advice_names_the_processor_only_where_it_can_help() {
        let unknown = LoadFailure::Unknown;
        assert_eq!(
            unknown.next_step(),
            "Copy the details and check the logs. If other models load, try a different download of this model."
        );
        assert_eq!(unknown.next_step_on(BackendKind::Cpu), unknown.next_step());
        for backend in [
            BackendKind::Vulkan,
            BackendKind::Cuda,
            BackendKind::Rocm,
            BackendKind::Metal,
        ] {
            assert_eq!(
                unknown.next_step_on(backend),
                "Copy the details and check the logs, or choose Processor only in Settings → Local models.",
                "{backend}"
            );
        }
        for failure in [
            LoadFailure::OutOfMemory,
            LoadFailure::InvalidDevice("Vulkan0".into()),
            LoadFailure::MissingRuntimeDll,
            LoadFailure::PortInUse,
            LoadFailure::ModelFileInvalid,
            LoadFailure::ContextTooLarge,
            LoadFailure::ImageFileInvalid,
        ] {
            for backend in [BackendKind::Cpu, BackendKind::Vulkan] {
                assert_eq!(
                    failure.next_step_on(backend),
                    failure.next_step(),
                    "{failure:?}"
                );
            }
        }
        // Plain words: no "CPU", "backend" or "mmproj" in what the user reads.
        for failure in [LoadFailure::Unknown, LoadFailure::ImageFileInvalid] {
            for backend in [BackendKind::Cpu, BackendKind::Vulkan] {
                let text = format!(
                    "{} {}",
                    failure.user_message(),
                    failure.next_step_on(backend)
                );
                for jargon in ["CPU", "backend", "mmproj", "projector", "CLIP", "!"] {
                    assert!(!text.contains(jargon), "{jargon:?} in {text:?}");
                }
            }
        }
        assert_eq!(
            LoadFailure::ImageFileInvalid.user_message(),
            "The image file next to this model couldn't be read."
        );
    }

    #[test]
    fn windows_loader_exit_codes() {
        assert_eq!(
            classify(&[], Some(-1_073_741_515)),
            LoadFailure::MissingRuntimeDll
        );
        assert_eq!(
            classify(&[], Some(-1_073_741_701)),
            LoadFailure::MissingRuntimeDll
        );
        assert_eq!(classify(&[], Some(1)), LoadFailure::Unknown);
        assert_eq!(classify(&[], None), LoadFailure::Unknown);
    }

    #[test]
    fn invalid_device_extraction() {
        assert_eq!(
            invalid_device("error while handling argument \"--device\": invalid device: ROCm1"),
            Some("ROCm1".into())
        );
        assert_eq!(
            invalid_device("invalid device: 'MTL0' extra"),
            Some("MTL0".into())
        );
        assert_eq!(invalid_device("invalid device: "), None);
        assert_eq!(invalid_device("all good"), None);
    }

    #[test]
    fn every_variant_has_text_and_step_down_hint() {
        let all = [
            LoadFailure::OutOfMemory,
            LoadFailure::InvalidDevice("Vulkan0".into()),
            LoadFailure::MissingRuntimeDll,
            LoadFailure::PortInUse,
            LoadFailure::ModelFileInvalid,
            LoadFailure::ContextTooLarge,
            LoadFailure::ImageFileInvalid,
            LoadFailure::Unknown,
        ];
        for failure in &all {
            assert!(!failure.user_message().is_empty());
            assert!(failure.next_step().ends_with('.'));
            assert!(failure.next_step_on(BackendKind::Vulkan).ends_with('.'));
            assert_eq!(failure.to_string(), failure.user_message());
        }
        assert!(!LoadFailure::ModelFileInvalid.may_succeed_on_another_backend());
        assert!(!LoadFailure::PortInUse.may_succeed_on_another_backend());
        assert!(!LoadFailure::ImageFileInvalid.may_succeed_on_another_backend());
        assert!(LoadFailure::OutOfMemory.may_succeed_on_another_backend());
    }

    /// Which failures make the supervisor retry on the same backend (0.0.6 D2c and the
    /// step-down): Save memory off for what an unsupported flash attention or q8_0 memory
    /// looks like, less of the model on the graphics card for running out of its memory.
    #[test]
    fn which_failures_retry_on_the_same_backend() {
        let table = [
            // (failure, Save memory may be the cause, out of memory)
            (LoadFailure::OutOfMemory, false, true),
            (LoadFailure::ContextTooLarge, true, true),
            (LoadFailure::Unknown, true, false),
            (LoadFailure::InvalidDevice("Vulkan0".into()), false, false),
            (LoadFailure::MissingRuntimeDll, false, false),
            (LoadFailure::PortInUse, false, false),
            (LoadFailure::ModelFileInvalid, false, false),
            // 0.0.7: the image file fails the same however the model runs; the supervisor
            // drops the image file instead.
            (LoadFailure::ImageFileInvalid, false, false),
        ];
        for (failure, save_memory, memory) in table {
            assert_eq!(
                failure.save_memory_may_be_the_cause(),
                save_memory,
                "{failure:?}"
            );
            assert_eq!(failure.is_out_of_memory(), memory, "{failure:?}");
        }
    }

    /// The lines the pinned build (b12110, `llama_init_from_model` and `llama_context` in
    /// `src/llama-context.cpp`) logs when the model refuses flash attention with a q8_0
    /// memory, on any backend; and lines that look alike but are not a refusal.
    #[test]
    fn save_memory_refusals_from_the_pinned_build() {
        for line in [
            "E llama_init_from_model: K cache type q8_0 with block size 32 does not divide n_embd_head_k=80",
            "E llama_init_from_model: V cache type q8_0 with block size 32 does not divide n_embd_head_v=80",
            "E llama_init_from_model: quantized V cache requires flash_attn to be enabled",
            "E llama_init_from_model: failed to initialize the context: quantized V cache was requested, but this requires Flash Attention",
        ] {
            let tail = lines(&format!(
                "I llama_context: constructing llama_context\n{line}\nE common_init_result: failed to create context with model 'm.gguf'"
            ));
            assert!(save_memory_refused(&tail), "{line}");
            // The classification is unchanged: the context could not be made.
            assert_eq!(classify(&tail, Some(1)), LoadFailure::ContextTooLarge);
        }
        for line in [
            // A warning; the refusal, if any, is the next line.
            "W llama_init_from_model: flash_attn is not compatible with Grok - forcing off",
            // Printed only with `-fa auto`, never with Save memory's `-fa on`.
            "I llama_init_from_model: enabling flash_attn since it is required for quantized V cache",
            "E llama_init_from_model: failed to create context",
            "llama_kv_cache: size =  127.00 MiB (  2048 cells,  28 layers,  1/1 seqs), K (q8_0):   63.50 MiB, V (q8_0):   63.50 MiB",
        ] {
            assert!(!save_memory_refused(&lines(line)), "{line}");
        }
        assert!(!save_memory_refused(&[]));
    }

    /// By the time an out-of-memory failure is shown, the app has already tried less of
    /// the model on the graphics card and the processor, so the text offers only what is
    /// left for the user, in the words of Model settings.
    #[test]
    fn out_of_memory_texts_match_what_the_app_does() {
        assert_eq!(
            LoadFailure::OutOfMemory.user_message(),
            "This model needs more memory than is free."
        );
        assert_eq!(
            LoadFailure::OutOfMemory.next_step(),
            "Close other programs or pick a smaller download. Lowering How much it can remember in Model settings also helps."
        );
        assert_eq!(
            LoadFailure::ContextTooLarge.user_message(),
            "The model loaded, but there isn't enough memory left for how much it should remember."
        );
        // Save memory may already be on in the user's settings: the app turns it off only
        // for the load that failed with it, so the text must not tell them to turn it on.
        assert_eq!(
            LoadFailure::ContextTooLarge.next_step(),
            "In Model settings, lower How much it can remember, or turn on Save memory if it is off."
        );
        for failure in [LoadFailure::OutOfMemory, LoadFailure::ContextTooLarge] {
            let text = format!("{} {}", failure.user_message(), failure.next_step());
            for jargon in ["CPU", "processor", "part of the model", "context", "!"] {
                assert!(!text.contains(jargon), "{jargon:?} in {text:?}");
            }
        }
    }

    #[test]
    fn serialises_with_a_code() {
        assert_eq!(
            serde_json::to_string(&LoadFailure::OutOfMemory).expect("json"),
            r#"{"code":"out_of_memory"}"#
        );
        assert_eq!(
            serde_json::to_string(&LoadFailure::InvalidDevice("Vulkan1".into())).expect("json"),
            r#"{"code":"invalid_device","device":"Vulkan1"}"#
        );
        // 0.0.7, src/lib/types.ts `LoadFailure`.
        assert_eq!(
            serde_json::to_string(&LoadFailure::ImageFileInvalid).expect("json"),
            r#"{"code":"image_file_invalid"}"#
        );
        assert_eq!(
            serde_json::from_str::<LoadFailure>(r#"{"code":"image_file_invalid"}"#).expect("reads"),
            LoadFailure::ImageFileInvalid
        );
    }
}
