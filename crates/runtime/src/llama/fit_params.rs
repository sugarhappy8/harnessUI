//! llama.cpp's own memory measurement before a load: `llama-fit-params -fitp on`
//! (item 6, sections 0 and 4.3).
//!
//! The pinned build's `llama-fit-params` loads a model's header and tensor table with
//! nothing allocated (`no_alloc`, `load_mode = NONE` in `common/fit.cpp` at b12110) and
//! prints, for exactly the arguments it is given, the memory each device, the computer and
//! every buffer type would need: model weights, conversation memory and working space, in
//! MiB. Measured on the build machine: 0.2 to 0.55 s per run and a peak of 65 MiB for a
//! 610 MiB model, so the weights are never read (the contract test
//! `fit_params_measures_without_reading_the_weights` checks it on every run).
//!
//! What this module does:
//! - [`measurable`]: only the pinned build is measured (I6-15). The installed runtime and
//!   the CUDA pack carry the lock's tag; the ROCm pack (another commit, and no fit-params
//!   file) and every other build get the app's own estimate; a development folder counts
//!   only in a debug build.
//! - [`build_measure_args`]: fixed words, numbers, the model path and the probe's device
//!   name; nothing a user or a model types.
//! - [`measure`]: one run through [`crate::spawn`] in the backend's folder with the
//!   backend's scrubbed environment (which carries `GGML_NO_BACKTRACE=1`, so a crash never
//!   starts a debugger), stdin closed, the last [`MAX_STDOUT_LINES`] stdout lines of at
//!   most 16 KiB kept, killed on the timeout or on cancel.
//! - [`parse_measure_output`]: with `--log-jsonl` the whole log stream is on stdout as JSON
//!   lines; exactly one record of type `fit_memory_breakdown` must be there, with values
//!   in range (section 2.0). The plain `Host 157 448 312` line is ignored: it leaves out
//!   repacked weights (`CPU_REPACK`, `AMX`), which the JSON record lists.
//!
//! One run at a time and the caching of results and failures are the app's (item 6,
//! I6-14): a newer request cancels the older one through the [`CancellationToken`].

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::backend::{Backend, BackendSource};
use super::launch::{
    ExpertsOnCpu, FlashAttn, HIP_VISIBLE_DEVICES_ENV, SAVE_MEMORY_CACHE_TYPE, layers_word,
};
use super::lines::{LineReader, Tail};
use super::lock;
use super::protocol::{
    MAX_MEMORY_ROWS, MAX_ROW_DESCRIPTION_CHARS, MAX_ROW_NAME_CHARS, MemoryRow, MemoryRowKind,
};
use crate::spawn::{self, EnvSpec, SpawnSpec, StdioMode};

/// How long one measurement may take before it is killed: two minutes (item 6, I6-14).
///
/// A run takes under a second on Linux and Windows. On macOS every llama.cpp program
/// compiles its Metal library from source when it starts (`ggml_backend_metal_reg` at
/// b12110, `--device none` included), and macOS keeps the compiled result for later runs,
/// so the first run after an install or update is slow. On a CI Mac (a virtual machine
/// with 3 processors and no real graphics chip), with the tests of one file running side
/// by side, one first run was still going at 30 s, the old limit, and was stopped, while
/// the runs after the first took about 2 s; a second run on a broken file, which crashes,
/// still took more than 20 s. Those runs shared the machine, so the log gives no time for
/// one compile on its own. Two minutes is four times the 30 s at which that first run was
/// stopped. A longer limit costs nothing in normal use: the estimate stays shown while a
/// run goes on, and a newer request still cancels it at once.
pub const FIT_PARAMS_TIMEOUT: Duration = Duration::from_secs(120);

/// Most stdout lines kept from one run, the last ones (each cut at 16 KiB by the line
/// reader); earlier lines are drained and dropped.
pub const MAX_STDOUT_LINES: usize = 512;

/// Log records and stderr lines kept for the one log entry a run writes.
pub const LOG_LINES_KEPT: usize = 40;

/// Longest log message kept, in characters (section 2.0).
pub const MAX_LOG_MESSAGE_CHARS: usize = 300;

/// Largest value a fit-params row may carry, in MiB (1 PiB, section 2.0). A larger one, a
/// negative one or a fraction makes the whole result [`FitParamsError::NoReport`].
pub const MAX_ROW_MIB: u64 = 1 << 30;

/// Bytes in a MiB.
const MIB: u64 = 1_048_576;

/// Which layers go on the device in a measurement (`-ngl`); the device is
/// [`MeasureConfig::device`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasureLayers {
    /// Every layer (`-ngl all`).
    All,
    /// This many layers, counted from the back (`-ngl N`; `-ngl 0` for none).
    Count(u32),
}

/// What to measure: one placement of one model at one context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasureConfig {
    /// The model file (`-m`). Must be valid Unicode ([`measure`] refuses it otherwise).
    pub model: PathBuf,
    /// Context length (`-c`).
    pub ctx: u32,
    /// The probe's device name (`--device`); `None` or empty for the processor (`none`).
    pub device: Option<String>,
    /// Layers on the device (`-ngl`).
    pub layers: MeasureLayers,
    /// Expert parts on the processor (`--cpu-moe`, `--n-cpu-moe K`).
    pub experts_on_cpu: ExpertsOnCpu,
    /// Conversation memory in computer memory (`-nkvo`).
    pub kv_in_computer_memory: bool,
    /// Save memory: `-fa on -ctk q8_0 -ctv q8_0`, whatever [`flash_attn`](Self::flash_attn)
    /// says.
    pub save_memory: bool,
    /// Fast attention (`-fa on` / `-fa off`; nothing for Auto) when Save memory is off.
    pub flash_attn: FlashAttn,
    /// `HIP_VISIBLE_DEVICES` for ROCm (set in the environment, never on the command line).
    pub hip_visible_devices: Option<String>,
}

/// Why a measurement gave no figures.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FitParamsError {
    /// This backend cannot be measured ([`measurable`] is false).
    #[error("this download cannot work out exact figures")]
    NoTool,
    /// The program could not be started, or the settings cannot be passed to it.
    #[error("llama-fit-params could not start: {message}")]
    Start {
        /// What went wrong.
        message: String,
    },
    /// The run took longer than its timeout and was killed.
    #[error("llama-fit-params took too long")]
    Timeout,
    /// The program exited with an error (`None`: killed by a signal, a crash on Unix).
    #[error("llama-fit-params failed (exit code {code:?})")]
    Failed {
        /// The exit code.
        code: Option<i32>,
    },
    /// The program ended normally but its output held no usable memory record.
    #[error("llama-fit-params reported no usable figures")]
    NoReport,
    /// A newer request cancelled the run, which was killed.
    #[error("the measurement was cancelled")]
    Cancelled,
}

/// Whether `backend` may be measured (item 6, I6-15): it has a fit-params file and is the
/// pinned build (its `build` equals the lock's `llama_cpp.tag`: the installed runtime and
/// the CUDA pack), or it is a development folder in a debug build. The ROCm pack (no
/// fit-params file, another commit), other builds and a development folder in a release
/// build get the app's estimate instead.
pub fn measurable(backend: &Backend) -> bool {
    measurable_with(
        backend,
        &lock::embedded().llama_cpp.tag,
        cfg!(debug_assertions),
    )
}

/// [`measurable`] against an explicit pinned tag and build profile (for tests).
pub fn measurable_with(backend: &Backend, pinned_tag: &str, debug_build: bool) -> bool {
    if backend.fit_params.is_none() {
        return false;
    }
    let pinned = !pinned_tag.is_empty() && backend.build == pinned_tag;
    let dev_folder = debug_build && backend.source == BackendSource::DevOverride;
    pinned || dev_folder
}

