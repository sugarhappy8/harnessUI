//! Naming why the Studio engine failed (docs/dev/0.1.0/studio-local-design.md, "Out of
//! memory" and "Other failures").
//!
//! The signatures are the pinned build's words (`master-1106-532d2c1` with its ggml and the
//! Vulkan driver's), the system loader's on Linux and macOS, and Windows' loader exit code;
//! `tests/fixtures/sd_stderr_*.txt` follow them. Log wording has no interface guarantee: a
//! bump of the pin re-checks it.

use crate::llama::devices::STATUS_DLL_NOT_FOUND;

/// Why a local run failed, in terms the card can explain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdFailure {
    /// The graphics card, or the computer, ran out of memory. Studio tries once more with
    /// memory saving.
    OutOfMemory,
    /// The graphics driver stopped responding (Vulkan's device lost).
    DeviceLost,
    /// A library the engine needs could not be loaded: its file name, or empty when only
    /// Windows' exit code (0xC0000135) said so.
    MissingLibrary(String),
    /// A model file is damaged or is not the model it should be.
    BadModel,
    /// The engine's program is not on this computer.
    EngineMissing,
    /// The engine's program could not be started (refused by the system), and why.
    EngineBlocked(String),
    /// The engine stopped for another reason: its exit code, or `None` when a signal ended it.
    Crashed {
        /// The exit code, if the engine exited.
        code: Option<i32>,
    },
    /// The user stopped the run.
    Cancelled,
    /// Reading or writing the run's files failed, and why.
    Io(String),
}

/// Names the failure of a run that did not succeed.
///
/// `tail`: the last 60 lines of stdout and stderr merged in arrival order (this build logs
/// INFO and WARN to stdout, ERROR to stderr).
///
/// Lines are read lower-cased, and what this build prints in quotes (paths, tensor names) is
/// left out, so a folder or a name holding a failure's words does not name the failure.
/// Lines about the engine's own `retrying with … tiling` after a decoder ran out of memory
/// are skipped: that retry is not a failure. Then, in order:
/// - a library that could not be loaded (`error while loading shared libraries: <name>`,
///   `library not loaded: <name>`, or Windows' `0xC0000135` exit code, which prints
///   nothing), first because the engine then never ran;
/// - a bad model file (`invalid magic`, `failed to load model`, `unknown model`,
///   `tensor '…' not found`, and this build's `unknown format`, `get sd version from file
///   failed`, `not in model metadata`, `has wrong shape in model metadata`, `invalid model
///   source`), before memory, because a damaged file's sizes can make an allocation fail and
///   memory saving would not help;
/// - a lost device (`errordevicelost`, `device lost`), before memory, because allocations
///   on a lost device fail too;
/// - out of memory (`out of memory`, `erroroutofdevicememory`, `failed to allocate`,
///   `buffer failed`, `graph allocation failed`, `cannot make enough memory available`,
///   `memory allocation of size`), only after the last tiling retry: the allocator's and
///   the driver's words come before that retry, so a run that recovered by tiling and then
///   stopped for another reason did not run out of memory.
///
/// Anything else is [`SdFailure::Crashed`] with the exit code. (`--auto-fit`, on by default
/// in this build, already spreads the weights over the card, memory and disk; Studio never
/// passes `--params-backend`, which turns it off.)
pub fn classify(tail: &[String], exit_code: Option<i32>) -> SdFailure {
    let all: Vec<Line<'_>> = tail.iter().map(|line| Line::new(line)).collect();
    let after_last_retry = all
        .iter()
        .rposition(Line::is_tiling_retry)
        .map_or(0, |at| at + 1);
    let lines: Vec<&Line<'_>> = all.iter().filter(|line| !line.is_tiling_retry()).collect();
    if let Some(name) = lines.iter().find_map(|line| line.missing_library()) {
        return SdFailure::MissingLibrary(name);
    }
    if exit_code == Some(STATUS_DLL_NOT_FOUND) {
        return SdFailure::MissingLibrary(String::new());
    }
    if lines.iter().any(|line| line.is_bad_model()) {
        return SdFailure::BadModel;
    }
    if lines.iter().any(|line| line.says_any(DEVICE_LOST)) {
        return SdFailure::DeviceLost;
    }
    if all[after_last_retry..]
        .iter()
        .any(|line| line.says_any(OUT_OF_MEMORY))
    {
        return SdFailure::OutOfMemory;
    }
    SdFailure::Crashed { code: exit_code }
}

const OUT_OF_MEMORY: &[&str] = &[
    "out of memory",
    "erroroutofdevicememory",
    "failed to allocate",
    "buffer failed",
    "graph allocation failed",
    "cannot make enough memory available",
    "memory allocation of size",
];

const DEVICE_LOST: &[&str] = &["errordevicelost", "device lost"];

const BAD_MODEL: &[&str] = &[
    "invalid magic",
    "failed to load model",
    "unknown model",
    // This build's own words (model_loader.cpp, diffusion_engine.cpp, model_manager.cpp,
    // model_loader_files.cpp): a file of no format it reads, or not the model it should be.
    "unknown format",
    "get sd version from file failed",
    "not in model metadata",
    "has wrong shape in model metadata",
    "invalid model source",
];

