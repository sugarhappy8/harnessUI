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
    /// The model file is missing, truncated, not a GGUF file, or does not match the layout the
    /// runner builds for its kind (a tensor or a key it needs is not in it).
    ModelFileInvalid,
    /// The file names a kind of model this version's llama.cpp has not learned ("unknown model
    /// architecture"). The file is fine: a newer version of the app comes with a newer runner.
    /// It fails the same on every backend of this build, so it is no step-down.
    UnknownArchitecture,
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
                "The model file couldn't be read. It may be incomplete or damaged, not a GGUF model, or made in an early or unusual layout."
            }
            Self::UnknownArchitecture => "This version can't open this kind of model yet.",
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
            Self::ModelFileInvalid => {
                "Download the model again, or choose a different file, such as a newer upload of the same model."
            }
            Self::UnknownArchitecture => {
                "Newer versions come with a newer model runner. Check for updates, or pick another model."
            }
            Self::ContextTooLarge => {
                "In Model settings, lower How much it can remember, or turn on Save memory if it is off."
            }
            // Shown only when loading without it failed as well (the supervisor drops it
            // first); Models → Installed offers both (0.0.7).
            Self::ImageFileInvalid => {
                "The model can load without it. In Models → Installed, try the image file again or download it again."
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
            Self::ModelFileInvalid
                | Self::UnknownArchitecture
                | Self::PortInUse
                | Self::ImageFileInvalid
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
/// after the weights loaded. Like every signature here, each must start a clause of what
/// a line says ([`Line::says`]).
const CONTEXT_SIGNATURES: &[&str] = &[
    "failed to create context",
    "failed to create_context",
    "failed to initialize the context",
    "failed to allocate buffer for kv cache",
    "failed to allocate compute pp buffers",
];

/// Allocation failures on any backend. The last is where ggml stops when a buffer could
/// not be allocated and nothing checked it, as for the image file's weights at the pinned
/// build: printed straight to stderr as "<file>:<line>: GGML_ASSERT(buffer) failed", often
/// as the only line, because the allocator's own lines go through the asynchronous logger
/// that the abort cuts off (0.0.7, `tests/fixtures/stderr_image_file_out_of_memory.txt`).
const ALLOCATION_SIGNATURES: &[&str] = &[
    "cudamalloc failed",
    "out of memory",
    "erroroutofdevicememory",
    "erroroutofhostmemory",
    "failed to allocate",
    "unable to allocate",
    "insufficient memory",
    "ggml_assert(buffer) failed",
];

/// The model file itself could not be opened or parsed, which fails the same way on every
/// backend. Every GGUF parse error is printed by a `gguf_init_from_*` function
/// ([`GGUF_READER`]); the others come from llama.cpp's model loader (all present as
/// strings in the pinned build).
///
/// The generic wrappers `error loading model: ...` and `failed to load model` are **not**
/// here: llama.cpp prints them for any exception thrown while loading, including a GPU
/// backend's own errors, and treating those as a broken file would stop the step-down to
/// the next backend (ARCHITECTURE.md 7.4). Without a file signature they classify as
/// [`LoadFailure::Unknown`].
const MODEL_FILE_SIGNATURES: &[&str] = &[
    "failed to open gguf file",
    "invalid magic characters",
    "model is corrupted or incomplete",
    // The file does not match what the runner builds for its kind: a tensor it needs is not in
    // it, has another shape, or the count differs (the early Inkling 2 uploads: 24 of 48
    // blocks, `src/llama-model-loader.cpp` v1.1.0), or a key it needs is absent.
    "check_tensor_dims: tensor",
    "done_getting_tensors: wrong number of tensors",
    "missing tensor",
    "key not found in model",
];

/// The file names a kind this llama.cpp has not learned (`src/llama-model.cpp`). Apart from
/// [`MODEL_FILE_SIGNATURES`]: the file is fine, and the words say that an update may help.
const UNKNOWN_ARCHITECTURE_SIGNATURES: &[&str] = &["unknown model architecture"];

/// The functions of ggml's GGUF reader, whose every line is about a file it could not
/// read: the model's, or the image file's while the server measures it (0.0.7).
const GGUF_READER: &str = "gguf_init_from";

/// The port was taken.
const PORT_SIGNATURES: &[&str] = &["couldn't bind http server socket"];

/// Classifies a failed load from the last lines of stderr and the exit code, for a load
/// given no image file. See [`classify_load`].
pub fn classify(stderr_tail: &[String], exit_code: Option<i32>) -> LoadFailure {
    classify_load(stderr_tail, exit_code, None, false)
}

/// Classifies a failed load from the last lines of stderr, the exit code, the image file
/// the load was given (`--mmproj`, 0.0.7, as the server was given its path), and whether
/// the server had said on stdout that it was loading it (`image_stage`: the managed-child
/// stage `mmproj_model`, which no text from a file can fake).
///
/// What the server and llama.cpp print about the image file (lines from `clip_*` and
/// `mtmd_*` functions, and the server's own about it) is partly the file's own text (a
/// kind, a tensor name), so it never counts as one of the other failures; names and paths
/// llama.cpp puts in quotes are left out of every line, and a signature must start a
/// clause ([`Line::says`]). A port that is taken or a device that does not exist counts
/// only before the image file's stage, since both stop the server when it starts. Then the
/// order matters: first the image file, which failed only
/// when the server was loading it and its own line says so for the very file it was given
/// ([`image_file_failure`]: the reasons printed with it decide, so lines a crafted file
/// brings along cannot turn it into a memory failure); then a context or allocation
/// failure, also while the image file loads (less of the model on the graphics card or
/// another backend may make room for it); then the model file. The last two are read only
/// from what was printed after the server last measured the image file: it reads the image
/// file with the same GGUF reader before the model loads and goes on when that fails, and
/// nothing is allocated before.
pub fn classify_load(
    stderr_tail: &[String],
    exit_code: Option<i32>,
    image: Option<&str>,
    image_stage: bool,
) -> LoadFailure {
    if matches!(exit_code, Some(code) if code == STATUS_DLL_NOT_FOUND || code == STATUS_INVALID_IMAGE_FORMAT)
    {
        return LoadFailure::MissingRuntimeDll;
    }
    let lines: Vec<Line<'_>> = stderr_tail.iter().map(|line| Line::new(line)).collect();
    let others = || lines.iter().filter(|line| !line.is_about_image_file());

    // A port that is taken and a device that does not exist stop the server when it starts,
    // before any model loads: once it said it was loading the image file, such lines are
    // text a crafted file brought along (P2 verification N3).
    if !image_stage {
        if others().any(|line| line.says(PORT_SIGNATURES)) {
            return LoadFailure::PortInUse;
        }
        if let Some(device) = others().find_map(Line::invalid_device) {
            return LoadFailure::InvalidDevice(device);
        }
    }
    if let Some(failure) = image
        .filter(|_| image_stage)
        .and_then(|image| image_file_failure(&lines, image))
    {
        return failure;
    }
    let after_measuring = lines
        .iter()
        .rposition(Line::is_measuring)
        .map_or(0, |at| at + 1);
    let loading = || {
        lines[after_measuring..]
            .iter()
            .filter(|line| !line.is_about_image_file())
    };
    if loading().any(|line| line.says(CONTEXT_SIGNATURES)) {
        return LoadFailure::ContextTooLarge;
    }
    if loading().any(|line| line.says(ALLOCATION_SIGNATURES)) {
        return LoadFailure::OutOfMemory;
    }
    if loading().any(|line| line.func.starts_with(GGUF_READER) || line.says(MODEL_FILE_SIGNATURES))
    {
        return LoadFailure::ModelFileInvalid;
    }
    if loading().any(|line| line.says(UNKNOWN_ARCHITECTURE_SIGNATURES)) {
        return LoadFailure::UnknownArchitecture;
    }
    LoadFailure::Unknown
}

/// Whether the load failed because of `image`, the image file it was given (0.0.7), and
/// how to treat it.
///
/// At the pinned build (b12774, llama.cpp v1.1.0: `tools/server/server-context.cpp`,
/// `tools/mtmd/mtmd.cpp`, `tools/mtmd/mtmd-model.cpp`) a failed `mtmd_init_from_file`
/// prints one line, "mtmd_init_from_file: error: <reason>" (a missing tensor, a key, a
/// mismatch, a kind it cannot use: captured in `tests/fixtures/stderr_image_file_broken.txt`),
/// and the server then "failed to load multimodal model, '<file>'" and exits. b12110 and
/// older builds printed an exception from the image-file reader first, as "clip_init:
/// failed to load model '<file>': <reason>", and put "Failed to load CLIP model" on the
/// `mtmd_init_from_file` line (`stderr_image_file_broken_b12110.txt`); the oldest, the
/// exception on the `mtmd_init_from_file` line itself, as v1.1.0 does again
/// (`stderr_image_file_owner_windows.txt`). Both forms are read, so a runtime folder from
/// before the bump still gets its failure told right.
///
/// Only the server's line decides that the image file failed, and only for the file it
/// was given: a line that is exactly that, after the timestamp and the level. The lines
/// before it say why. A graphics card's own error while the image file loads (a Vulkan
/// exception, a backend that would not start) is not the file's fault: it is
/// [`LoadFailure::Unknown`], which another backend may load, and running out of memory
/// there is [`LoadFailure::OutOfMemory`]. Only the start of a reason is looked at, where
/// llama.cpp names what failed; a kind or a name from the file, or the file's own path
/// (b12774 prints it unquoted in "failed to load mmproj model from <path>"), comes after
/// it. "mtmd_get_memory_usage: error: …" and "mtmd_get_cap_from_file: …" also appear when
/// the server measures the file before the model loads, and the load goes on after them.
fn image_file_failure(lines: &[Line<'_>], image: &str) -> Option<LoadFailure> {
    let failed = lines.iter().any(|line| {
        line.func == "load_model"
            && line
                .text
                .trim_end()
                .strip_prefix("failed to load multimodal model, '")
                .and_then(|rest| rest.strip_suffix('\''))
                .is_some_and(|printed| same_file(printed, image))
    });
    if !failed {
        return None;
    }
    let mut why = Why::File;
    for line in lines {
        let reason = match line.func {
            // b12110 and older.
            "clip_init" => line
                .text
                .strip_prefix("failed to load model '")
                .and_then(|rest| clip_init_reason(rest, image)),
            // The only line b12774 prints about why, and the owner's older build too.
            "mtmd_init_from_file" => line.text.strip_prefix("error: "),
            _ => None,
        };
        if let Some(reason) = reason {
            why = why.max(Why::of(reason));
        }
    }
    Some(match why {
        Why::File => LoadFailure::ImageFileInvalid,
        Why::Device => LoadFailure::Unknown,
        Why::Memory => LoadFailure::OutOfMemory,
    })
}

/// The reason in `clip_init: failed to load model '<file>': <reason>`, from the part after
/// the opening quote, when the file is `image`.
fn clip_init_reason<'a>(rest: &'a str, image: &str) -> Option<&'a str> {
    if let Some(reason) = rest
        .strip_prefix(image)
        .and_then(|after| after.strip_prefix("': "))
    {
        return Some(reason);
    }
    let (printed, reason) = rest.split_once("': ")?;
    same_file(printed, image).then_some(reason)
}

