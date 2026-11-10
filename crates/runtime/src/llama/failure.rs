//! Classifying why llama-server failed to load (ARCHITECTURE.md 7.4 and 15).
//!
//! The signatures come from llama.cpp's source at the pinned build and from failures
//! captured from the pinned binary (`tests/fixtures/stderr_*.txt`). Log wording can change
//! in any release; the contract tests re-check it on every bump.

use serde::{Deserialize, Serialize};

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
    /// None of the known signatures matched.
    Unknown,
}

impl LoadFailure {
    /// A short plain-language explanation.
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::OutOfMemory => "This model doesn't fit in the available memory.",
            Self::InvalidDevice(_) => {
                "The selected graphics device isn't available to this backend."
            }
            Self::MissingRuntimeDll => "A file the model runner needs is missing or damaged.",
            Self::PortInUse => "Another program is using the network port the model runner needs.",
            Self::ModelFileInvalid => {
                "The model file couldn't be read. It may be incomplete, damaged, not a GGUF model, or a kind of model this version can't run."
            }
            Self::ContextTooLarge => {
                "The model loaded, but the chosen context length doesn't fit in memory."
            }
            Self::Unknown => "The model runner stopped while loading the model.",
        }
    }

    /// What the user can do about it.
    pub fn next_step(&self) -> &'static str {
        match self {
            Self::OutOfMemory => {
                "Use a smaller context, run part of the model on the CPU, or pick a smaller model."
            }
            Self::InvalidDevice(_) => {
                "Choose another device or backend in Settings, or switch to CPU for now."
            }
            Self::MissingRuntimeDll => {
                "Reinstall the app. For a downloaded backend, remove it in Settings and download it again."
            }
            Self::PortInUse => "Restart the app.",
            Self::ModelFileInvalid => "Download the model again, or choose a different file.",
            Self::ContextTooLarge => "Lower the context length and try again.",
            Self::Unknown => "Copy the details and check the logs, or try the CPU backend.",
        }
    }

    /// Whether trying the next backend down could help (ARCHITECTURE.md 7.4, "Stepping
    /// down"). A broken model file or a port problem fails the same way on any backend.
    pub fn may_succeed_on_another_backend(&self) -> bool {
        !matches!(self, Self::ModelFileInvalid | Self::PortInUse)
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

/// Classifies a failed load from the last lines of stderr and the exit code.
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
    if any(MODEL_FILE_SIGNATURES) {
        return LoadFailure::ModelFileInvalid;
    }
    LoadFailure::Unknown
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
            LoadFailure::Unknown,
        ];
        for failure in &all {
            assert!(!failure.user_message().is_empty());
            assert!(failure.next_step().ends_with('.'));
            assert_eq!(failure.to_string(), failure.user_message());
        }
        assert!(!LoadFailure::ModelFileInvalid.may_succeed_on_another_backend());
        assert!(!LoadFailure::PortInUse.may_succeed_on_another_backend());
        assert!(LoadFailure::OutOfMemory.may_succeed_on_another_backend());
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
    }
}
