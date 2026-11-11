//! Probing a backend with `llama-server --list-devices` (ARCHITECTURE.md 7.2).
//!
//! The pinned build prints `Available devices:` and then one line per non-CPU device:
//!
//! ```text
//!   Vulkan0: AMD Radeon RX 9070 XT (16304 MiB, 15890 MiB free)
//! ```
//!
//! or `  (none)`, and exits 0. The list also contains integrated GPUs and, on macOS, a
//! `BLAS: Accelerate (0 MiB, 0 MiB free)` row, so rows are matched by name prefix.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::backend::{Backend, BackendKind};
use super::lines::{LineReader, Tail};
use crate::spawn::{self, EnvSpec, SpawnSpec, StdioMode};

/// Lines of output kept from a probe.
const PROBE_TAIL_LINES: usize = 200;

/// Windows `STATUS_DLL_NOT_FOUND` (0xC0000135) as the signed exit code the OS reports.
pub const STATUS_DLL_NOT_FOUND: i32 = 0xC000_0135_u32 as i32;

/// Windows `STATUS_INVALID_IMAGE_FORMAT` (0xC000007B), seen when a DLL has the wrong
/// architecture or a dependency is broken.
pub const STATUS_INVALID_IMAGE_FORMAT: i32 = 0xC000_007B_u32 as i32;

/// One device row of `--list-devices`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// Device name to pass to `--device` (`Vulkan0`, `ROCm1`, `MTL0`).
    pub name: String,
    /// Human-readable description (`AMD Radeon RX 9070 XT`).
    pub description: String,
    /// Total memory in MiB.
    pub total_mib: u64,
    /// Free memory in MiB at probe time.
    pub free_mib: u64,
}

/// Parses `--list-devices` output. Tolerant: unrelated lines, `(none)`, CRLF line endings and
/// extra spaces are ignored, and a description may itself contain parentheses.
pub fn parse_list_devices(stdout: &str) -> Vec<Device> {
    stdout.lines().filter_map(parse_device_line).collect()
}

fn parse_device_line(line: &str) -> Option<Device> {
    let line = line.trim();
    let (name, rest) = line.split_once(':')?;
    let name = name.trim();
    if name.is_empty()
        || name
            .chars()
            .any(|c| c.is_whitespace() || c == '(' || c == ')')
    {
        return None;
    }
    let rest = rest.trim().strip_suffix(')')?;
    let open = rest.rfind('(')?;
    let description = rest[..open].trim();
    let (total, free) = rest[open + 1..].split_once(',')?;
    let total_mib = parse_mib(total.trim().strip_suffix("MiB")?)?;
    let free_mib = parse_mib(free.trim().strip_suffix("MiB free")?)?;
    Some(Device {
        name: name.to_owned(),
        description: description.to_owned(),
        total_mib,
        free_mib,
    })
}