/// Whether the server printed `given`: the same path, or, where the log lost characters of
/// a folder name to another code page (they read as U+FFFD), the same file name.
fn same_file(printed: &str, given: &str) -> bool {
    if printed == given {
        return true;
    }
    let name = |path: &str| {
        path.rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_owned()
    };
    printed.contains('\u{FFFD}') && !name(given).is_empty() && name(printed) == name(given)
}

/// Why the image file failed to load, from the start of a reason `clip_init` or
/// `mtmd_init_from_file` printed. Ordered: memory over a device's error over the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Why {
    /// The file itself: a missing tensor or key, a kind this build cannot use, a mismatch
    /// with the model, a file that is not GGUF or cannot be opened.
    File,
    /// The graphics card or its driver: a Vulkan exception, a backend that would not start.
    Device,
    /// The card or the computer ran out of memory.
    Memory,
}

impl Why {
    fn of(reason: &str) -> Self {
        let reason = reason.trim().to_ascii_lowercase();
        // Vulkan-Hpp's exceptions read "vk::<call>: <result>", all of it the driver's.
        if reason.starts_with("vk::") {
            let memory = ["erroroutofdevicememory", "erroroutofhostmemory"]
                .iter()
                .any(|result| reason.ends_with(result));
            return if memory { Self::Memory } else { Self::Device };
        }
        if matches!(
            reason.as_str(),
            "std::bad_alloc" | "bad allocation" | "bad_alloc"
        ) {
            return Self::Memory;
        }
        // "failed to initialize CPU backend" and "failed to initialize \"<device>\" backend",
        // with the function that threw it in front ("clip_ctx" at b12110, "mtmd_backends" at
        // b12774); CUDA, HIP and Vulkan's C names, should one reach here.
        let after_function = match reason.split_once(": ") {
            Some((function, rest)) if is_function(function) => rest,
            _ => reason.as_str(),
        };
        let backend = after_function.starts_with("failed to initialize ")
            && after_function.ends_with(" backend");
        let device = ["cuda error", "hip error", "vk_error_"]
            .iter()
            .any(|start| reason.starts_with(start));
        if backend || device {
            return Self::Device;
        }
        Self::File
    }
}