const MISSING_LIBRARY: &[&str] = &[
    "error while loading shared libraries:",
    "library not loaded:",
];

/// One line of the tail, as the classifier reads it.
struct Line<'a> {
    original: &'a str,
    /// ASCII lower-cased, so byte offsets match `original`.
    lower: String,
    /// `lower` with what is inside single quotes left out (the quotes stay).
    unquoted: String,
}

impl<'a> Line<'a> {
    fn new(original: &'a str) -> Self {
        let lower = original.to_ascii_lowercase();
        let unquoted = without_quoted(&lower);
        Self {
            original,
            lower,
            unquoted,
        }
    }

    fn says_any(&self, signatures: &[&str]) -> bool {
        signatures
            .iter()
            .any(|signature| self.unquoted.contains(signature))
    }

    /// The engine's own retry with a tiled decoder (`VAE decode ran out of memory;
    /// retrying with spatial tiling`).
    fn is_tiling_retry(&self) -> bool {
        self.unquoted
            .find("retrying with")
            .is_some_and(|at| self.unquoted[at..].contains("tiling"))
    }

    fn is_bad_model(&self) -> bool {
        self.says_any(BAD_MODEL)
            || self
                .lower
                .find("tensor '")
                .is_some_and(|at| self.lower[at + "tensor '".len()..].contains("' not found"))
    }

    /// The library's file name after the loader's words, without its folder.
    fn missing_library(&self) -> Option<String> {
        if !self.says_any(MISSING_LIBRARY) {
            return None;
        }
        let end = MISSING_LIBRARY
            .iter()
            .find_map(|signature| self.lower.find(signature).map(|at| at + signature.len()))?;
        let name = self.original[end..]
            .trim_start()
            .split(|c: char| c == ':' || c.is_whitespace())
            .next()
            .unwrap_or_default()
            .trim_matches(['\'', '"', '`']);
        let name = name.rsplit(['/', '\\']).next().unwrap_or_default();
        Some(name.to_owned())
    }
}