/// The exact argument list for `cfg`:
///
/// ```text
/// -m M -c C -np 1 --device D (-ngl all | -ngl N | -ngl 0) [--cpu-moe | --n-cpu-moe K]
/// [-nkvo] [-fa on | -fa off] [-ctk q8_0 -ctv q8_0] --fit off -fitp on --log-jsonl --offline
/// ```
///
/// `D` is the device, or `none` for none or an empty name. `FirstLayers(0)` adds no
/// expert flag. Counts have the server's limits: above `launch::MAX_LAYER_COUNT` a layer
/// count is `all` and an expert layer count `--cpu-moe`. Save memory gives `-fa on -ctk q8_0 -ctv q8_0` (the server's words); without
/// it fast attention gives `-fa on`, `-fa off` or nothing. `--fit off` measures the
/// arguments as they are; `-fitp on` prints the measurement; `--log-jsonl` makes it a JSON
/// record. The path is converted lossily: [`measure`] refuses a path that is not valid
/// Unicode before it gets here.
pub fn build_measure_args(cfg: &MeasureConfig) -> Vec<String> {
    let device = cfg
        .device
        .as_deref()
        .filter(|d| !d.is_empty())
        .unwrap_or("none");
    let mut args: Vec<String> = vec![
        "-m".to_owned(),
        cfg.model.to_string_lossy().into_owned(),
        "-c".to_owned(),
        cfg.ctx.to_string(),
        "-np".to_owned(),
        "1".to_owned(),
        "--device".to_owned(),
        device.to_owned(),
        "-ngl".to_owned(),
        match cfg.layers {
            MeasureLayers::All => "all".to_owned(),
            MeasureLayers::Count(n) => layers_word(n),
        },
    ];
    args.extend(cfg.experts_on_cpu.flag().unwrap_or_default());
    if cfg.kv_in_computer_memory {
        args.push("-nkvo".to_owned());
    }
    if cfg.save_memory {
        args.extend(
            [
                "-fa",
                "on",
                "-ctk",
                SAVE_MEMORY_CACHE_TYPE,
                "-ctv",
                SAVE_MEMORY_CACHE_TYPE,
            ]
            .map(str::to_owned),
        );
    } else {
        match cfg.flash_attn {
            FlashAttn::Auto => {}
            FlashAttn::On => args.extend(["-fa", "on"].map(str::to_owned)),
            FlashAttn::Off => args.extend(["-fa", "off"].map(str::to_owned)),
        }
    }
    args.extend(["--fit", "off", "-fitp", "on", "--log-jsonl", "--offline"].map(str::to_owned));
    args
}

/// The environment of a measurement: the backend's own ([`Backend::base_env`], with
/// `GGML_NO_BACKTRACE=1` and, on Windows, the bundled Vulkan runtime on `PATH`) plus
/// `HIP_VISIBLE_DEVICES` for ROCm. No `LLAMA_ARG_*`, no key.
pub fn measure_env(backend: &Backend, cfg: &MeasureConfig) -> EnvSpec {
    let env = backend.base_env();
    match &cfg.hip_visible_devices {
        Some(devices) => env.set(HIP_VISIBLE_DEVICES_ENV, devices.clone()),
        None => env,
    }
}

/// The rows of the one `fit_memory_breakdown` record in `stdout`, in bytes.
///
/// Every line is read as a JSON object; anything else (the plain `Host 157 448 312` line,
/// a cut line) is ignored. Exactly one object whose top-level `type` is exactly
/// `fit_memory_breakdown`, with `data.unit` exactly `MiB` and 1 to [`MAX_MEMORY_ROWS`]
/// rows, is required: none, or two or more, give [`FitParamsError::NoReport`]. Text that
/// looks like such a record inside a log message is just text.
///
/// Rows (section 2.0): kinds are mapped explicitly (`device`, `host`, `buffer_type`) and
/// any other kind skips the row; only `total`, `free`, `self`, `model`, `context` and
/// `compute` are read, each a whole number from 0 to [`MAX_ROW_MIB`] when present (a
/// negative value, a fraction, a larger number or another type makes the whole result
/// `NoReport`), a missing `model`, `context` or `compute` counts as 0, and other fields
/// (`unaccounted`, which can be negative) are ignored. Names and descriptions are escaped
/// and cut. No row of a known kind is `NoReport`.
pub fn parse_measure_output(stdout: &[String]) -> Result<Vec<MemoryRow>, FitParamsError> {
    let mut records = stdout
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| value.get("type").and_then(Value::as_str) == Some("fit_memory_breakdown"));
    let (Some(record), None) = (records.next(), records.next()) else {
        return Err(FitParamsError::NoReport);
    };
    let data = record.get("data").ok_or(FitParamsError::NoReport)?;
    if data.get("unit").and_then(Value::as_str) != Some("MiB") {
        return Err(FitParamsError::NoReport);
    }
    let rows = data
        .get("rows")
        .and_then(Value::as_array)
        .ok_or(FitParamsError::NoReport)?;
    if rows.is_empty() || rows.len() > MAX_MEMORY_ROWS {
        return Err(FitParamsError::NoReport);
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some(row) = measure_row(row)? {
            out.push(row);
        }
    }
    if out.is_empty() {
        return Err(FitParamsError::NoReport);
    }
    Ok(out)
}

/// One fit-params row in bytes, `Ok(None)` to skip it (an unknown kind), or `NoReport` for
/// a value out of range.
fn measure_row(row: &Value) -> Result<Option<MemoryRow>, FitParamsError> {
    if !row.is_object() {
        return Err(FitParamsError::NoReport);
    }
    let mib = |key: &str| -> Result<Option<u64>, FitParamsError> {
        match row.get(key) {
            None => Ok(None),
            Some(value) => match value.as_u64() {
                Some(n) if n <= MAX_ROW_MIB => {
                    n.checked_mul(MIB).map(Some).ok_or(FitParamsError::NoReport)
                }
                _ => Err(FitParamsError::NoReport),
            },
        }
    };
    // Every number is checked, also for a row that is then skipped and for `self`, which
    // is not kept (it is the sum of the three parts).
    let total = mib("total")?;
    let free = mib("free")?;
    let _self_mib = mib("self")?;
    let model = mib("model")?.unwrap_or(0);
    let context = mib("context")?.unwrap_or(0);
    let compute = mib("compute")?.unwrap_or(0);
    let Some(kind) = row
        .get("kind")
        .and_then(Value::as_str)
        .and_then(MemoryRowKind::from_llama)
    else {
        return Ok(None);
    };
    let text = |key: &str, cap: usize| {
        escape_text(
            row.get(key).and_then(Value::as_str).unwrap_or_default(),
            cap,
        )
    };
    let device = kind == MemoryRowKind::Device;
    Ok(Some(MemoryRow {
        kind,
        name: text("name", MAX_ROW_NAME_CHARS),
        description: if device {
            text("description", MAX_ROW_DESCRIPTION_CHARS)
        } else {
            String::new()
        },
        total_bytes: if device { total } else { None },
        free_bytes: if device { free } else { None },
        model_bytes: model,
        context_bytes: context,
        compute_bytes: compute,
    }))
}