/// One line of llama-server's log, taken apart for matching (0.0.7).
///
/// The pinned build starts a line with a timestamp and a level letter ("0.03.927.960 E "),
/// then, for the server's and the common code's own lines, a three-letter tag ("srv  ",
/// "cmn  "), then the function that printed it and what it said ("load_model: failed …").
/// Names llama.cpp echoes from files (an architecture, a key, a token, a path) are in
/// quotes; they are left out of [`Line::lower`], so a name in a file or a folder can't pass
/// for one of the build's own messages.
struct Line<'a> {
    /// The function that printed it, or empty for a line without one (an argument error,
    /// a continuation, an assertion).
    func: &'a str,
    /// What it said, as printed: the whole message for a line without a function.
    text: &'a str,
    /// [`Line::text`] without its quoted parts, ASCII-lowercased (so offsets match).
    lower: String,
}

impl<'a> Line<'a> {
    fn new(line: &'a str) -> Self {
        let message = message(line);
        let (func, text) = match message.split_once(": ") {
            Some((func, text)) if is_function(func) => (func, text),
            _ => ("", message),
        };
        Self {
            func,
            text,
            lower: without_quotes(text).to_ascii_lowercase(),
        }
    }

    /// Whether a clause of what the line said (its start, or what follows ": " or ", ")
    /// starts with one of `signatures`.
    fn says(&self, signatures: &[&str]) -> bool {
        clause_starts(&self.lower).any(|clause| {
            signatures
                .iter()
                .any(|signature| clause.starts_with(signature))
        })
    }

    /// The device of "invalid device: X", printed when the backend has no such device.
    fn invalid_device(&self) -> Option<String> {
        if !self.says(&["invalid device: "]) {
            return None;
        }
        invalid_device(self.text)
    }

    /// Whether the line is about the image file: printed by `clip_*` or `mtmd_*` code, or
    /// the server's own words about it.
    fn is_about_image_file(&self) -> bool {
        self.func.starts_with("clip_")
            || self.func.starts_with("mtmd_")
            || self.func == "load_model"
                && (self.text.starts_with("failed to load multimodal model")
                    || self.text.starts_with("[mtmd] "))
    }

    /// Whether the line is one the server prints when it measures the image file before
    /// the model loads (and goes on after).
    fn is_measuring(&self) -> bool {
        matches!(
            self.func,
            "mtmd_get_memory_usage" | "mtmd_get_cap_from_file"
        ) || self.func == "load_model" && self.text.starts_with("[mtmd] failed to get memory usage")
    }
}

/// What a line says after the timestamp, the level letter and the module tag the pinned
/// build puts first, where it has them.
fn message(line: &str) -> &str {
    let mut rest = line.trim();
    if let Some((stamp, after)) = rest.split_once(' ')
        && stamp.contains('.')
        && stamp
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        rest = after;
    }
    if let [level, b' ', ..] = rest.as_bytes()
        && level.is_ascii_uppercase()
    {
        rest = &rest[2..];
    }
    if let [a, b, c, b' ', ..] = rest.as_bytes()
        && [a, b, c].iter().all(|byte| byte.is_ascii_lowercase())
    {
        rest = rest[4..].trim_start();
    }
    rest
}

/// Whether `word` reads as the name of the function that printed a line.
fn is_function(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '(' | ')'))
}

/// Where the clauses of `text` start: its start, and after each ": " or ", ".
fn clause_starts(text: &str) -> impl Iterator<Item = &str> {
    std::iter::once(text).chain(text.match_indices([':', ',']).filter_map(|(at, _)| {
        let rest = &text[at + 1..];
        rest.starts_with(' ').then(|| rest.trim_start())
    }))
}

/// `text` without the parts llama.cpp puts in quotes (names and paths it echoes): from the
/// first quote to the last one of the same kind, so a quote inside a name cannot end it
/// early, or to the end of the line when it is not closed. An apostrophe between two
/// letters or digits ("couldn't") is not a quote.
fn without_quotes(text: &str) -> String {
    let mut out = text.to_owned();
    for quote in ['\'', '"'] {
        let chars: Vec<(usize, char)> = out.char_indices().collect();
        let is_quote = |at: usize| {
            let inside_word = at > 0
                && chars[at - 1].1.is_alphanumeric()
                && chars
                    .get(at + 1)
                    .is_some_and(|(_, next)| next.is_alphanumeric());
            chars[at].1 == quote && !inside_word
        };
        let Some(first) = (0..chars.len()).find(|&at| is_quote(at)) else {
            continue;
        };
        let end = (first + 1..chars.len())
            .rev()
            .find(|&at| is_quote(at))
            .map_or(out.len(), |last| chars[last].0 + quote.len_utf8());
        out.replace_range(chars[first].0..end, "");
    }
    out
}