/// `text` with the inside of each pair of single quotes left out: `from '/a b' ok` becomes
/// `from '' ok`. An unpaired quote leaves the rest as it is.
fn without_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('\'') {
        let Some(len) = rest[open + 1..].find('\'') else {
            break;
        };
        out.push_str(&rest[..=open]);
        out.push('\'');
        rest = &rest[open + 1 + len + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failure fixture's lines: the merged tail the engine runner keeps.
    fn fixture(name: &str) -> Vec<String> {
        let text = match name {
            "sd_stderr_out_of_memory_vulkan.txt" => {
                include_str!("../../tests/fixtures/sd_stderr_out_of_memory_vulkan.txt")
            }
            "sd_stderr_out_of_memory_buffers.txt" => {
                include_str!("../../tests/fixtures/sd_stderr_out_of_memory_buffers.txt")
            }
            "sd_stderr_device_lost.txt" => {
                include_str!("../../tests/fixtures/sd_stderr_device_lost.txt")
            }
            "sd_stderr_missing_dll.txt" => {
                include_str!("../../tests/fixtures/sd_stderr_missing_dll.txt")
            }
            "sd_stderr_bad_model.txt" => {
                include_str!("../../tests/fixtures/sd_stderr_bad_model.txt")
            }
            other => panic!("no fixture {other}"),
        };
        text.lines().map(str::to_owned).collect()
    }

    #[test]
    fn failures_are_named() {
        assert_eq!(
            classify(&fixture("sd_stderr_out_of_memory_vulkan.txt"), Some(1)),
            SdFailure::OutOfMemory
        );
        assert_eq!(
            classify(&fixture("sd_stderr_device_lost.txt"), Some(1)),
            SdFailure::DeviceLost
        );
        assert_eq!(
            classify(&fixture("sd_stderr_missing_dll.txt"), Some(-1073741515)),
            SdFailure::MissingLibrary("ggml-vulkan.dll".into())
        );
        assert_eq!(
            classify(&fixture("sd_stderr_bad_model.txt"), Some(1)),
            SdFailure::BadModel
        );
        assert_eq!(
            classify(&[], Some(139)),
            SdFailure::Crashed { code: Some(139) }
        );
    }

    #[test]
    fn memory_failures_in_this_builds_words_are_named() {
        for line in [
            "[E] model manager alloc params backend buffer failed",
            "[E] model manager alloc compute params backend buffer failed",
            "[E] diffusion graph allocation failed",
            "[W] execution engine cannot make enough memory available on Vulkan0",
            "[E] memory allocation of size 2147483648 failed",
        ] {
            assert_eq!(
                classify(&[line.into()], Some(1)),
                SdFailure::OutOfMemory,
                "{line}"
            );
        }
        assert_eq!(
            classify(&fixture("sd_stderr_out_of_memory_buffers.txt"), Some(1)),
            SdFailure::OutOfMemory
        );
    }

    #[test]
    fn the_engines_own_tiling_retry_is_not_a_failure() {
        assert_eq!(
            classify(
                &["[W] VAE decode ran out of memory; retrying with vae tiling".into()],
                Some(139)
            ),
            SdFailure::Crashed { code: Some(139) }
        );
        assert_eq!(
            classify(
                &[
                    "[W] VAE decode ran out of memory; retrying with spatial+temporal tiling --- backend_fit.cpp:530".into(),
                    "[E] ggml: ggml_gallocr_reserve_n_impl: failed to allocate Vulkan0 buffer of size 1073741824".into(),
                ],
                Some(1)
            ),
            SdFailure::OutOfMemory
        );
        // The allocator's and Vulkan's words come before the retry line: a run that recovered
        // by tiling and later crashed for another reason did not run out of memory (ruling
        // L3-oom-after-retry).
        assert_eq!(
            classify(
                &[
                    "ggml_vulkan: Device memory allocation of size 1073741824 failed.".into(),
                    "ggml_vulkan: vk::Device::allocateMemory: ErrorOutOfDeviceMemory".into(),
                    "[E] ggml: ggml_gallocr_reserve_n_impl: failed to allocate Vulkan0 buffer of size 1073741824".into(),
                    "[W] VAE decode ran out of memory; retrying with spatial tiling --- backend_fit.cpp:530".into(),
                ],
                Some(139)
            ),
            SdFailure::Crashed { code: Some(139) }
        );
    }

    #[test]
    fn this_builds_words_for_a_wrong_model_file_are_named() {
        // What the pinned build prints for a file that is not a model it can read, or not the
        // model it should be (ruling L3-bad-model-words); matched outside quotes.
        for line in [
            "[W] unknown format /m/leejet/FLUX.2-klein-4B-GGUF/flux-2-klein-4b-Q4_0.gguf --- model_loader.cpp:236",
            "[E] get sd version from file failed: '' --- diffusion_engine.cpp:979",
            "[E] Diffusion model tensor 'model.diffusion_model.img_in.weight' not in model metadata --- model_manager.cpp:780",
            "[E] VAE tensor 'first_stage_model.decoder.conv_in.weight' has wrong shape in model metadata: got [3, 3, 32, 512], expected [3, 3, 16, 512] --- model_manager.cpp:810",
            "[E] invalid model source '/m/Comfy-Org/flux2-klein-4B/split_files/vae/flux2-vae.safetensors': duplicate tensor in model source: decoder.conv_in.weight --- model_loader_files.cpp:108",
        ] {
            assert_eq!(
                classify(&[line.into()], Some(1)),
                SdFailure::BadModel,
                "{line}"
            );
        }
    }

    #[test]
    fn a_missing_library_keeps_its_name() {
        // Windows' loader prints nothing: the exit code alone says it (ruling L3-self-missing-dll).
        assert_eq!(
            classify(&[], Some(-1073741515)),
            SdFailure::MissingLibrary(String::new())
        );
        assert_eq!(
            classify(
                &["sd-cli: error while loading shared libraries: libvulkan.so.1: cannot open shared object file: No such file or directory".into()],
                Some(127)
            ),
            SdFailure::MissingLibrary("libvulkan.so.1".into())
        );
        assert_eq!(
            classify(
                &[
                    "dyld[4242]: Library not loaded: @rpath/libVulkan.1.dylib".into(),
                    "  Referenced from: <5C9E…> /Applications/harnessUI.app/Contents/MacOS/sd-cli"
                        .into(),
                ],
                None
            ),
            SdFailure::MissingLibrary("libVulkan.1.dylib".into())
        );
    }

    #[test]
    fn words_inside_quotes_are_not_read() {
        // Paths and tensor names are printed in quotes; a folder or a crafted name holding
        // a failure's words does not name the failure.
        assert_eq!(
            classify(
                &[
                    "[I] loading diffusion model from '/Users/a/Out of memory/unknown model/m.gguf' --- diffusion_engine.cpp:697".into(),
                    "[I] unknown tensor 'device lost' in model file --- model_loader.cpp:1592".into(),
                    "[I] loading vae from '/Users/a/unknown format/not in model metadata/vae.safetensors' --- diffusion_engine.cpp:768".into(),
                ],
                Some(1)
            ),
            SdFailure::Crashed { code: Some(1) }
        );
        assert_eq!(
            classify(
                &["[E] load tensor failed: tensor 'model.diffusion_model.img_in.weight' not found --- model_loader.cpp:1494".into()],
                Some(1)
            ),
            SdFailure::BadModel
        );
    }

    #[test]
    fn a_lost_device_is_named_before_the_memory_it_could_not_allocate() {
        assert_eq!(
            classify(
                &[
                    "ggml_vulkan: Device memory allocation of size 1048576 failed.".into(),
                    "ggml_vulkan: vk::Device::allocateMemory: ErrorDeviceLost".into(),
                ],
                Some(1)
            ),
            SdFailure::DeviceLost
        );
    }

    #[test]
    fn anything_else_is_a_crash_with_its_exit_code() {
        assert_eq!(
            classify(&["[E] generate failed --- main.cpp:1001".into()], Some(1)),
            SdFailure::Crashed { code: Some(1) }
        );
        assert_eq!(classify(&[], None), SdFailure::Crashed { code: None });
    }
}