/// Measures `cfg` on `backend` with `llama-fit-params` (section 4.3): the rows of its one
/// memory record, in bytes.
///
/// - [`FitParamsError::NoTool`] at once when the backend is not [`measurable`].
/// - [`FitParamsError::Start`] when the model path is not valid Unicode (launch refuses
///   such paths too; never a lossy conversion) or the program cannot be started.
/// - [`FitParamsError::Timeout`] / [`FitParamsError::Cancelled`]: the run is killed (its
///   whole process tree) when `timeout` passes or `cancel` fires.
/// - [`FitParamsError::Failed`] when it exits with an error; [`FitParamsError::NoReport`]
///   when it ends normally without one usable record ([`parse_measure_output`]).
/// - A run that ends normally and printed the plain measurement line but no record lost
///   its record at exit ([`record_was_lost`]); it is run once more, with what is left of
///   `timeout`. A second loss is `NoReport`.
///
/// Each run that started writes one log entry: the last [`LOG_LINES_KEPT`] warning and
/// error records of its log stream and the last [`LOG_LINES_KEPT`] stderr lines, escaped
/// and cut ([`log_records`]). On a broken file the reason is in those records, not on
/// stderr, when llama.cpp's log thread wrote it before the abort (most runs; in 6 of 40
/// runs on the build machine it did not, and stdout held only the first info record).
pub async fn measure(
    backend: &Backend,
    cfg: &MeasureConfig,
    timeout: Duration,
    cancel: CancellationToken,
) -> Result<Vec<MemoryRow>, FitParamsError> {
    if !measurable(backend) {
        return Err(FitParamsError::NoTool);
    }
    let deadline = tokio::time::Instant::now() + timeout;
    let first = run(backend, cfg, timeout, cancel.clone()).await;
    if !first.record_lost {
        return first.result;
    }
    let left = deadline.saturating_duration_since(tokio::time::Instant::now());
    if left.is_zero() {
        return Err(FitParamsError::Timeout);
    }
    run(backend, cfg, left, cancel).await.result
}

/// What one run gave: its result, and whether it ended normally with the plain
/// measurement line but no record ([`record_was_lost`]; the result is then `NoReport`).
struct Run {
    result: Result<Vec<MemoryRow>, FitParamsError>,
    record_lost: bool,
}

impl From<FitParamsError> for Run {
    fn from(error: FitParamsError) -> Self {
        Self {
            result: Err(error),
            record_lost: false,
        }
    }
}

/// [`measure`] without the [`measurable`] check: one run.
async fn run(
    backend: &Backend,
    cfg: &MeasureConfig,
    timeout: Duration,
    cancel: CancellationToken,
) -> Run {
    let Some(program) = &backend.fit_params else {
        return FitParamsError::NoTool.into();
    };
    if cfg.model.to_str().is_none() {
        return FitParamsError::Start {
            message: "the model path is not valid Unicode".into(),
        }
        .into();
    }
    if cancel.is_cancelled() {
        return FitParamsError::Cancelled.into();
    }
    let spec = SpawnSpec::new(program, &backend.dir)
        .args(build_measure_args(cfg))
        .env(measure_env(backend, cfg))
        .stdin(StdioMode::Null)
        .stdout(StdioMode::Piped)
        .stderr(StdioMode::Piped);
    let mut child = match spawn::spawn(spec) {
        Ok(child) => child,
        Err(error) => {
            return FitParamsError::Start {
                message: error.to_string(),
            }
            .into();
        }
    };
    let stdout = child.take_stdout();
    let stderr = child.take_stderr();
    // The last lines are kept: the record and the plain line come at the end of a run.
    let read_stdout = async move {
        let mut tail = Tail::new(MAX_STDOUT_LINES);
        if let Some(stdout) = stdout {
            let mut reader = LineReader::new(stdout);
            while let Some(line) = reader.next_line().await {
                tail.push(line);
            }
        }
        tail.to_vec()
    };
    let read_stderr = async move {
        let mut tail = Tail::new(LOG_LINES_KEPT);
        if let Some(stderr) = stderr {
            let mut reader = LineReader::new(stderr);
            while let Some(line) = reader.next_line().await {
                tail.push(line);
            }
        }
        tail.to_vec()
    };
    enum Outcome {
        Done(
            Vec<String>,
            Vec<String>,
            std::io::Result<std::process::ExitStatus>,
        ),
        TimedOut,
        Cancelled,
    }
    let outcome = {
        let all = async { tokio::join!(read_stdout, read_stderr, child.wait()) };
        tokio::select! {
            biased;
            () = cancel.cancelled() => Outcome::Cancelled,
            done = tokio::time::timeout(timeout, all) => match done {
                Ok((out, err, status)) => Outcome::Done(out, err, status),
                Err(_) => Outcome::TimedOut,
            },
        }
    };
    let (stdout, stderr, status) = match outcome {
        Outcome::Done(out, err, status) => (out, err, status),
        Outcome::TimedOut => {
            let _ = child.kill().await;
            tracing::warn!(
                seconds = timeout.as_secs(),
                "llama-fit-params took too long and was stopped"
            );
            return FitParamsError::Timeout.into();
        }
        Outcome::Cancelled => {
            let _ = child.kill().await;
            return FitParamsError::Cancelled.into();
        }
    };
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            return FitParamsError::Start {
                message: error.to_string(),
            }
            .into();
        }
    };
    let result = if status.success() {
        parse_measure_output(&stdout)
    } else {
        Err(FitParamsError::Failed {
            code: status.code(),
        })
    };
    let record_lost = result == Err(FitParamsError::NoReport) && record_was_lost(&stdout);
    let (records, stderr_tail) = log_records(&stdout, &stderr);
    match &result {
        Ok(rows) => tracing::debug!(
            rows = rows.len(),
            ?records,
            ?stderr_tail,
            "llama-fit-params measured a placement"
        ),
        Err(_) if record_lost => tracing::debug!(
            ?records,
            ?stderr_tail,
            "llama-fit-params ended before its record was written"
        ),
        Err(error) => tracing::warn!(
            %error,
            ?records,
            ?stderr_tail,
            "llama-fit-params gave no figures"
        ),
    }
    Run {
        result,
        record_lost,
    }
}