/// Lines the pinned build logs when the **model** cannot take Save memory's settings
/// (`-fa on -ctk q8_0 -ctv q8_0`), on every backend, the processor included
/// (`llama_init_from_model` and `llama_context` in `src/llama-context.cpp`, the same words
/// at b12110 and b12774; the contract test `a_model_that_refuses_save_memory_is_retried_once_without_it`
/// reads them from a real server):
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

    /// A kind the pinned llama.cpp has not learned: captured from the real server (b12774) on a
    /// file that names "futurearch". Not a damaged file, and not one another backend would
    /// open: an update brings a newer runner, so the words say that, and "download it again"
    /// (what a broken file gets) would be false advice for a 23 GB file that is fine.
    #[test]
    fn captured_unknown_architecture_is_its_own_failure_with_its_own_words() {
        let tail = fixture(include_bytes!(
            "../../tests/fixtures/stderr_unknown_architecture.txt"
        ));
        let failure = classify(&tail, Some(1));
        assert_eq!(failure, LoadFailure::UnknownArchitecture);
        assert!(!failure.may_succeed_on_another_backend());
        let (message, step) = (failure.user_message(), failure.next_step());
        assert_eq!(message, "This version can't open this kind of model yet.");
        assert!(step.contains("Check for updates"), "{step}");
        assert!(step.contains("pick another model"), "{step}");
        for words in [message, step] {
            assert!(!words.contains("damaged"), "{words}");
            assert!(!words.contains("Download the model again"), "{words}");
        }
        assert_eq!(failure.next_step_on(BackendKind::Vulkan), step);
        // The failure's JSON is what the window reads.
        assert_eq!(
            serde_json::to_string(&failure).unwrap(),
            r#"{"code":"unknown_architecture"}"#
        );
    }

    /// The early Inkling 2 uploads (arch "inkling2", 24 of 48 blocks in the file): captured
    /// from the real server on a copy of onemunanoyo's file with its header and tensor table.
    /// The name is known and the file does not match what the runner builds from it: it fails
    /// the same on every backend, so it is a file problem, not one for a step-down, and not
    /// the unknown failure ("try a different download") it was filed under before.
    #[test]
    fn captured_missing_tensor_is_model_file_invalid() {
        let tail = fixture(include_bytes!(
            "../../tests/fixtures/stderr_tensor_not_found.txt"
        ));
        let failure = classify(&tail, Some(1));
        assert_eq!(failure, LoadFailure::ModelFileInvalid);
        assert!(!failure.may_succeed_on_another_backend());
        assert!(
            failure.next_step().contains("newer upload"),
            "{}",
            failure.next_step()
        );
    }

    /// What llama.cpp's loader says when a file does not match the layout it builds for the
    /// file's kind (v1.1.0, `src/llama-model-loader.cpp`): all are the file's, on any backend.
    #[test]
    fn a_file_that_does_not_match_its_kind_is_model_file_invalid() {
        for line in [
            "0.00.577.545 E llama_model_load: error loading model: check_tensor_dims: tensor 'blk.24.attn_q.weight' not found",
            "0.00.577.545 E llama_model_load: error loading model: check_tensor_dims: tensor 'output.weight' has wrong shape; expected  2048, 32000, got  4096, 32000",
            "0.00.577.545 E llama_model_load: error loading model: done_getting_tensors: wrong number of tensors; expected 734, got 704",
            "0.00.577.545 E llama_model_load: error loading model: missing tensor 'blk.3.ffn_gate.weight'",
            "0.00.577.545 E llama_model_load: error loading model: key not found in model: inkling2_loop.loop.start",
        ] {
            assert_eq!(
                classify(&lines(line), Some(1)),
                LoadFailure::ModelFileInvalid,
                "{line}"
            );
        }
        // A graphics backend's own exception is not a file problem, and a name from a file
        // that looks like one of these messages does not make it one.
        for line in [
            "0.00.100.000 E ggml_vulkan: device lost: 'check_tensor_dims: tensor x not found'",
            "0.00.100.000 E llama_model_load: error loading model: vk::Device::createBuffer: ErrorDeviceLost 'key not found in model: x'",
        ] {
            assert_eq!(
                classify(&lines(line), Some(1)),
                LoadFailure::Unknown,
                "{line}"
            );
        }
    }

    /// The image files of the captured logs (`tests/fixtures/stderr_image_file_*.txt`), as
    /// the server named them in its last line.
    const BROKEN: &str = "/home/u/models/someone/Sample-GGUF/Sample.mmproj-f16.gguf";
    const GARBAGE: &str = "/home/u/models/someone/Sample-GGUF/mmproj-Sample-F16.gguf";
    const OWNERS: &str = r"C:\Users\me\harnessUI\models\mradermacher\Ministral-3-14B-Reasoning-2512-Uncensored-GGUF\Ministral-3-14B-Reasoning-2512-Uncensored.mmproj-f16.gguf";

    /// A load given `image` that failed after the server said it was loading it.
    fn with_image(tail: &[String], image: &str) -> LoadFailure {
        classify_load(tail, Some(1), Some(image), true)
    }

    /// 0.0.7: the image file failed after the model loaded, captured from the pinned build
    /// (b12774) with an image file that misses a tensor its kind needs
    /// (`v.token_embd.img_break`, as in the owner's report) and with one that is not a GGUF
    /// file at all, and the same two from b12110, which printed more lines. The second log
    /// also holds the GGUF reader's "invalid magic characters", which alone names a broken
    /// model file: here it is about the image file, and the model is not blamed. Only a
    /// load given that image file can fail because of it: without one, or with another, the
    /// same log is not the image file's failure.
    #[test]
    fn captured_broken_image_files_are_image_file_invalid() {
        for (tail, image) in [
            (
                fixture(include_bytes!(
                    "../../tests/fixtures/stderr_image_file_broken.txt"
                )),
                BROKEN,
            ),
            (
                fixture(include_bytes!(
                    "../../tests/fixtures/stderr_image_file_garbage.txt"
                )),
                GARBAGE,
            ),
            (
                fixture(include_bytes!(
                    "../../tests/fixtures/stderr_image_file_broken_b12110.txt"
                )),
                BROKEN,
            ),
            (
                fixture(include_bytes!(
                    "../../tests/fixtures/stderr_image_file_garbage_b12110.txt"
                )),
                GARBAGE,
            ),
        ] {
            let failure = with_image(&tail, image);
            assert_eq!(failure, LoadFailure::ImageFileInvalid, "{tail:?}");
            assert!(!failure.may_succeed_on_another_backend());
            assert!(!failure.save_memory_may_be_the_cause());
            assert!(!failure.is_out_of_memory());
            assert_ne!(classify(&tail, Some(1)), LoadFailure::ImageFileInvalid);
            assert_ne!(
                with_image(&tail, "/home/u/models/other/mmproj-F16.gguf"),
                LoadFailure::ImageFileInvalid
            );
        }
    }

    /// The owner's log (Windows, every backend failing the same way): the exception is on
    /// the `mtmd_init_from_file` line itself, without a `clip_init` line, the form b12774
    /// prints again. The user folder is replaced and the folder the report left out is the
    /// one downloads use.
    #[test]
    fn the_owners_log_is_image_file_invalid() {
        let tail = fixture(include_bytes!(
            "../../tests/fixtures/stderr_image_file_owner_windows.txt"
        ));
        assert_eq!(with_image(&tail, OWNERS), LoadFailure::ImageFileInvalid);
    }

    /// The server's own last word decides, "failed to load multimodal model, '<file>'" for
    /// exactly the image file it was given, at the start of a message (m3). The `clip_init`
    /// and `mtmd_init_from_file` lines before it only say why.
    #[test]
    fn only_the_servers_line_for_the_given_image_file_decides() {
        let path = r"C:\Users\me\harnessUI\models\x\M.mmproj-f16.gguf";
        let clip = format!(
            "0.03.927.960 E clip_init: failed to load model '{path}': operator(): unable to find tensor v.token_embd.img_break"
        );
        let mtmd = format!(
            "0.03.927.961 E mtmd_init_from_file: error: Failed to load CLIP model from {path}"
        );
        let server =
            format!("0.03.927.966 E srv    load_model: failed to load multimodal model, '{path}'");
        let all = lines(&format!(
            "{clip}\n{mtmd}\n{server}\n0.03.929.912 E srv  server_run_i: exiting due to model loading error"
        ));
        assert_eq!(with_image(&all, path), LoadFailure::ImageFileInvalid);
        assert_eq!(
            with_image(&lines(&server), path),
            LoadFailure::ImageFileInvalid
        );
        // The form b12774 prints: the reason is on the `mtmd_init_from_file` line.
        let reason = "0.03.927.960 E mtmd_init_from_file: error: operator(): unable to find tensor v.token_embd.img_break".to_owned();
        let new = lines(&format!("{reason}\n{server}"));
        assert_eq!(with_image(&new, path), LoadFailure::ImageFileInvalid);
        for alone in [&clip, &mtmd, &reason] {
            assert_eq!(
                with_image(&lines(alone), path),
                LoadFailure::Unknown,
                "{alone}"
            );
        }
        // Another image file, none given, or the words somewhere else in a line.
        assert_eq!(
            with_image(&all, r"C:\Users\me\harnessUI\models\x\other.gguf"),
            LoadFailure::Unknown
        );
        assert_eq!(classify(&all, Some(1)), LoadFailure::Unknown);
        // The server never said it was loading the image file (stdout): stderr alone, which
        // text from files reaches, cannot make it the image file's failure (P2 verification N3).
        assert_eq!(
            classify_load(&all, Some(1), Some(path), false),
            LoadFailure::Unknown
        );
        let echoed = lines(&format!(
            "E srv  log_server_r: request: error: failed to load multimodal model, '{path}'"
        ));
        assert_eq!(with_image(&echoed, path), LoadFailure::Unknown);
    }

    /// M1: a graphics card's own error while the image file loads (the reader prints every
    /// exception it catches) is not the file's fault: it is not an image file failure, so
    /// it is not remembered, and another backend may load the model with it. Running out of
    /// the card's memory there is running out of memory.
    ///
    /// Both forms: b12110 printed the exception on a `clip_init` line and "Failed to load
    /// CLIP model" on the `mtmd_init_from_file` line; b12774 prints the exception on the
    /// `mtmd_init_from_file` line alone (`mtmd_backends` is the constructor that throws the
    /// backend failures, `tools/mtmd/mtmd-subcontext.cpp`). The card errors here are from
    /// the sources of those builds, not captured: the container has no graphics card.
    #[test]
    fn a_device_error_while_the_image_file_loads_is_not_the_files_fault() {
        let path = "/m/mmproj-F16.gguf";
        let b12110 = |what: &str| {
            lines(&format!(
                "0.03.927.960 E clip_init: failed to load model '{path}': {what}\n\
                 0.03.927.961 E mtmd_init_from_file: error: Failed to load CLIP model from {path}\n\
                 0.03.927.966 E srv    load_model: failed to load multimodal model, '{path}'\n\
                 0.03.929.912 E srv  server_run_i: exiting due to model loading error"
            ))
        };
        let b12774 = |what: &str| {
            lines(&format!(
                "0.03.927.960 E mtmd_init_from_file: error: {what}\n\
                 0.03.927.966 E srv    load_model: failed to load multimodal model, '{path}'\n\
                 0.03.929.912 E srv  server_run_i: exiting due to model loading error"
            ))
        };
        for (what, new) in [
            ("vk::Queue::submit: ErrorDeviceLost", None),
            ("vk::Device::createComputePipeline: ErrorUnknown", None),
            (
                "clip_ctx: failed to initialize \"Vulkan0\" backend",
                Some("mtmd_backends: failed to initialize \"Vulkan0\" backend"),
            ),
            (
                "clip_ctx: failed to initialize CPU backend",
                Some("failed to initialize CPU backend"),
            ),
        ] {
            for tail in [b12110(what), b12774(new.unwrap_or(what))] {
                let failure = with_image(&tail, path);
                assert_eq!(failure, LoadFailure::Unknown, "{what}");
                assert!(failure.may_succeed_on_another_backend());
            }
        }
        for tail in [
            b12110("vk::Device::allocateMemory: ErrorOutOfDeviceMemory"),
            b12774("vk::Device::allocateMemory: ErrorOutOfDeviceMemory"),
            b12774("std::bad_alloc"),
        ] {
            assert_eq!(with_image(&tail, path), LoadFailure::OutOfMemory);
        }
        // The file's own faults stay its own.
        for what in [
            "operator(): unable to find tensor v.token_embd.img_break",
            "operator(): unable to find tensor v.patch_embd.weight",
            "load_hparams: unknown projector type: newkind",
            "Key not found: clip.vision.image_size",
            "clip_model_loader: failed to load CLIP model from /m/mmproj-F16.gguf. Does this file exist?",
        ] {
            assert_eq!(
                with_image(&b12110(what), path),
                LoadFailure::ImageFileInvalid,
                "{what}"
            );
            assert_eq!(
                with_image(&b12774(what), path),
                LoadFailure::ImageFileInvalid,
                "{what}"
            );
        }
        // What b12774 says for a file that is not GGUF, with no encoder, or that holds no
        // model it can use: the file's own path is in the line, and does not decide.
        for what in [
            format!("mtmd_model_loader: failed to load mmproj model from {path}. Does this file exist?"),
            format!("Failed to load mmproj model from {path}"),
            "mtmd_model_loader: failed to load mmproj model from /m/failed to initialize the Vulkan0 backend.gguf. Does this file exist?".to_owned(),
        ] {
            assert_eq!(
                with_image(&b12774(&what), path),
                LoadFailure::ImageFileInvalid,
                "{what}"
            );
        }
        // The mismatch llama.cpp checks after the reader, on the mtmd line.
        let mismatch = lines(&format!(
            "E mtmd_init_from_file: error: mismatch between text model (n_embd = 5120) and mmproj (n_embd = 4096)\n\
             E srv    load_model: failed to load multimodal model, '{path}'"
        ));
        assert_eq!(with_image(&mismatch, path), LoadFailure::ImageFileInvalid);
    }

    /// Running out of memory while the image file loads is still running out of memory:
    /// less of the model on the graphics card, or another backend, may make room for it. At
    /// the pinned build a failed allocation of its weights is reported by ggml's buffer
    /// allocator and then stops the server at an assertion, before any line about the image
    /// file (m8: the lines this build prints).
    #[test]
    fn running_out_of_memory_while_the_image_file_loads_is_out_of_memory() {
        let tail = |alloc: &str| {
            lines(&format!(
                "0.00.033.358 I srv    load_model: loading model '/m/model.gguf'\n\
                 0.03.741.858 I cmn          init: llama threadpool init, n_threads = 12\n\
                 {alloc}\n\
                 /src/llama.cpp/ggml/src/ggml-backend.cpp:328: GGML_ASSERT(buffer) failed"
            ))
        };
        for alloc in [
            "0.03.900.000 E ggml_backend_buft_alloc_buffer_n_default: failed to allocate Vulkan0 buffer of size 851443712",
            "0.03.900.000 E ggml_vulkan: Device memory allocation of size 851443712 failed.\n0.03.900.001 E ggml_backend_buft_alloc_buffer_n_default: failed to allocate Vulkan0 buffer of size 851443712",
            "0.03.900.000 E ggml_backend_cuda_buffer_type_alloc_buffer: allocating 812.00 MiB on device 0: cudaMalloc failed: out of memory",
            "0.03.900.000 E ggml_metal_buffer_init: error: failed to allocate buffer, size = 812.00 MiB",
        ] {
            for image in [None, Some("/m/mmproj-F16.gguf")] {
                let failure = classify_load(&tail(alloc), Some(134), image, image.is_some());
                assert_eq!(failure, LoadFailure::OutOfMemory, "{alloc}");
                assert!(failure.may_succeed_on_another_backend());
            }
        }
    }

    /// The server measures the image file before the model loads and goes on when that
    /// fails: such a line alone never makes the failure the image file's, and what the GGUF
    /// reader says about the image file then is not about the model (m1). What failed after
    /// it decides.
    #[test]
    fn measuring_the_image_file_is_not_a_failure_of_it() {
        let b12110 = "E mtmd_get_memory_usage: error: Failed to load CLIP model from /m/mmproj.gguf\n\
                        E mtmd_get_cap_from_file: failed to get capabilities from file '/m/mmproj.gguf': clip_model_loader: failed to load CLIP model from /m/mmproj.gguf. Does this file exist?\n\
                        E srv    load_model: [mtmd] failed to get memory usage of mmproj";
        // b12774 prints the loader's own reason after the same two function names.
        let b12774 = "E mtmd_get_memory_usage: error: mtmd_model_loader: failed to load mmproj model from /m/mmproj.gguf. Does this file exist?\n\
                      E mtmd_get_cap_from_file: failed to get capabilities from file '/m/mmproj.gguf': mtmd_model_loader: failed to load mmproj model from /m/mmproj.gguf. Does this file exist?";
        for measured in [b12110, b12774] {
            for image in [None, Some("/m/mmproj.gguf")] {
                let load = |tail: &[String]| classify_load(tail, Some(1), image, image.is_some());
                assert_eq!(load(&lines(measured)), LoadFailure::Unknown);
                let weights = lines(&format!(
                    "{measured}\nE alloc_tensor_range: failed to allocate CUDA0 buffer of size 9437184000\n\
                 E llama_model_load_from_file_impl: failed to load model"
                ));
                assert_eq!(load(&weights), LoadFailure::OutOfMemory);
                let model = lines(&format!(
                    "{measured}\nE gguf_init_from_file_impl: failed to read header\n\
                 E llama_model_load_from_file_impl: failed to load model"
                ));
                assert_eq!(load(&model), LoadFailure::ModelFileInvalid);
                let context = lines(&format!(
                    "{measured}\nE common_init_result: failed to create context with model 'm.gguf'"
                ));
                assert_eq!(load(&context), LoadFailure::ContextTooLarge);
            }
        }

        // The real capture of a file that is not GGUF, up to where the model loads: the GGUF
        // reader's complaints are about the image file. Then the graphics card fails while
        // the model loads: that is the card's failure, and the next backend may load it.
        let garbage = fixture(include_bytes!(
            "../../tests/fixtures/stderr_image_file_garbage.txt"
        ));
        let before_model: Vec<String> = garbage
            .iter()
            .take_while(|l| !l.contains("control-looking token"))
            .cloned()
            .collect();
        assert!(
            before_model
                .iter()
                .any(|l| l.contains("invalid magic characters"))
        );
        let device_lost = lines(
            "0.00.900.000 E llama_model_load: error loading model: vk::Queue::submit: ErrorDeviceLost\n\
             0.00.900.001 E llama_model_load_from_file_impl: failed to load model\n\
             0.00.900.002 E srv    load_model: failed to load model, '/home/u/models/Qwen3-0.6B-Q8_0.gguf'\n\
             0.00.900.003 E srv  server_run_i: exiting due to model loading error",
        );
        let tail = [before_model.clone(), device_lost.clone()].concat();
        for image in [None, Some(GARBAGE)] {
            let failure = classify_load(&tail, Some(1), image, image.is_some());
            assert_eq!(failure, LoadFailure::Unknown, "{image:?}");
            assert!(failure.may_succeed_on_another_backend());
        }
        // A cut-short image file: the reader's complaint, measured, then the card fails.
        let truncated = lines(
            "0.00.014.268 I srv    load_model: loading model '/m/model.gguf'\n\
             0.00.015.000 E gguf_init_from_file_impl: failed to read tensor data\n\
             0.00.015.100 E mtmd_get_memory_usage: error: Failed to load CLIP model from /m/mmproj-F16.gguf\n\
             0.00.900.000 E llama_model_load: error loading model: vk::Device::createComputePipeline: ErrorUnknown\n\
             0.00.900.001 E llama_model_load_from_file_impl: failed to load model",
        );
        assert_eq!(
            classify_load(&truncated, Some(1), Some("/m/mmproj-F16.gguf"), true),
            LoadFailure::Unknown
        );
        // The model's own file broken after the image file was measured is still the model's.
        let broken_model = [
            before_model,
            lines("0.00.020.000 E gguf_init_from_file_impl: failed to read header"),
        ]
        .concat();
        assert_eq!(
            classify_load(&broken_model, Some(1), Some(GARBAGE), true),
            LoadFailure::ModelFileInvalid
        );
    }

    /// m3: names and paths llama.cpp quotes from files (an architecture, a key, a tensor, a
    /// folder) never pass for one of its own messages.
    #[test]
    fn text_from_files_does_not_change_the_classification() {
        for image in [None, Some("/m/mmproj-F16.gguf")] {
            let load = |text: &str| classify_load(&lines(text), Some(1), image, image.is_some());
            assert_eq!(
                load(
                    "0.00.020.000 E llama_model_load: error loading model: error loading model architecture: unknown model architecture: 'failed to load multimodal model'\n\
                     0.00.020.001 E llama_model_load_from_file_impl: failed to load model"
                ),
                LoadFailure::UnknownArchitecture
            );
            assert_eq!(
                load(
                    "0.00.020.000 E gguf_init_from_reader: duplicate key 'clip_init: failed to load model' for KV pair 3\n\
                     0.00.020.001 E llama_model_load_from_file_impl: failed to load model"
                ),
                LoadFailure::ModelFileInvalid
            );
            // Folders named like an error, with the card failing for its own reason.
            for folder in [
                "Out of memory tests",
                "couldn't bind HTTP server socket",
                "invalid device: Vulkan9",
                "failed to create context",
                "gguf_init_from here",
            ] {
                let text = format!(
                    "0.00.013.567 I srv    load_model: loading model '/home/u/{folder}/model.gguf'\n\
                     0.00.900.000 E llama_model_load: error loading model: vk::Queue::submit: ErrorDeviceLost\n\
                     0.00.900.002 E srv    load_model: failed to load model, '/home/u/{folder}/model.gguf'"
                );
                assert_eq!(load(&text), LoadFailure::Unknown, "{folder}");
            }
        }
        // A broken image file in such a folder is still the image file's.
        let image = "/home/u/Out of memory tests/mmproj-F16.gguf";
        let tail = lines(&format!(
            "0.03.927.960 E clip_init: failed to load model '{image}': operator(): unable to find tensor v.token_embd.img_break\n\
             0.03.927.966 E srv    load_model: failed to load multimodal model, '{image}'"
        ));
        assert_eq!(with_image(&tail, image), LoadFailure::ImageFileInvalid);
        // A crafted kind of image file names what a card says when it runs out of memory:
        // the image file's own line is about the image file, never a memory failure.
        let crafted = lines(
            "0.03.927.960 E clip_init: failed to load model '/m/mmproj.gguf': load_hparams: unknown projector type: out of memory\n\
             0.03.927.966 E srv    load_model: failed to load multimodal model, '/m/mmproj.gguf'",
        );
        assert_eq!(
            with_image(&crafted, "/m/mmproj.gguf"),
            LoadFailure::ImageFileInvalid
        );
        // A path the model loader prints without quotes, in a folder named like an error.
        let unquoted = lines(
            "0.00.015.353 E gguf_init_from_file: failed to open GGUF file '/home/u/Out of memory tests/model.gguf' (No such file or directory)\n\
             0.00.015.734 E llama_model_load: error loading model: llama_model_loader: failed to load model from /home/u/Out of memory tests/model.gguf\n\
             0.00.015.748 E llama_model_load_from_file_impl: failed to load model",
        );
        assert_eq!(classify(&unquoted, Some(1)), LoadFailure::ModelFileInvalid);
        // An apostrophe in a word is not a quote ("couldn't").
        assert_eq!(
            classify(
                &lines(
                    "0.00.011.000 E srv  init_listene: couldn't bind HTTP server socket, hostname: 127.0.0.1, port: 8080"
                ),
                Some(1)
            ),
            LoadFailure::PortInUse
        );
    }

    /// The image file of the captured logs below.
    const SMOLVLM: &str =
        "/home/u/models/ggml-org/SmolVLM-256M-Instruct-GGUF/mmproj-SmolVLM-256M-Instruct-Q8_0.gguf";

    /// P2 verification N2: at the pinned build an image file that does not fit stops the
    /// server at ggml's assertion, and the allocator's own lines, which go through the
    /// asynchronous logger, are usually lost (5 runs of 6; captured in
    /// `tests/fixtures/stderr_image_file_out_of_memory.txt`). The assertion is running out
    /// of memory, also while the image file loads; another assertion, or its words quoted
    /// from a file, is not.
    #[test]
    fn running_out_of_memory_for_the_image_file_ends_at_an_assertion() {
        let tail = fixture(include_bytes!(
            "../../tests/fixtures/stderr_image_file_out_of_memory.txt"
        ));
        for (image, stage) in [(Some(SMOLVLM), true), (Some(SMOLVLM), false), (None, false)] {
            assert_eq!(
                classify_load(&tail, Some(134), image, stage),
                LoadFailure::OutOfMemory,
                "{image:?} {stage}"
            );
        }
        for text in [
            r"C:\a\llama.cpp\ggml\src\ggml-backend.cpp:328: GGML_ASSERT(buffer) failed",
            "/src/ggml/src/ggml-backend.cpp:328: GGML_ASSERT(buffer) failed\n\
             0.00.244.243 E ggml_backend_cpu_buffer_type_alloc_buffer: failed to allocate buffer of size 103756800",
        ] {
            assert_eq!(
                classify_load(&lines(text), None, Some(SMOLVLM), true),
                LoadFailure::OutOfMemory,
                "{text}"
            );
        }
        for text in [
            "/src/tools/mtmd/clip.cpp:4705: GGML_ASSERT(cur && \"tensor not found in ctx_data\") failed",
            "0.00.100.000 W load: control-looking token: 1 'GGML_ASSERT(buffer) failed' was not control-type",
        ] {
            assert_eq!(
                classify_load(&lines(text), None, Some(SMOLVLM), true),
                LoadFailure::Unknown,
                "{text}"
            );
        }
    }

    /// P2 verification N3: a crafted image file's duplicate key and its kind are printed
    /// with raw line breaks, so they bring lines that look like an allocation failure
    /// (captured from the pinned build). After the server said it was loading the image
    /// file, its own line for exactly that file decides from the reasons printed with it:
    /// the failure stays the image file's, and no memory retry repeats at every load.
    #[test]
    fn a_crafted_image_file_cannot_pass_for_running_out_of_memory() {
        for (bytes, image) in [
            (
                &include_bytes!("../../tests/fixtures/stderr_image_file_crafted_key.txt")[..],
                "/home/u/models/someone/Sample-GGUF/dupkey-oom.mmproj-f16.gguf",
            ),
            (
                &include_bytes!("../../tests/fixtures/stderr_image_file_crafted_kind.txt")[..],
                "/home/u/models/someone/Sample-GGUF/projtype.mmproj-f16.gguf",
            ),
        ] {
            let tail = fixture(bytes);
            assert_eq!(
                classify_load(&tail, Some(1), Some(image), true),
                LoadFailure::ImageFileInvalid,
                "{image}"
            );
        }
        // What it brings while the server measures it, before the model loads, is not the
        // model's failure either: here the model's own file then turns out broken.
        let tail = fixture(include_bytes!(
            "../../tests/fixtures/stderr_image_file_crafted_key.txt"
        ));
        let measured: Vec<String> = tail
            .iter()
            .take_while(|l| !l.contains("control-looking token"))
            .cloned()
            .collect();
        let broken_model = [
            measured,
            lines(
                "0.00.200.000 E gguf_init_from_file_impl: failed to read header\n\
                 0.00.200.001 E llama_model_load_from_file_impl: failed to load model",
            ),
        ]
        .concat();
        assert_eq!(
            classify_load(
                &broken_model,
                Some(1),
                Some("/home/u/models/someone/Sample-GGUF/dupkey-oom.mmproj-f16.gguf"),
                false
            ),
            LoadFailure::ModelFileInvalid
        );
    }

    /// P2 verification N3 (the rest): a crafted image file's duplicate key or kind can also
    /// bring the line of a port that is taken or of a device that does not exist (captured
    /// from the pinned build). Those stop the server when it starts, before any model loads:
    /// after it said it was loading the image file they are the file's text, so the verdict
    /// is the image file's, not new ports or another device. Before that they count.
    #[test]
    fn a_crafted_image_file_cannot_pass_for_a_port_or_a_device() {
        for (bytes, image) in [
            (
                &include_bytes!("../../tests/fixtures/stderr_image_file_crafted_key_port.txt")[..],
                "/home/u/models/someone/Sample-GGUF/dupkey-port.mmproj-f16.gguf",
            ),
            (
                &include_bytes!("../../tests/fixtures/stderr_image_file_crafted_kind_port.txt")[..],
                "/home/u/models/someone/Sample-GGUF/kind-port.mmproj-f16.gguf",
            ),
            (
                &include_bytes!("../../tests/fixtures/stderr_image_file_crafted_key_device.txt")[..],
                "/home/u/models/someone/Sample-GGUF/dupkey-device.mmproj-f16.gguf",
            ),
        ] {
            let tail = fixture(bytes);
            assert_eq!(
                classify_load(&tail, Some(1), Some(image), true),
                LoadFailure::ImageFileInvalid,
                "{image}"
            );
        }
        for (bytes, want) in [
            (
                &include_bytes!("../../tests/fixtures/stderr_port_in_use.txt")[..],
                LoadFailure::PortInUse,
            ),
            (
                &include_bytes!("../../tests/fixtures/stderr_invalid_device.txt")[..],
                LoadFailure::InvalidDevice("Vulkan0".into()),
            ),
        ] {
            assert_eq!(
                classify_load(&fixture(bytes), Some(1), Some("/m/mmproj.gguf"), false),
                want
            );
        }
    }

    /// P2 verification N3: someone else's broken model file can print what looks like the
    /// server's own line about the image file (captured: a GGUF key with a line break, and
    /// a byte that reads as U+FFFD where the folder would be). The server never said it was
    /// loading the image file, so the model's file is to blame and the image file is left
    /// alone.
    #[test]
    fn a_model_file_cannot_blame_the_image_file() {
        let tail = fixture(include_bytes!(
            "../../tests/fixtures/stderr_model_fakes_image_file_line.txt"
        ));
        assert!(
            tail.iter()
                .any(|l| l.contains("failed to load multimodal model"))
        );
        assert_eq!(
            classify_load(&tail, Some(1), Some(SMOLVLM), false),
            LoadFailure::ModelFileInvalid
        );
    }

    /// Logs that say nothing about an image file stay as they were, with or without one.
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
            (
                "invalid device",
                &include_bytes!("../../tests/fixtures/stderr_invalid_device.txt")[..],
                LoadFailure::InvalidDevice("Vulkan0".into()),
            ),
        ] {
            assert_eq!(classify(&fixture(bytes), Some(1)), want, "{name}");
            assert_eq!(
                classify_load(&fixture(bytes), Some(1), Some("/m/mmproj.gguf"), false),
                want,
                "{name}"
            );
        }
        let unrelated = lines(
            "I srv    load_model: loading model '/m/model.gguf'\n\
             I srv    load_model: loaded multimodal model, '/m/mmproj.gguf'\n\
             E llama_model_load: error loading model: vk::Queue::submit: ErrorDeviceLost",
        );
        assert_eq!(classify(&unrelated, Some(1)), LoadFailure::Unknown);
        assert_eq!(
            with_image(&unrelated, "/m/mmproj.gguf"),
            LoadFailure::Unknown
        );
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

    /// The lines the pinned build (b12110 and b12774, `llama_init_from_model` and `llama_context` in
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