fn parse_mib(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

/// The backend a device belongs to, by name prefix (`Vulkan`, `CUDA`, `ROCm`, `MTL`)
/// followed by digits. `None` for anything else, such as `BLAS`.
pub fn backend_of(device_name: &str) -> Option<BackendKind> {
    [
        BackendKind::Vulkan,
        BackendKind::Cuda,
        BackendKind::Rocm,
        BackendKind::Metal,
    ]
    .into_iter()
    .find(|kind| {
        kind.device_prefix()
            .and_then(|prefix| device_name.strip_prefix(prefix))
            .is_some_and(|index| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// Why a probe marked a backend unusable. The reason is kept for the UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProbeError {
    /// The server could not be started at all.
    #[error("could not start llama-server: {message}")]
    Start {
        /// The OS error.
        message: String,
    },
    /// No answer within the timeout; the process tree was killed.
    #[error("llama-server --list-devices did not finish within {seconds} s")]
    Timeout {
        /// The timeout that elapsed, in whole seconds.
        seconds: u64,
    },
    /// A Windows runtime DLL is missing or broken (`STATUS_DLL_NOT_FOUND` or
    /// `STATUS_INVALID_IMAGE_FORMAT`): the server exited before printing anything.
    #[error("a runtime library llama-server needs is missing (exit code {code:#x})")]
    MissingRuntimeDll {
        /// The raw exit code.
        code: i32,
    },
    /// The server exited with a failure status.
    #[error("llama-server --list-devices failed with exit code {code:?}")]
    NonZeroExit {
        /// Exit code, or `None` when killed by a signal.
        code: Option<i32>,
        /// The last lines of its output.
        output_tail: Vec<String>,
    },
    /// The server ran but listed no device of this backend, which is what a missing or
    /// outdated driver looks like.
    #[error("no {backend} device found")]
    NoMatchingDevice {
        /// The backend that was probed.
        backend: BackendKind,
    },
}

/// The outcome of probing one backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResult {
    /// The backend that was probed.
    pub backend_kind: BackendKind,
    /// Devices of this backend only. For [`BackendKind::Cpu`] a single synthetic `CPU`
    /// device when the server ran. Empty on error.
    pub devices: Vec<Device>,
    /// Why the backend is unusable, if it is.
    pub error: Option<ProbeError>,
    /// The last lines the server printed (standard output, then standard error), for the
    /// diagnostics report: "no device found" alone does not say why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output: Vec<String>,
}

impl ProbeResult {
    /// Whether the backend passed the probe.
    pub fn is_usable(&self) -> bool {
        self.error.is_none() && !self.devices.is_empty()
    }
}

/// The synthetic device reported for the CPU backend. Its memory figures are 0 because
/// `--list-devices` does not report system RAM; the model manager reads that elsewhere.
pub fn cpu_device() -> Device {
    Device {
        name: "CPU".to_owned(),
        description: "CPU".to_owned(),
        total_mib: 0,
        free_mib: 0,
    }
}

/// Runs `<server> --list-devices` for `backend` with its runtime directory as working
/// directory and a scrubbed environment, and classifies the result.
///
/// For GPU backends, rows of this backend's prefix with no memory are dropped, and an
/// empty result is [`ProbeError::NoMatchingDevice`].
pub async fn probe(backend: &Backend, timeout: Duration) -> ProbeResult {
    probe_with_env(backend, timeout, EnvSpec::minimal()).await
}

/// [`probe`] with a chosen environment. The app always probes and launches with the
/// minimal one; diagnostics repeat a probe that found nothing with a wider one, to tell
/// whether a driver needs a variable the minimal environment leaves out.
pub async fn probe_with_env(backend: &Backend, timeout: Duration, env: EnvSpec) -> ProbeResult {
    let fail = |error| ProbeResult {
        backend_kind: backend.kind,
        devices: Vec::new(),
        error: Some(error),
        output: Vec::new(),
    };
    let spec = SpawnSpec::new(&backend.server, &backend.dir)
        // `-v` first: llama-server then logs which backend libraries it loaded and why one
        // could not be loaded. Without it a Vulkan library that fails to load is silent and
        // the list is just "(none)". The device rows are the same either way.
        .arg("-v")
        .arg("--list-devices")
        .env(env)
        .stdin(StdioMode::Null)
        .stdout(StdioMode::Piped)
        .stderr(StdioMode::Piped);
    let mut child = match spawn::spawn(spec) {
        Ok(child) => child,
        Err(error) => {
            return fail(ProbeError::Start {
                message: error.to_string(),
            });
        }
    };
    let stdout = child.take_stdout();
    let stderr = child.take_stderr();
    let collect_stdout = async move {
        let mut lines = Vec::new();
        if let Some(stdout) = stdout {
            let mut reader = LineReader::new(stdout);
            while let Some(line) = reader.next_line().await {
                if lines.len() < PROBE_TAIL_LINES {
                    lines.push(line);
                }
            }
        }
        lines
    };
    let collect_stderr = async move {
        let mut tail = Tail::new(PROBE_TAIL_LINES);
        if let Some(stderr) = stderr {
            let mut reader = LineReader::new(stderr);
            while let Some(line) = reader.next_line().await {
                tail.push(line);
            }
        }
        tail.to_vec()
    };
    let run = async {
        let (stdout, stderr, status) = tokio::join!(collect_stdout, collect_stderr, child.wait());
        (stdout, stderr, status)
    };
    let (stdout, stderr, status) = match tokio::time::timeout(timeout, run).await {
        Ok(done) => done,
        Err(_) => {
            let _ = child.kill().await;
            return fail(ProbeError::Timeout {
                seconds: timeout.as_secs(),
            });
        }
    };
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            return fail(ProbeError::Start {
                message: error.to_string(),
            });
        }
    };
    if !status.success() {
        let code = status.code();
        if let Some(code) =
            code.filter(|&c| c == STATUS_DLL_NOT_FOUND || c == STATUS_INVALID_IMAGE_FORMAT)
        {
            return fail(ProbeError::MissingRuntimeDll { code });
        }
        let mut output_tail = stdout;
        output_tail.extend(stderr);
        let skip = output_tail.len().saturating_sub(PROBE_TAIL_LINES);
        output_tail.drain(..skip);
        return fail(ProbeError::NonZeroExit { code, output_tail });
    }
    let mut result = classify_devices(backend.kind, &stdout.join("\n"));
    let mut output = stdout;
    output.extend(stderr);
    let skip = output.len().saturating_sub(PROBE_TAIL_LINES);
    output.drain(..skip);
    result.output = output;
    result
}

/// Turns successful `--list-devices` output into a [`ProbeResult`] for `kind`.
pub fn classify_devices(kind: BackendKind, stdout: &str) -> ProbeResult {
    if kind.device_prefix().is_none() {
        return ProbeResult {
            backend_kind: kind,
            devices: vec![cpu_device()],
            error: None,
            output: Vec::new(),
        };
    }
    let devices: Vec<Device> = parse_list_devices(stdout)
        .into_iter()
        .filter(|device| backend_of(&device.name) == Some(kind) && device.total_mib > 0)
        .collect();
    let error = devices
        .is_empty()
        .then_some(ProbeError::NoMatchingDevice { backend: kind });
    ProbeResult {
        backend_kind: kind,
        devices,
        error,
        output: Vec::new(),
    }
}

/// Picks the device to run on: a discrete GPU over an integrated one, then the most total
/// memory, then the lowest index.
///
/// "Most memory" alone is not enough: on a Ryzen desktop the HIP runtime lists the
/// integrated GPU with tens of GB of shared memory, more than the discrete card
/// (rocm.recheck.md), and running on it is slow or crashes.
pub fn pick_device(devices: &[Device]) -> Option<&Device> {
    devices
        .iter()
        .enumerate()
        .max_by_key(|(index, device)| {
            (
                !looks_integrated(&device.description),
                device.total_mib,
                std::cmp::Reverse(*index),
            )
        })
        .map(|(_, device)| device)
}

/// Whether a device description names an integrated GPU. Discrete cards carry a model
/// name (`Radeon RX 9070 XT`, `Arc A770`); integrated ones on current CPUs do not.
pub fn looks_integrated(description: &str) -> bool {
    let d = description.to_ascii_lowercase();
    let d = d.replace("(tm)", "").replace("(r)", "");
    let d = d.split_whitespace().collect::<Vec<_>>().join(" ");
    const EXACT: &[&str] = &[
        "amd radeon graphics",
        "radeon graphics",
        "amd radeon(tm) graphics",
        "intel graphics",
        "intel arc graphics",
    ];
    const CONTAINS: &[&str] = &[
        "radeon vega",
        "uhd graphics",
        "intel hd graphics",
        "iris",
        "integrated",
        "780m",
        "760m",
        "680m",
        "890m",
        "880m",
    ];
    EXACT.contains(&d.as_str()) || CONTAINS.iter().any(|needle| d.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe that finds no device keeps what the server printed, on both streams: it is
    /// the only way to tell a missing driver from a driver that failed to load.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_probe_keeps_what_the_server_printed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let server = dir.path().join("llama-server");
        std::fs::write(
            &server,
            "#!/bin/sh\necho \"args: $*\" >&2\necho 'load_backend: failed to load ggml-vulkan.dll' >&2\necho 'Available devices:'\necho '  (none)'\n",
        )
        .unwrap();
        std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).unwrap();
        let backend = Backend {
            kind: BackendKind::Vulkan,
            dir: dir.path().to_path_buf(),
            server,
            fit_params: None,
            build: "test".into(),
            source: crate::llama::backend::BackendSource::Installed,
        };
        let result = probe(&backend, Duration::from_secs(20)).await;
        assert_eq!(
            result.error,
            Some(ProbeError::NoMatchingDevice {
                backend: BackendKind::Vulkan
            })
        );
        assert!(
            result
                .output
                .iter()
                .any(|l| l.contains("Available devices:")),
            "{:?}",
            result.output
        );
        assert!(
            result
                .output
                .iter()
                .any(|l| l.contains("load_backend: failed to load ggml-vulkan.dll")),
            "{:?}",
            result.output
        );
        // Verbose, so the server says which backend libraries it loaded or could not load;
        // without it a failed load is silent and the list is just "(none)".
        assert!(
            result.output.iter().any(|l| l == "args: -v --list-devices"),
            "{:?}",
            result.output
        );
    }

    #[test]
    fn parses_rows_and_ignores_noise() {
        let out = "load_backend: loaded CPU backend from /x/libggml-cpu.so\r\n\
                   Available devices:\r\n\
                   \x20 Vulkan0: AMD Radeon RX 9070 XT (16304 MiB, 15890 MiB free)\r\n\
                   \x20 Vulkan1:   AMD Radeon(TM) Graphics   (50246 MiB, 49000 MiB free)  \r\n\
                   \x20 BLAS: Accelerate (0 MiB, 0 MiB free)\n";
        let devices = parse_list_devices(out);
        assert_eq!(
            devices,
            vec![
                Device {
                    name: "Vulkan0".into(),
                    description: "AMD Radeon RX 9070 XT".into(),
                    total_mib: 16304,
                    free_mib: 15890
                },
                Device {
                    name: "Vulkan1".into(),
                    description: "AMD Radeon(TM) Graphics".into(),
                    total_mib: 50246,
                    free_mib: 49000
                },
                Device {
                    name: "BLAS".into(),
                    description: "Accelerate".into(),
                    total_mib: 0,
                    free_mib: 0
                },
            ]
        );
    }

    #[test]
    fn parses_none_and_garbage_as_empty() {
        assert!(parse_list_devices("Available devices:\n  (none)\n").is_empty());
        assert!(parse_list_devices("").is_empty());
        for junk in [
            "  Vulkan0: no memory figures",
            "  Vulkan0: X (abc MiB, 1 MiB free)",
            "  Vulkan0: X (1 MiB, 1 MiB)",
            "  : X (1 MiB, 1 MiB free)",
            "  two words: X (1 MiB, 1 MiB free)",
            "  Vulkan0: X (1 MiB 1 MiB free)",
        ] {
            assert!(parse_list_devices(junk).is_empty(), "{junk:?}");
        }
    }

    #[test]
    fn description_keeps_inner_parentheses() {
        let devices = parse_list_devices(
            "  ROCm0: AMD Radeon(TM) Graphics (gfx1036) (50246 MiB, 49000 MiB free)",
        );
        assert_eq!(devices[0].description, "AMD Radeon(TM) Graphics (gfx1036)");
        assert_eq!(devices[0].total_mib, 50246);
    }

    #[test]
    fn real_cpu_build_output_has_no_devices() {
        // Captured from the pinned b12110 Linux CPU build.
        assert!(parse_list_devices("Available devices:\n  (none)\n").is_empty());
        let result = classify_devices(BackendKind::Vulkan, "Available devices:\n  (none)\n");
        assert_eq!(
            result.error,
            Some(ProbeError::NoMatchingDevice {
                backend: BackendKind::Vulkan
            })
        );
        assert!(!result.is_usable());
    }

    #[test]
    fn backend_of_matches_prefix_and_index() {
        assert_eq!(backend_of("Vulkan0"), Some(BackendKind::Vulkan));
        assert_eq!(backend_of("CUDA12"), Some(BackendKind::Cuda));
        assert_eq!(backend_of("ROCm1"), Some(BackendKind::Rocm));
        assert_eq!(backend_of("MTL0"), Some(BackendKind::Metal));
        assert_eq!(backend_of("BLAS"), None);
        assert_eq!(backend_of("Vulkan"), None);
        assert_eq!(backend_of("VulkanX"), None);
        assert_eq!(backend_of("CPU"), None);
    }

    #[test]
    fn classify_filters_by_backend_and_drops_zero_memory_rows() {
        let out = "  Vulkan0: GPU (8000 MiB, 7000 MiB free)\n  CUDA0: NV (12000 MiB, 11000 MiB free)\n  Vulkan1: Broken (0 MiB, 0 MiB free)\n";
        let vulkan = classify_devices(BackendKind::Vulkan, out);
        assert!(vulkan.is_usable());
        assert_eq!(vulkan.devices.len(), 1);
        assert_eq!(vulkan.devices[0].name, "Vulkan0");
        let cuda = classify_devices(BackendKind::Cuda, out);
        assert_eq!(cuda.devices[0].name, "CUDA0");
        let rocm = classify_devices(BackendKind::Rocm, out);
        assert_eq!(
            rocm.error,
            Some(ProbeError::NoMatchingDevice {
                backend: BackendKind::Rocm
            })
        );
    }

    #[test]
    fn cpu_always_gets_one_synthetic_device() {
        let cpu = classify_devices(BackendKind::Cpu, "Available devices:\n  (none)\n");
        assert_eq!(cpu.devices, vec![cpu_device()]);
        assert!(cpu.is_usable());
    }

    #[test]
    fn pick_prefers_discrete_over_a_bigger_integrated_gpu() {
        let devices = parse_list_devices(
            "  ROCm0: AMD Radeon(TM) Graphics (50246 MiB, 49000 MiB free)\n  ROCm1: AMD Radeon RX 9070 XT (16304 MiB, 16000 MiB free)\n",
        );
        assert_eq!(
            pick_device(&devices).map(|d| d.name.as_str()),
            Some("ROCm1")
        );
    }

    #[test]
    fn pick_prefers_more_memory_then_lower_index() {
        let devices = parse_list_devices(
            "  Vulkan0: NVIDIA GeForce RTX 3060 (12288 MiB, 11000 MiB free)\n  Vulkan1: NVIDIA GeForce RTX 4090 (24564 MiB, 24000 MiB free)\n  Vulkan2: NVIDIA GeForce RTX 4090 (24564 MiB, 24000 MiB free)\n",
        );
        assert_eq!(
            pick_device(&devices).map(|d| d.name.as_str()),
            Some("Vulkan1")
        );
        assert_eq!(pick_device(&[]), None);
        let only_igpu =
            parse_list_devices("  Vulkan0: Intel(R) UHD Graphics 770 (8000 MiB, 7000 MiB free)\n");
        assert_eq!(
            pick_device(&only_igpu).map(|d| d.name.as_str()),
            Some("Vulkan0")
        );
    }

    #[test]
    fn integrated_heuristic() {
        for integrated in [
            "AMD Radeon(TM) Graphics",
            "AMD Radeon Graphics",
            "Intel(R) UHD Graphics 770",
            "Intel(R) Iris(R) Xe Graphics",
            "Intel(R) Arc(TM) Graphics",
            "AMD Radeon 780M Graphics",
        ] {
            assert!(looks_integrated(integrated), "{integrated}");
        }
        for discrete in [
            "AMD Radeon RX 9070 XT",
            "NVIDIA GeForce RTX 4090",
            "Intel(R) Arc(TM) A770 Graphics",
            "Apple M5",
            "AMD Radeon Pro W7900",
        ] {
            assert!(!looks_integrated(discrete), "{discrete}");
        }
    }

    #[test]
    fn windows_dll_codes() {
        assert_eq!(STATUS_DLL_NOT_FOUND, -1_073_741_515);
        assert_eq!(STATUS_INVALID_IMAGE_FORMAT, -1_073_741_701);
    }
}