/// Whether a run that ended normally printed fit-params' plain measurement line
/// (`Host 157 448 312 `, always the last plain line, after any device lines) without its
/// JSON record: the measurement was done and only the record went missing. llama.cpp
/// queues the record for its log thread and then prints the plain lines and exits, so the
/// process can end before that thread writes it (llama.cpp leaks its logger on purpose
/// and says logs may be lost at exit; seen in 2 runs in 160 on the build machine with four
/// runs side by side). The figures of the plain line are never used: it leaves out
/// repacked weights.
fn record_was_lost(stdout: &[String]) -> bool {
    stdout.iter().any(|line| {
        let words: Vec<&str> = line.trim_end().split(' ').collect();
        words.len() == 4
            && words[0] == "Host"
            && words[1..]
                .iter()
                .all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// What one run leaves for the log (section 4.3): the last [`LOG_LINES_KEPT`] records of
/// `stdout` whose `type` is `log` and whose `level` is `warn` or `error`, their messages
/// escaped and cut to [`MAX_LOG_MESSAGE_CHARS`] characters, and the last
/// [`LOG_LINES_KEPT`] lines of `stderr`, escaped and cut the same way.
pub fn log_records(stdout: &[String], stderr: &[String]) -> (Vec<String>, Vec<String>) {
    let mut records: VecDeque<String> = VecDeque::new();
    for line in stdout {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("log")
            || !matches!(
                value.get("level").and_then(Value::as_str),
                Some("warn" | "error")
            )
        {
            continue;
        }
        let message = value
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim_end_matches(['\r', '\n']);
        if records.len() == LOG_LINES_KEPT {
            records.pop_front();
        }
        records.push_back(escape_text(message, MAX_LOG_MESSAGE_CHARS));
    }
    let skip = stderr.len().saturating_sub(LOG_LINES_KEPT);
    let stderr_tail = stderr[skip..]
        .iter()
        .map(|line| escape_text(line, MAX_LOG_MESSAGE_CHARS))
        .collect();
    (records.into(), stderr_tail)
}

/// Text from llama.cpp, a model file or the system as the app keeps it (section 2.0):
/// every control character (Unicode Cc), bidirectional mark or override (U+061C, U+200E,
/// U+200F, U+202A to U+202E, U+2066 to U+2069) and zero-width or invisible character
/// (U+200B to U+200D, U+2060 to U+2064, U+FEFF, and the rest of `uploader_text`'s set:
/// soft hyphen, fillers, variation selectors, tag characters and others; see `is_hidden`)
/// written as `[U+XXXX]`, then cut to at most `cap` characters; an escape that does not fit
/// whole is left out. Reading stops at the cap, so a huge input costs no more than its first
/// characters.
///
/// The rule of `models::hf::uploader_text`, copied because this crate does not depend on
/// `models`.
pub(crate) fn escape_text(text: &str, cap: usize) -> String {
    let mut out = String::new();
    let mut used = 0_usize;
    for c in text.chars() {
        if is_hidden(c) {
            let escape = format!("[U+{:04X}]", u32::from(c));
            let len = escape.len(); // ASCII: bytes are characters
            if used.saturating_add(len) > cap {
                break;
            }
            out.push_str(&escape);
            used = used.saturating_add(len);
        } else {
            if used >= cap {
                break;
            }
            out.push(c);
            used = used.saturating_add(1);
        }
    }
    out
}

/// Whether `c` is written as an escape by [`escape_text`]: control characters and every
/// character that changes how text looks without being visible. A copy of
/// `is_invisible_or_control` in `crates/models/src/hf.rs` (the set of
/// `models::hf::uploader_text`, the approval cards' sanitiser and the comparison screen);
/// keep the two the same.
fn is_hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'                      // soft hyphen
                | '\u{061C}'                // Arabic letter mark
                | '\u{115F}' | '\u{1160}'   // Hangul fillers
                | '\u{17B4}' | '\u{17B5}'
                | '\u{180B}'..='\u{180F}'   // Mongolian variation selectors, vowel separator
                | '\u{200B}'..='\u{200F}'   // zero-width space and joiners, LRM, RLM
                | '\u{202A}'..='\u{202E}'   // bidi embeddings and overrides
                | '\u{2060}'..='\u{206F}'   // word joiner, invisible operators, bidi isolates
                | '\u{3164}'                // Hangul filler
                | '\u{FE00}'..='\u{FE0F}'   // variation selectors
                | '\u{FEFF}'                // zero-width no-break space
                | '\u{FFA0}'
                | '\u{FFF9}'..='\u{FFFB}'   // interlinear annotation
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0000}'..='\u{E007F}' // tag characters
                | '\u{E0100}'..='\u{E01EF}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llama::backend::BackendKind;

    const MEASURE_CPU: &str = include_str!("../../tests/fixtures/fit_params_measure_cpu.jsonl");
    const MEASURE_VULKAN: &str =
        include_str!("../../tests/fixtures/fit_params_measure_vulkan.jsonl");
    const BROKEN_FILE: &str = include_str!("../../tests/fixtures/fit_params_broken_file.jsonl");

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    fn config() -> MeasureConfig {
        MeasureConfig {
            model: PathBuf::from("/models/m.gguf"),
            ctx: 8192,
            device: Some("Vulkan0".into()),
            layers: MeasureLayers::All,
            experts_on_cpu: ExpertsOnCpu::None,
            kv_in_computer_memory: false,
            save_memory: false,
            flash_attn: FlashAttn::Auto,
            hip_visible_devices: None,
        }
    }

    fn backend(build: &str, source: BackendSource, fit_params: bool) -> Backend {
        Backend {
            kind: BackendKind::Vulkan,
            dir: PathBuf::from("/rt"),
            server: PathBuf::from("/rt/llama-server"),
            fit_params: fit_params.then(|| PathBuf::from("/rt/llama-fit-params")),
            build: build.into(),
            source,
        }
    }

    /// A record line with these rows (JSON text).
    fn record(rows: &str) -> String {
        format!(r#"{{"type":"fit_memory_breakdown","data":{{"unit":"MiB","rows":[{rows}]}}}}"#)
    }

    /// Every combination of the measured choices, as the exact list of section 4.3.
    #[test]
    fn measure_args_are_exact() {
        assert_eq!(
            build_measure_args(&config()).join(" "),
            "-m /models/m.gguf -c 8192 -np 1 --device Vulkan0 -ngl all --fit off -fitp on \
             --log-jsonl --offline"
        );
        let mut seen = 0;
        for device in [Some("Vulkan0"), Some(""), None] {
            for layers in [
                MeasureLayers::All,
                MeasureLayers::Count(0),
                MeasureLayers::Count(30),
            ] {
                for experts in [
                    ExpertsOnCpu::None,
                    ExpertsOnCpu::All,
                    ExpertsOnCpu::FirstLayers(0),
                    ExpertsOnCpu::FirstLayers(5),
                ] {
                    for kv in [false, true] {
                        for save_memory in [false, true] {
                            for fa in [FlashAttn::Auto, FlashAttn::On, FlashAttn::Off] {
                                let cfg = MeasureConfig {
                                    device: device.map(str::to_owned),
                                    layers,
                                    experts_on_cpu: experts,
                                    kv_in_computer_memory: kv,
                                    save_memory,
                                    flash_attn: fa,
                                    hip_visible_devices: Some("1".into()),
                                    ..config()
                                };
                                let mut want: Vec<String> = [
                                    "-m",
                                    "/models/m.gguf",
                                    "-c",
                                    "8192",
                                    "-np",
                                    "1",
                                    "--device",
                                    device.filter(|d| !d.is_empty()).unwrap_or("none"),
                                    "-ngl",
                                ]
                                .map(str::to_owned)
                                .to_vec();
                                want.push(match layers {
                                    MeasureLayers::All => "all".to_owned(),
                                    MeasureLayers::Count(n) => n.to_string(),
                                });
                                match experts {
                                    ExpertsOnCpu::All => want.push("--cpu-moe".into()),
                                    ExpertsOnCpu::FirstLayers(5) => {
                                        want.extend(["--n-cpu-moe".into(), "5".into()]);
                                    }
                                    _ => {}
                                }
                                if kv {
                                    want.push("-nkvo".into());
                                }
                                let fa_words: &[&str] = match (save_memory, fa) {
                                    (true, _) => &["-fa", "on", "-ctk", "q8_0", "-ctv", "q8_0"],
                                    (false, FlashAttn::Auto) => &[],
                                    (false, FlashAttn::On) => &["-fa", "on"],
                                    (false, FlashAttn::Off) => &["-fa", "off"],
                                };
                                want.extend(fa_words.iter().map(|s| (*s).to_owned()));
                                want.extend(
                                    ["--fit", "off", "-fitp", "on", "--log-jsonl", "--offline"]
                                        .map(str::to_owned),
                                );
                                let args = build_measure_args(&cfg);
                                assert_eq!(args, want, "{cfg:?}");
                                assert!(
                                    !args.iter().any(|a| a == "1" && args.len() < 3),
                                    "HIP_VISIBLE_DEVICES never on the command line"
                                );
                                for flag in ["-fa", "-ngl", "--device", "-c", "--fit"] {
                                    let n = args.iter().filter(|a| *a == flag).count();
                                    assert!(n <= 1, "{flag} in {args:?}");
                                }
                                seen += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(seen, 3 * 3 * 4 * 2 * 2 * 3);

        // The limits of the command line, as for the server (`launch::MAX_LAYER_COUNT`):
        // llama.cpp builds one override per layer for `--n-cpu-moe K` before it reads the
        // model, so a huge K uses up memory long before the timeout, and from about 4,000
        // it aborts. A count above the limit means every layer, which is what any count at
        // or above the model's own does.
        let words = |layers: MeasureLayers, experts: ExpertsOnCpu| {
            let cfg = MeasureConfig {
                layers,
                experts_on_cpu: experts,
                ..config()
            };
            let args = build_measure_args(&cfg);
            let at = args.iter().position(|a| a == "-ngl").expect("-ngl");
            let end = args.iter().position(|a| a == "--fit").expect("--fit");
            args[at..end].join(" ")
        };
        let limit = crate::llama::launch::MAX_LAYER_COUNT;
        for (layers, experts, want) in [
            (
                MeasureLayers::Count(limit),
                ExpertsOnCpu::FirstLayers(limit),
                "-ngl 1000 --n-cpu-moe 1000",
            ),
            (
                MeasureLayers::Count(limit + 1),
                ExpertsOnCpu::FirstLayers(limit + 1),
                "-ngl all --cpu-moe",
            ),
            (
                MeasureLayers::Count(u32::MAX),
                ExpertsOnCpu::FirstLayers(2_000_000_000),
                "-ngl all --cpu-moe",
            ),
            (
                MeasureLayers::Count(0),
                ExpertsOnCpu::FirstLayers(u32::MAX),
                "-ngl 0 --cpu-moe",
            ),
        ] {
            assert_eq!(words(layers, experts), want, "{layers:?} {experts:?}");
        }
    }

    /// ROCm's device mask goes into the environment only; the base is the backend's own.
    #[test]
    fn measure_env_adds_only_the_device_mask() {
        let b = backend("b12110", BackendSource::Installed, true);
        let plain = measure_env(&b, &config()).resolve_from(Vec::new());
        assert!(plain.iter().all(|(k, _)| k != "HIP_VISIBLE_DEVICES"));
        assert!(
            plain
                .iter()
                .any(|(k, v)| k == "GGML_NO_BACKTRACE" && v == "1")
        );
        let rocm = MeasureConfig {
            hip_visible_devices: Some("1".into()),
            ..config()
        };
        let resolved = measure_env(&b, &rocm).resolve_from(Vec::new());
        assert!(
            resolved
                .iter()
                .any(|(k, v)| k == "HIP_VISIBLE_DEVICES" && v == "1")
        );
        assert!(
            resolved
                .iter()
                .all(|(k, _)| !k.to_string_lossy().starts_with("LLAMA_"))
        );
    }

    /// The capture of section 0, item 3: Host and the repacked `AMX` weights, MiB made
    /// bytes, the plain line (which leaves out `AMX`) ignored.
    #[test]
    fn parses_the_captured_measure_output() {
        let rows = parse_measure_output(&lines(MEASURE_CPU)).expect("rows");
        assert_eq!(
            rows,
            vec![
                MemoryRow {
                    kind: MemoryRowKind::Host,
                    name: "Host".into(),
                    description: String::new(),
                    total_bytes: None,
                    free_bytes: None,
                    model_bytes: 157 * MIB,
                    context_bytes: 448 * MIB,
                    compute_bytes: 312 * MIB,
                },
                MemoryRow {
                    kind: MemoryRowKind::BufferType,
                    name: "AMX".into(),
                    description: String::new(),
                    total_bytes: None,
                    free_bytes: None,
                    model_bytes: 674 * MIB,
                    context_bytes: 0,
                    compute_bytes: 0,
                },
            ]
        );
        // Only the plain line: nothing.
        assert_eq!(
            parse_measure_output(&lines("Host 157 448 312")),
            Err(FitParamsError::NoReport)
        );
    }

    /// A device row carries total, free and a description; a negative `unaccounted` is
    /// ignored (the hand-written Vulkan shape, and a minimal row).
    #[test]
    fn parses_a_device_row_with_total_and_free() {
        let rows = parse_measure_output(&lines(MEASURE_VULKAN)).expect("rows");
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0],
            MemoryRow {
                kind: MemoryRowKind::Device,
                name: "Vulkan0".into(),
                description: "AMD Radeon RX 9070 XT".into(),
                total_bytes: Some(16304 * MIB),
                free_bytes: Some(15400 * MIB),
                model_bytes: 10137 * MIB,
                context_bytes: 1229 * MIB,
                compute_bytes: 614 * MIB,
            }
        );
        assert_eq!(rows[1].kind, MemoryRowKind::Host);
        assert_eq!(rows[1].model_bytes, 4199 * MIB);
        assert_eq!(rows[2].kind, MemoryRowKind::BufferType);
        let one = record(
            r#"{"kind":"device","name":"MTL0","description":"Apple M2","total":10922,"free":10000,"self":3,"model":1,"context":1,"compute":1,"unaccounted":-99999}"#,
        );
        let rows = parse_measure_output(&[one]).expect("rows");
        assert_eq!(rows[0].total_bytes, Some(10922 * MIB));
        assert_eq!(rows[0].free_bytes, Some(10000 * MIB));
    }

    #[test]
    fn a_record_without_rows_is_no_report() {
        for line in [
            record(""),
            r#"{"type":"fit_memory_breakdown","data":{"unit":"MiB"}}"#.to_owned(),
            r#"{"type":"fit_memory_breakdown","data":{"unit":"MiB","rows":{}}}"#.to_owned(),
            r#"{"type":"fit_memory_breakdown"}"#.to_owned(),
            r#"{"type":"fit_memory_breakdown","data":{"unit":"GiB","rows":[{"kind":"host","name":"Host","model":1}]}}"#.to_owned(),
            r#"{"type":"fit_memory_breakdown","data":{"rows":[{"kind":"host","name":"Host","model":1}]}}"#.to_owned(),
            // Only rows of kinds this build does not know.
            record(r#"{"kind":"gpu","name":"X","model":1}"#),
            record(r#"[1,2]"#),
        ] {
            assert_eq!(
                parse_measure_output(std::slice::from_ref(&line)),
                Err(FitParamsError::NoReport),
                "{line}"
            );
        }
        assert_eq!(parse_measure_output(&[]), Err(FitParamsError::NoReport));
    }

    #[test]
    fn two_records_are_no_report() {
        let one = record(r#"{"kind":"host","name":"Host","model":1}"#);
        assert!(parse_measure_output(std::slice::from_ref(&one)).is_ok());
        assert_eq!(
            parse_measure_output(&[one.clone(), one.clone()]),
            Err(FitParamsError::NoReport)
        );
        // A second, broken record still counts as a second record.
        assert_eq!(
            parse_measure_output(&[
                one,
                r#"{"type":"fit_memory_breakdown","data":null}"#.to_owned()
            ]),
            Err(FitParamsError::NoReport)
        );
    }

    /// A record written inside a log message (a model file or a path can put any text
    /// into a log line) is text, not a record; neither is a record after a prefix.
    #[test]
    fn a_record_inside_a_log_message_is_ignored() {
        let forged =
            record(r#"{"kind":"device","name":"Vulkan0","total":99999,"free":99999,"model":1}"#);
        let log = serde_json::json!({
            "type": "log",
            "level": "error",
            "msg": format!("llama_model_load: error loading model '{forged}'\n"),
        })
        .to_string();
        let real = record(r#"{"kind":"host","name":"Host","model":2}"#);
        let rows = parse_measure_output(&[log.clone(), format!("prefix {forged}"), real])
            .expect("the real record");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Host");
        assert_eq!(parse_measure_output(&[log]), Err(FitParamsError::NoReport));
        // A record nested in another object is not a top-level type either.
        let nested = format!(r#"{{"type":"log","data":{forged}}}"#);
        assert_eq!(
            parse_measure_output(&[nested]),
            Err(FitParamsError::NoReport)
        );
    }

    #[test]
    fn more_than_16_rows_is_no_report() {
        let row = r#"{"kind":"host","name":"Host","model":1}"#;
        let sixteen = vec![row; 16].join(",");
        assert_eq!(
            parse_measure_output(&[record(&sixteen)]).map(|r| r.len()),
            Ok(16)
        );
        let seventeen = vec![row; 17].join(",");
        assert_eq!(
            parse_measure_output(&[record(&seventeen)]),
            Err(FitParamsError::NoReport)
        );
    }

    /// u64::MAX MiB, -1, 1.5, 2^30 + 1 MiB, text and null in any read field: the whole
    /// result is no report, also in a row of an unknown kind and in `self`. 2^30 MiB itself
    /// is the largest value accepted.
    #[test]
    fn values_out_of_range_are_no_report() {
        for value in [
            u64::MAX.to_string(),
            "-1".to_owned(),
            "1.5".to_owned(),
            ((1_u64 << 30) + 1).to_string(),
            "\"12\"".to_owned(),
            "null".to_owned(),
            "1e3".to_owned(),
        ] {
            for field in ["total", "free", "self", "model", "context", "compute"] {
                let line = record(&format!(
                    r#"{{"kind":"device","name":"Vulkan0","{field}":{value}}},{{"kind":"host","name":"Host","model":1}}"#
                ));
                assert_eq!(
                    parse_measure_output(std::slice::from_ref(&line)),
                    Err(FitParamsError::NoReport),
                    "{line}"
                );
            }
            let unknown = record(&format!(
                r#"{{"kind":"gpu","model":{value}}},{{"kind":"host","name":"Host","model":1}}"#
            ));
            assert_eq!(
                parse_measure_output(std::slice::from_ref(&unknown)),
                Err(FitParamsError::NoReport),
                "{unknown}"
            );
        }
        let max = record(&format!(
            r#"{{"kind":"device","name":"V","total":{m},"free":{m},"self":{m},"model":{m},"context":{m},"compute":{m}}}"#,
            m = 1_u64 << 30
        ));
        let rows = parse_measure_output(&[max]).expect("the largest value is accepted");
        assert_eq!(rows[0].model_bytes, 1_u64 << 50);
        assert_eq!(rows[0].total_bytes, Some(1_u64 << 50));
        // A negative `unaccounted` (and other fields) is ignored.
        let ignored = record(
            r#"{"kind":"device","name":"V","model":1,"unaccounted":-5,"extra":"x","self":1}"#,
        );
        assert!(parse_measure_output(&[ignored]).is_ok());
    }

    #[test]
    fn hostile_names_are_escaped_and_capped() {
        let long = "x".repeat(500);
        let line = record(&format!(
            r#"{{"kind":"device","name":"Vul\u202ekan\u0007{long}","description":"AMD\u200b Radeon\u2066\n{long}","model":1}}"#
        ));
        let rows = parse_measure_output(&[line]).expect("rows");
        let row = &rows[0];
        assert!(
            row.name.starts_with("Vul[U+202E]kan[U+0007]xx"),
            "{}",
            row.name
        );
        assert_eq!(row.name.chars().count(), MAX_ROW_NAME_CHARS);
        assert!(
            row.description
                .starts_with("AMD[U+200B] Radeon[U+2066][U+000A]xx"),
            "{}",
            row.description
        );
        assert_eq!(row.description.chars().count(), MAX_ROW_DESCRIPTION_CHARS);
        for c in ['\u{202e}', '\u{7}', '\u{200b}', '\u{2066}', '\n'] {
            assert!(!row.name.contains(c) && !row.description.contains(c));
        }
        // Tag characters, variation selectors and the other invisible characters of
        // `models::hf::uploader_text`'s set are escaped too.
        let line = record(
            "{\"kind\":\"device\",\"name\":\"AB\u{e0041}\u{e0042}0\u{ad}\u{fe0f}\u{180e}\u{206a}\",\
             \"description\":\"Radeon\u{206f}\u{fff9}\u{e0100}\",\"model\":1}",
        );
        let rows = parse_measure_output(&[line]).expect("rows");
        assert_eq!(
            rows[0].name,
            "AB[U+E0041][U+E0042]0[U+00AD][U+FE0F][U+180E][U+206A]"
        );
        assert_eq!(rows[0].description, "Radeon[U+206F][U+FFF9][U+E0100]");
        // A description on a host row is dropped (only device rows carry one).
        let host = record(r#"{"kind":"host","name":"Host","description":"x","total":5,"model":1}"#);
        let rows = parse_measure_output(&[host]).expect("rows");
        assert_eq!(rows[0].description, "");
        assert_eq!(rows[0].total_bytes, None);
    }

    /// The escaping rule of section 2.0, piece by piece.
    #[test]
    fn escape_text_rules() {
        assert_eq!(escape_text("plain", 64), "plain");
        assert_eq!(escape_text("a\u{061c}b", 64), "a[U+061C]b");
        let listed = [
            '\u{0}', '\u{1f}', '\u{7f}', '\u{9f}', '\u{200b}', '\u{200c}', '\u{200d}', '\u{200e}',
            '\u{200f}', '\u{202a}', '\u{202e}', '\u{2060}', '\u{2064}', '\u{2066}', '\u{2069}',
            '\u{feff}',
        ];
        // The rest of `models::hf::uploader_text`'s set (crates/models/src/hf.rs,
        // `is_invisible_or_control`): soft hyphen, fillers, variation selectors, the
        // deprecated format characters, interlinear annotation, musical formatting and the
        // tag characters that can hide text ("ASCII smuggling").
        let also = [
            '\u{ad}',
            '\u{115f}',
            '\u{1160}',
            '\u{17b4}',
            '\u{17b5}',
            '\u{180b}',
            '\u{180e}',
            '\u{180f}',
            '\u{2065}',
            '\u{206a}',
            '\u{206f}',
            '\u{3164}',
            '\u{fe00}',
            '\u{fe0f}',
            '\u{ffa0}',
            '\u{fff9}',
            '\u{fffb}',
            '\u{1d173}',
            '\u{1d17a}',
            '\u{e0000}',
            '\u{e0041}',
            '\u{e007f}',
            '\u{e0100}',
            '\u{e01ef}',
        ];
        for c in listed.into_iter().chain(also) {
            let out = escape_text(&format!("a{c}b"), 64);
            assert_eq!(out, format!("a[U+{:04X}]b", u32::from(c)));
        }
        // Their visible neighbours pass.
        for c in [
            '\u{ac}',
            '\u{ae}',
            '\u{1810}',
            '\u{2070}',
            '\u{3165}',
            '\u{fe10}',
            '\u{fffc}',
            '\u{1d17b}',
            '\u{e0080}',
            '\u{e01f0}',
        ] {
            assert_eq!(escape_text(&format!("a{c}b"), 64), format!("a{c}b"));
        }
        // Hidden text in tag characters is spelled out, never kept.
        let smuggled = "llama\u{e0049}\u{e0047}\u{e004e}\u{206a}\u{ad}";
        assert_eq!(
            escape_text(smuggled, 300),
            "llama[U+E0049][U+E0047][U+E004E][U+206A][U+00AD]"
        );
        // Visible characters pass, counted in characters.
        assert_eq!(escape_text("éèê", 2), "éè");
        assert_eq!(escape_text("Radeon™ 9070", 64), "Radeon™ 9070");
        // An escape that does not fit whole is left out.
        assert_eq!(escape_text("abc\u{202e}", 8), "abc");
        assert_eq!(escape_text("abc\u{202e}", 11), "abc[U+202E]");
        assert_eq!(escape_text("", 10), "");
        assert_eq!(escape_text("abc", 0), "");
        // A huge input is cut quickly.
        let huge = "\u{202e}".repeat(1_000_000);
        assert_eq!(escape_text(&huge, 80).chars().count(), 80);
    }

    /// The broken-file run of section 0, item 12: the reason is in the log records on
    /// stdout (stderr only says an exception was thrown); both are kept for the log,
    /// escaped and cut, the last 40 of each.
    #[test]
    fn broken_file_log_records_are_kept_for_the_log() {
        let stdout = lines(BROKEN_FILE);
        assert_eq!(parse_measure_output(&stdout), Err(FitParamsError::NoReport));
        let stderr = vec![
            "terminate called after throwing an instance of 'std::runtime_error'".to_owned(),
            "  what():  failed to load model".to_owned(),
        ];
        let (records, tail) = log_records(&stdout, &stderr);
        assert_eq!(
            records,
            vec![
                "llama_model_load: error loading model: tensor 'token_embd.weight' data is not \
                 within the file bounds, model is corrupted or incomplete"
                    .to_owned(),
                "llama_model_load_from_file_impl: failed to load model".to_owned(),
            ],
            "info records are left out, the trailing line break is trimmed"
        );
        assert_eq!(tail, stderr);

        // At most 40 of each, the last ones, every message escaped and cut.
        let many: Vec<String> = (0..100)
            .map(|i| {
                serde_json::json!({
                    "type": "log",
                    "level": if i % 2 == 0 { "warn" } else { "error" },
                    "msg": format!("{i}\u{202e}{}", "y".repeat(1000)),
                })
                .to_string()
            })
            .collect();
        let err: Vec<String> = (0..100).map(|i| format!("e{i}\u{7}")).collect();
        let (records, tail) = log_records(&many, &err);
        assert_eq!(records.len(), LOG_LINES_KEPT);
        // A model file can name its architecture with hidden tag characters; llama.cpp
        // repeats the name in an error record, which reaches the app's log escaped.
        let hidden = serde_json::json!({
            "type": "log",
            "level": "error",
            "msg": "unknown model architecture: 'llama\u{e0049}\u{e0047}\u{206a}\u{ad}'",
        })
        .to_string();
        let (hidden_records, hidden_tail) = log_records(&[hidden], &["x\u{e0041}".to_owned()]);
        assert_eq!(
            hidden_records,
            vec!["unknown model architecture: 'llama[U+E0049][U+E0047][U+206A][U+00AD]'"]
        );
        assert_eq!(hidden_tail, vec!["x[U+E0041]"]);
        assert!(records[0].starts_with("60[U+202E]y"), "{}", records[0]);
        assert!(
            records
                .iter()
                .all(|r| r.chars().count() <= MAX_LOG_MESSAGE_CHARS)
        );
        assert_eq!(tail.len(), LOG_LINES_KEPT);
        assert_eq!(tail[0], "e60[U+0007]");
        assert_eq!(tail[39], "e99[U+0007]");
        // Debug and info records, and records of other types, are not kept.
        let quiet = vec![
            r#"{"type":"log","level":"info","msg":"x"}"#.to_owned(),
            r#"{"type":"log","level":"debug","msg":"x"}"#.to_owned(),
            r#"{"type":"other","level":"error","msg":"x"}"#.to_owned(),
            "not json".to_owned(),
        ];
        assert!(log_records(&quiet, &[]).0.is_empty());
    }

    /// I6-15: the installed runtime and the CUDA pack carry the lock's tag; the ROCm pack
    /// does not (and has no fit-params file); a development folder counts in a debug build
    /// only; no fit-params file never counts.
    #[test]
    fn only_pinned_builds_are_measurable() {
        let tag = lock::embedded().llama_cpp.tag.clone();
        let installed = backend(&tag, BackendSource::Installed, true);
        let cuda = backend(&tag, BackendSource::Pack { id: "cuda".into() }, true);
        let rocm = backend(
            "lemonade-b1372",
            BackendSource::Pack { id: "rocm".into() },
            false,
        );
        let rocm_with_file = backend(
            "lemonade-b1372",
            BackendSource::Pack { id: "rocm".into() },
            true,
        );
        let dev = backend("dev", BackendSource::DevOverride, true);
        let dev_without_file = backend("dev", BackendSource::DevOverride, false);
        let installed_without_file = backend(&tag, BackendSource::Installed, false);
        for debug in [false, true] {
            assert!(measurable_with(&installed, &tag, debug));
            assert!(measurable_with(&cuda, &tag, debug));
            assert!(!measurable_with(&rocm, &tag, debug));
            assert!(!measurable_with(&rocm_with_file, &tag, debug));
            assert!(!measurable_with(&dev_without_file, &tag, debug));
            assert!(!measurable_with(&installed_without_file, &tag, debug));
            assert_eq!(measurable_with(&dev, &tag, debug), debug);
        }
        // An empty pinned tag never matches.
        assert!(!measurable_with(
            &backend("", BackendSource::Installed, true),
            "",
            false
        ));
        // The lock's real packs: CUDA is pinned, ROCm is not.
        let lock = lock::embedded();
        assert_eq!(
            lock.pack("cuda").map(|p| p.build.as_str()),
            Some(tag.as_str())
        );
        assert_ne!(
            lock.pack("rocm").map(|p| p.build.as_str()),
            Some(tag.as_str())
        );
        assert_eq!(
            measurable(&installed),
            measurable_with(&installed, &tag, cfg!(debug_assertions))
        );
    }

    /// A model path that is not valid Unicode is refused before anything starts (never a
    /// lossy conversion), like launch does.
    #[cfg(unix)]
    #[tokio::test]
    async fn measure_refuses_a_path_that_is_not_unicode() {
        use std::os::unix::ffi::OsStrExt;
        let tag = lock::embedded().llama_cpp.tag.clone();
        let b = backend(&tag, BackendSource::Installed, true);
        let cfg = MeasureConfig {
            model: PathBuf::from(std::ffi::OsStr::from_bytes(b"/models/\xff.gguf")),
            ..config()
        };
        let result = measure(&b, &cfg, FIT_PARAMS_TIMEOUT, CancellationToken::new()).await;
        assert!(
            matches!(&result, Err(FitParamsError::Start { message }) if message.contains("Unicode")),
            "{result:?}"
        );
    }

    /// Not measurable: no run at all.
    #[tokio::test]
    async fn measure_without_the_tool_is_no_tool() {
        let rocm = backend(
            "lemonade-b1372",
            BackendSource::Pack { id: "rocm".into() },
            false,
        );
        assert_eq!(
            measure(
                &rocm,
                &config(),
                FIT_PARAMS_TIMEOUT,
                CancellationToken::new()
            )
            .await,
            Err(FitParamsError::NoTool)
        );
    }

    /// A program that cannot be started is `Start`; a cancelled request starts nothing.
    #[tokio::test]
    async fn measure_reports_start_failures_and_cancel() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tag = lock::embedded().llama_cpp.tag.clone();
        let b = Backend {
            dir: dir.path().to_path_buf(),
            fit_params: Some(dir.path().join("no-such-llama-fit-params")),
            ..backend(&tag, BackendSource::Installed, true)
        };
        let result = measure(&b, &config(), FIT_PARAMS_TIMEOUT, CancellationToken::new()).await;
        assert!(
            matches!(result, Err(FitParamsError::Start { .. })),
            "{result:?}"
        );
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert_eq!(
            measure(&b, &config(), FIT_PARAMS_TIMEOUT, cancelled).await,
            Err(FitParamsError::Cancelled)
        );
    }

    /// With a stand-in program (a shell script): a hang is killed on the timeout and on
    /// cancel, a failing exit is `Failed`, and a good record is read, all through the
    /// crate's spawn.
    #[cfg(unix)]
    #[tokio::test]
    async fn measure_runs_kills_and_reads_a_stand_in() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let tag = lock::embedded().llama_cpp.tag.clone();
        let script = |name: &str, body: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            Backend {
                dir: dir.path().to_path_buf(),
                fit_params: Some(path),
                ..backend(&tag, BackendSource::Installed, true)
            }
        };
        let good = script(
            "good",
            &format!(
                "echo 'plain line'\necho '{}'\necho 'Host 1 2 3'",
                record(
                    r#"{"kind":"host","name":"Host","self":6,"model":1,"context":2,"compute":3}"#
                )
            ),
        );
        let rows = measure(
            &good,
            &config(),
            FIT_PARAMS_TIMEOUT,
            CancellationToken::new(),
        )
        .await
        .expect("rows");
        assert_eq!(rows[0].context_bytes, 2 * MIB);

        let failing = script("failing", "echo 'x' >&2\nexit 3");
        assert_eq!(
            measure(
                &failing,
                &config(),
                FIT_PARAMS_TIMEOUT,
                CancellationToken::new()
            )
            .await,
            Err(FitParamsError::Failed { code: Some(3) })
        );
        let silent = script("silent", "exit 0");
        assert_eq!(
            measure(
                &silent,
                &config(),
                FIT_PARAMS_TIMEOUT,
                CancellationToken::new()
            )
            .await,
            Err(FitParamsError::NoReport)
        );

        let hang = script("hang", "sleep 30");
        let started = std::time::Instant::now();
        assert_eq!(
            measure(
                &hang,
                &config(),
                Duration::from_millis(300),
                CancellationToken::new()
            )
            .await,
            Err(FitParamsError::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(10));

        let cancel = CancellationToken::new();
        let later = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            later.cancel();
        });
        let started = std::time::Instant::now();
        assert_eq!(
            measure(&hang, &config(), FIT_PARAMS_TIMEOUT, cancel).await,
            Err(FitParamsError::Cancelled)
        );
        assert!(started.elapsed() < Duration::from_secs(10));

        // The arguments arrive as separate words, exactly as built.
        let echo = script(
            "echo-args",
            &format!(
                "for a in \"$@\"; do printf '%s\\n' \"$a\" >> '{}'; done\necho '{}'",
                dir.path().join("args.txt").display(),
                record(r#"{"kind":"host","name":"Host","model":1}"#)
            ),
        );
        let cfg = MeasureConfig {
            model: dir.path().join("a model with spaces.gguf"),
            ..config()
        };
        measure(&echo, &cfg, FIT_PARAMS_TIMEOUT, CancellationToken::new())
            .await
            .expect("rows");
        let seen = std::fs::read_to_string(dir.path().join("args.txt")).expect("args");
        let seen: Vec<&str> = seen.lines().collect();
        assert_eq!(seen, build_measure_args(&cfg));
    }

    /// The limit covers a first start on macOS (I6-14), where the program also compiles
    /// its Metal library before it measures: on a CI Mac one first run was still going
    /// when the old limit of 30 s stopped it. The limit is at least four times that.
    #[test]
    fn the_limit_covers_a_first_start_on_macos() {
        let stopped_at = Duration::from_secs(30);
        assert!(
            FIT_PARAMS_TIMEOUT >= stopped_at * 4,
            "{FIT_PARAMS_TIMEOUT:?} is too short for a first start on macOS"
        );
    }

    /// The plain line fit-params prints after it queued its record, as the pinned build
    /// writes it (with a trailing space), is the sign that only the record went missing.
    #[test]
    fn the_plain_host_line_means_the_record_was_lost() {
        assert!(record_was_lost(&lines("Host 157 448 312 ")));
        assert!(record_was_lost(&lines(
            "{\"type\":\"log\",\"level\":\"info\",\"msg\":\"x\"}\nVulkan0 10137 1229 614 \nHost 4199 0 306 "
        )));
        assert!(record_was_lost(&lines("Host 0 0 0\r")));
        for not in [
            "",
            "Host 1 2",
            "Host 1 2 3 4",
            "Host 1 2 3 x",
            "Host 1  2 3",
            "Host a b c",
            "Hostile 1 2 3",
            " Host 1 2 3",
            "Host -1 2 3",
            "Vulkan0 1 2 3",
            r#"{"type":"log","msg":"Host 1 2 3"}"#,
        ] {
            assert!(!record_was_lost(&lines(not)), "{not:?}");
        }
    }

    /// llama.cpp queues the record for its log thread and then prints the plain lines and
    /// exits; under load the process can end before the record is written (2 runs in 160
    /// on the build machine). A run that printed the plain line but no record is measured
    /// once more within the same timeout; a second loss, a silent run and a failed run are
    /// not retried.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_lost_record_is_measured_once_more() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let tag = lock::embedded().llama_cpp.tag.clone();
        let runs = dir.path().join("runs");
        let script = |name: &str, body: &str| {
            let path = dir.path().join(name);
            std::fs::write(
                &path,
                format!("#!/bin/sh\necho x >> '{}'\n{body}\n", runs.display()),
            )
            .expect("write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            Backend {
                dir: dir.path().to_path_buf(),
                fit_params: Some(path),
                ..backend(&tag, BackendSource::Installed, true)
            }
        };
        let count_runs = || {
            let n = std::fs::read_to_string(&runs).map_or(0, |s| s.lines().count());
            let _ = std::fs::remove_file(&runs);
            n
        };
        let good = record(r#"{"kind":"host","name":"Host","model":1,"context":2,"compute":3}"#);

        // Lost on the first run, written on the second.
        let flaky = script(
            "flaky",
            &format!(
                "if [ \"$(wc -l < '{}')\" -ge 2 ]; then echo '{good}'; fi\necho 'Host 1 2 3 '",
                runs.display()
            ),
        );
        let rows = measure(
            &flaky,
            &config(),
            FIT_PARAMS_TIMEOUT,
            CancellationToken::new(),
        )
        .await
        .expect("the second run's record");
        assert_eq!(rows[0].context_bytes, 2 * MIB);
        assert_eq!(count_runs(), 2);

        // Lost twice: no report, after two runs.
        let lost = script("lost", "echo 'Host 1 2 3 '");
        assert_eq!(
            measure(
                &lost,
                &config(),
                FIT_PARAMS_TIMEOUT,
                CancellationToken::new()
            )
            .await,
            Err(FitParamsError::NoReport)
        );
        assert_eq!(count_runs(), 2);

        // No plain line, a failed exit, or a record that is there: one run each.
        let silent = script("silent", "echo 'plain line'");
        assert_eq!(
            measure(
                &silent,
                &config(),
                FIT_PARAMS_TIMEOUT,
                CancellationToken::new()
            )
            .await,
            Err(FitParamsError::NoReport)
        );
        assert_eq!(count_runs(), 1);
        let failing = script("failing", "echo 'Host 1 2 3 '\nexit 1");
        assert_eq!(
            measure(
                &failing,
                &config(),
                FIT_PARAMS_TIMEOUT,
                CancellationToken::new()
            )
            .await,
            Err(FitParamsError::Failed { code: Some(1) })
        );
        assert_eq!(count_runs(), 1);
        let fine = script("fine", &format!("echo '{good}'\necho 'Host 1 2 3 '"));
        measure(
            &fine,
            &config(),
            FIT_PARAMS_TIMEOUT,
            CancellationToken::new(),
        )
        .await
        .expect("rows");
        assert_eq!(count_runs(), 1);

        // The second run gets only what is left of the timeout.
        let slow = script(
            "slow",
            &format!(
                "if [ \"$(wc -l < '{}')\" -ge 2 ]; then sleep 30; fi\nsleep 0.4\necho 'Host 1 2 3 '",
                runs.display()
            ),
        );
        let started = std::time::Instant::now();
        assert_eq!(
            measure(
                &slow,
                &config(),
                Duration::from_secs(2),
                CancellationToken::new()
            )
            .await,
            Err(FitParamsError::Timeout)
        );
        let took = started.elapsed();
        assert!(took < Duration::from_secs(5), "{took:?}");
        assert_eq!(count_runs(), 2);
    }

    /// The record and the plain line come at the end of a run, so the last
    /// [`MAX_STDOUT_LINES`] lines are kept: a run that logs more than that first is still
    /// read (and its warnings are the latest ones).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_long_log_before_the_record_is_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let tag = lock::embedded().llama_cpp.tag.clone();
        let path = dir.path().join("chatty");
        let log = r#"{"type":"log","level":"warn","msg":"a warning"}"#;
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\ni=0\nwhile [ $i -lt 2000 ]; do echo '{log}'; i=$((i+1)); done\necho '{}'\necho 'Host 1 2 3 '\n",
                record(r#"{"kind":"host","name":"Host","model":1,"context":2,"compute":3}"#)
            ),
        )
        .expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let chatty = Backend {
            dir: dir.path().to_path_buf(),
            fit_params: Some(path),
            ..backend(&tag, BackendSource::Installed, true)
        };
        let rows = measure(
            &chatty,
            &config(),
            FIT_PARAMS_TIMEOUT,
            CancellationToken::new(),
        )
        .await
        .expect("the record after 2,000 log lines");
        assert_eq!(rows[0].context_bytes, 2 * MIB);
    }
}
