//! A fake Studio engine for tests: this crate's (`tests/sd_cli.rs`) and the app's.
//!
//! A test binary started with [`FAKE_MODE_ENV`] set calls [`fake_main`] with the engine's
//! arguments, and it behaves as `sd-cli` does: it refuses an argument the pinned build does
//! not know (`error: unknown argument: …` and exit 1, so a flag the real engine would refuse
//! fails a test too), reads `--prompt-file`, prints the pinned build's output (the fixtures
//! `tests/fixtures/sd_progress_*.txt` and `sd_stderr_*.txt`, with their `\r` redraws and
//! `ESC[K`; the bars, INFO and WARN lines on stdout, the rest on stderr), writes tiny files
//! where `sd-cli` writes its results, and exits as the mode says. `--list-devices` prints the
//! Vulkan devices fixture in every mode but the two made to fail it (`devices-fail` and
//! `devices-hang`).
//!
//! With [`FAKE_RECORD_ENV`] naming a folder, each start for a job records there, `<n>`
//! counting the starts from 0:
//! - `argv-<n>.json`: the arguments, a JSON array of strings;
//! - `env-<n>.json`: the environment, a JSON object;
//! - `prompt-<n>.txt`: the bytes of the `--prompt-file`;
//! - `files-<n>.json`: the names in the output's folder when the start began, sorted, so a
//!   test can see that the prompt files were there and that a retry starts without the first
//!   start's half file;
//! - `grandchild-<n>.txt`: the process id of mode `hang`'s grandchild, so a test can see that
//!   Stop killed it too.
//!
//! A `--list-devices` start records none of these, so a runner that lists the devices before
//! a job still finds that job's start at 0, and a test that only lists devices finds no start.
//! Only the two modes made to fail it write a file then: `devices-fail` adds a line to
//! `devices-fail.txt` at each listing, so a test can count how often the engine was asked,
//! and `devices-hang` writes `devices-hang.txt`: its own process id, so a test can see that
//! the engine was stopped.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::args::FLAGS;
use crate::llama::devices::STATUS_DLL_NOT_FOUND;
use crate::llama::lines::LineReader;
use crate::spawn::{self, EnvSpec, SpawnSpec, StdioMode};

/// The variable that makes a test binary the fake engine, and its mode:
/// - `ok`: the progress of a picture (FLUX.2 klein), a batch or a clip, then its files;
/// - `oom-once`: runs out of memory at the first start and succeeds at the next (it leaves
///   a marker in the record folder, so it needs one);
/// - `oom`: runs out of memory at every start;
/// - `crash`: stops while drawing, with exit code 3 and no message;
/// - `hang`: starts one grandchild through the spawn helper, draws, and then both sleep;
/// - `missing-dll`: a library cannot be loaded (Windows' exit code 0xC0000135, 127
///   elsewhere);
/// - `bad-model`: a damaged model file;
/// - `slow`: one progress line, then [`SLOW_PAUSE`] of silence, then the rest as `ok`;
/// - `no-output`: the progress of `ok` up to its saving lines, then exit 0 without a file;
/// - `devices-fail`: `--list-devices` fails as `missing-dll` does (adding a line to
///   `devices-fail.txt` in the record folder each time); a job runs as `ok`;
/// - `devices-hang`: `--list-devices` records its process id and never answers (it sleeps); a
///   job runs as `ok`.
///
/// `oom`, `oom-once`, `crash` and `bad-model` write half a file first, as an engine that
/// fails while saving can.
pub const FAKE_MODE_ENV: &str = "RUNTIME_FAKE_SD";

/// The variable naming the folder each start records into (see the module's header).
pub const FAKE_RECORD_ENV: &str = "RUNTIME_FAKE_SD_RECORD";

/// Mode `slow`'s silence after its first line: 1.2 s, so the stall check in `tests/sd_cli.rs`
/// can take a third of it (400 ms) as its limit. With the 400 ms of silence and 200 ms limit
/// first planned, an engine start slower than 200 ms (a loaded Windows or macOS runner)
/// already sends a [`Stalled`](super::RunEvent::Stalled) before the first line, because the
/// stall timer starts with the engine, and the check sees two.
pub const SLOW_PAUSE: Duration = Duration::from_millis(1200);

/// How long mode `hang` and its grandchild sleep before they give up.
const HANG_FOR: Duration = Duration::from_secs(120);

/// Mode `crash`'s exit code.
const CRASH_CODE: i32 = 3;

/// Mode `missing-dll`'s exit code: Windows' 0xC0000135, 127 elsewhere.
const MISSING_DLL_CODE: i32 = if cfg!(windows) {
    STATUS_DLL_NOT_FOUND
} else {
    127
};

/// The mode mode `hang` starts its grandchild in, and the line it says when it is ready.
const GRANDCHILD: &str = "grandchild";
const GRANDCHILD_READY: &str = "fake-sd-grandchild-ready";

/// The marker mode `oom-once` leaves in the record folder.
const OOM_ONCE_MARKER: &str = "oom-once.done";

const PICTURE: &str = include_str!("../../tests/fixtures/sd_progress_flux2_klein.txt");
const BATCH: &str = include_str!("../../tests/fixtures/sd_progress_batch_of_two.txt");
const VIDEO: &str = include_str!("../../tests/fixtures/sd_progress_wan22_ti2v.txt");
const OUT_OF_MEMORY: &str = include_str!("../../tests/fixtures/sd_stderr_out_of_memory_vulkan.txt");
const BAD_MODEL: &str = include_str!("../../tests/fixtures/sd_stderr_bad_model.txt");
const MISSING_DLL: &str = include_str!("../../tests/fixtures/sd_stderr_missing_dll.txt");
const DEVICES: &str = include_str!("../../tests/fixtures/sd_list_devices_vulkan.txt");

/// Flags without a value: three of [`FLAGS`], and `--list-devices`.
const SWITCHES: &[&str] = &[
    "--vae-tiling",
    "--offload-to-cpu",
    "--disable-image-metadata",
    "--list-devices",
];

/// The fake engine, for the mode in [`FAKE_MODE_ENV`]: `args` are the engine's arguments
/// (without the program). It prints a fixture with `\r` redraws, writes tiny files to `-o`
/// (`<stem>.png` for one picture, `<stem>_0.png`, `<stem>_1.png`, … for a batch, the `-o`
/// path itself for `-M vid_gen`) and exits as the mode says.
pub fn fake_main(args: &[String]) -> ! {
    let mode = std::env::var(FAKE_MODE_ENV).unwrap_or_default();
    if mode == GRANDCHILD {
        grandchild();
    }
    let parsed = match Args::parse(args) {
        Ok(parsed) => parsed,
        Err(message) => fail(&format!("error: {message}\n")),
    };
    let record = std::env::var_os(FAKE_RECORD_ENV)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    if parsed.list_devices {
        list_devices(&mode, record.as_deref());
    }
    let start = record
        .as_deref()
        .map(|dir| (dir, record_start(dir, args, &parsed)));
    if let Some(path) = &parsed.prompt_file {
        let Ok(prompt) = std::fs::read(path) else {
            fail("[E] failed to read the prompt file --- main.cpp:0\n");
        };
        if let Some((dir, n)) = start {
            save(&dir.join(format!("prompt-{n}.txt")), &prompt);
        }
    }
    let outputs = parsed.outputs();
    let ran_out_once = record
        .as_deref()
        .is_some_and(|dir| dir.join(OOM_ONCE_MARKER).is_file());
    match mode.as_str() {
        "ok" | "devices-fail" | "devices-hang" => succeed(&parsed, &outputs, None),
        "slow" => succeed(&parsed, &outputs, Some(SLOW_PAUSE)),
        "no-output" => {
            emit(until_line(parsed.progress(), "[I] save result").0);
            std::process::exit(0)
        }
        "oom-once" if ran_out_once => succeed(&parsed, &outputs, None),
        "oom-once" => {
            if let Some(dir) = &record {
                save(&dir.join(OOM_ONCE_MARKER), b"");
            }
            fail_after_half_a_file(&outputs, OUT_OF_MEMORY)
        }
        "oom" => fail_after_half_a_file(&outputs, OUT_OF_MEMORY),
        "bad-model" => fail_after_half_a_file(&outputs, BAD_MODEL),
        "crash" => {
            write_half(&outputs);
            emit(until_line(PICTURE, "[I] sampling completed").0);
            std::process::exit(CRASH_CODE)
        }
        "missing-dll" => {
            emit(MISSING_DLL);
            std::process::exit(MISSING_DLL_CODE)
        }
        "hang" => hang(start),
        other => fail(&format!("[E] the fake engine has no mode {other:?}\n")),
    }
}

/// A 1×1 PNG, the file the fake writes for a picture.
pub fn tiny_png() -> Vec<u8> {
    TINY_PNG.to_vec()
}

/// A WebM (VP8) of one 16×16 frame, the file the fake writes for a clip.
pub fn tiny_webm() -> Vec<u8> {
    TINY_WEBM.to_vec()
}

/// `--list-devices`: the Vulkan devices fixture, or in mode `devices-fail` the output and
/// exit code of `missing-dll` (after it adds a line to `devices-fail.txt` in `record`, if
/// given), or in mode `devices-hang` nothing until it is stopped (after it records its
/// process id in `record`, if given, as `devices-hang.txt`).
fn list_devices(mode: &str, record: Option<&Path>) -> ! {
    match mode {
        "devices-fail" => {
            if let Some(dir) = record {
                append_line(&dir.join("devices-fail.txt"), "listed");
            }
            emit(MISSING_DLL);
            std::process::exit(MISSING_DLL_CODE)
        }
        "devices-hang" => {
            if let Some(dir) = record {
                // Written under another name and renamed, so a test never reads half of it.
                let partial = dir.join("devices-hang.txt.partial");
                save(&partial, std::process::id().to_string().as_bytes());
                if let Err(error) = std::fs::rename(&partial, dir.join("devices-hang.txt")) {
                    fail(&format!(
                        "[E] the fake engine could not record its process id: {error}\n"
                    ));
                }
            }
            std::thread::sleep(HANG_FOR);
            std::process::exit(1)
        }
        _ => {
            write_or_exit(&mut std::io::stdout().lock(), DEVICES);
            std::process::exit(0)
        }
    }
}

/// The arguments the fake reads; the others it only checks are known.
#[derive(Debug, Default)]
struct Args {
    output: Option<PathBuf>,
    prompt_file: Option<PathBuf>,
    batch: u32,
    video: bool,
    list_devices: bool,
}

impl Args {
    /// Reads `args` as the pinned parser does: each flag is a known name, and every flag
    /// but [`SWITCHES`] takes the next argument as its value.
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut parsed = Self {
            batch: 1,
            ..Self::default()
        };
        let mut rest = args.iter();
        while let Some(flag) = rest.next() {
            if SWITCHES.contains(&flag.as_str()) {
                parsed.list_devices |= flag == "--list-devices";
                continue;
            }
            if !FLAGS.contains(&flag.as_str()) {
                return Err(format!("unknown argument: {flag}"));
            }
            let value = rest
                .next()
                .ok_or_else(|| format!("missing argument for {flag}"))?;
            match flag.as_str() {
                "-o" => parsed.output = Some(value.into()),
                "--prompt-file" => parsed.prompt_file = Some(value.into()),
                "--batch-count" => {
                    parsed.batch = value
                        .parse()
                        .map_err(|_| format!("invalid --batch-count: {value}"))?;
                }
                "-M" => parsed.video = value == "vid_gen",
                _ => {}
            }
        }
        Ok(parsed)
    }

    /// The files `sd-cli`'s `save_results` writes for these arguments.
    fn outputs(&self) -> Vec<PathBuf> {
        let Some(output) = &self.output else {
            return Vec::new();
        };
        if self.video || self.batch <= 1 {
            return vec![output.clone()];
        }
        let stem = output
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let extension = output
            .extension()
            .map(|extension| extension.to_string_lossy().into_owned())
            .unwrap_or_default();
        (0..self.batch)
            .map(|i| output.with_file_name(format!("{stem}_{i}.{extension}")))
            .collect()
    }

    /// The output fixture for these arguments.
    fn progress(&self) -> &'static str {
        if self.video {
            VIDEO
        } else if self.batch > 1 {
            BATCH
        } else {
            PICTURE
        }
    }
}

/// Prints the progress (with a pause after its first line, for mode `slow`), writes the
/// files before the saving lines, and exits 0.
fn succeed(args: &Args, outputs: &[PathBuf], pause: Option<Duration>) -> ! {
    let (drawing, saving) = until_line(args.progress(), "[I] save result");
    let drawing = match pause {
        Some(pause) => {
            let (first, rest) = drawing.split_at(drawing.find('\n').map_or(0, |end| end + 1));
            emit(first);
            std::thread::sleep(pause);
            rest
        }
        None => drawing,
    };
    emit(drawing);
    for path in outputs {
        save(path, &tiny_file(path));
    }
    emit(saving);
    std::process::exit(0)
}

fn fail_after_half_a_file(outputs: &[PathBuf], failure: &str) -> ! {
    write_half(outputs);
    emit(failure);
    std::process::exit(1)
}

/// Starts the grandchild, records its id, draws, and sleeps until it is killed.
fn hang(start: Option<(&Path, usize)>) -> ! {
    // The grandchild is held by a thread of its own, with its own runtime: a test harness
    // may already run this inside a runtime.
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        let child = runtime.block_on(start_grandchild());
        let _ = sender.send(child.as_ref().and_then(|child| child.id()));
        std::thread::sleep(HANG_FOR);
        drop(child);
    });
    match receiver.recv_timeout(Duration::from_secs(20)) {
        Ok(Some(pid)) => {
            if let Some((dir, n)) = start {
                save(
                    &dir.join(format!("grandchild-{n}.txt")),
                    pid.to_string().as_bytes(),
                );
            }
        }
        _ => emit("the fake engine could not start its grandchild\n"),
    }
    emit(until_line(PICTURE, "[I] sampling completed").0);
    std::thread::sleep(HANG_FOR);
    std::process::exit(1)
}

/// This program again, with the same arguments, in mode [`GRANDCHILD`], once it says it is
/// ready.
async fn start_grandchild() -> Option<spawn::ManagedChild> {
    let program = std::env::current_exe().ok()?;
    let cwd = std::env::current_dir().ok()?;
    let spec = SpawnSpec::new(program, cwd)
        .args(std::env::args_os().skip(1))
        .env(EnvSpec::minimal().set(FAKE_MODE_ENV, GRANDCHILD))
        .stdin(StdioMode::Null)
        .stdout(StdioMode::Piped)
        .stderr(StdioMode::Null);
    let mut child = spawn::spawn(spec).ok()?;
    let mut reader = LineReader::new(child.take_stdout()?);
    let ready = async {
        while let Some(line) = reader.next_line().await {
            if line.trim() == GRANDCHILD_READY {
                return true;
            }
        }
        false
    };
    let ready = tokio::time::timeout(Duration::from_secs(15), ready).await;
    ready.unwrap_or(false).then_some(child)
}

/// Mode `hang`'s grandchild: it joins its parent's process group, says it is ready, and
/// sleeps.
fn grandchild() -> ! {
    #[cfg(unix)]
    join_parents_group();
    write_or_exit(
        &mut std::io::stdout().lock(),
        &format!("{GRANDCHILD_READY}\n"),
    );
    std::thread::sleep(HANG_FOR);
    std::process::exit(0)
}

/// The spawn helper makes every child the leader of a process group of its own. A helper
/// an engine starts the usual way stays in the engine's group, which is what Stop kills on
/// Unix, so the grandchild moves back into its parent's group. (On Windows it is in its
/// parent's Job Object whatever it does.)
#[cfg(unix)]
fn join_parents_group() {
    // SAFETY: these calls only read process ids and move this process into its parent's
    // group, in the same session; they touch no memory of this process.
    unsafe {
        let group = libc::getpgid(libc::getppid());
        if group > 0 {
            libc::setpgid(0, group);
        }
    }
}

/// Records the start in `dir` and returns its number.
fn record_start(dir: &Path, args: &[String], parsed: &Args) -> usize {
    let mut n = 0;
    while dir.join(format!("argv-{n}.json")).exists() {
        n += 1;
    }
    save_json(&dir.join(format!("argv-{n}.json")), &args);
    let env: BTreeMap<String, String> = std::env::vars_os()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect();
    save_json(&dir.join(format!("env-{n}.json")), &env);
    let mut names: Vec<String> = parsed
        .output
        .as_deref()
        .and_then(Path::parent)
        .and_then(|folder| std::fs::read_dir(folder).ok())
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    save_json(&dir.join(format!("files-{n}.json")), &names);
    n
}

/// Half of the first output, as an engine that failed while saving leaves it.
fn write_half(outputs: &[PathBuf]) {
    if let Some(path) = outputs.first() {
        let bytes = tiny_file(path);
        save(path, &bytes[..bytes.len() / 2]);
    }
}

fn tiny_file(path: &Path) -> Vec<u8> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("webm") => tiny_webm(),
        Some("webp") => TINY_WEBP.to_vec(),
        _ => tiny_png(),
    }
}

/// `text` up to the first line that starts with `marker`, and the rest from that line.
fn until_line<'a>(text: &'a str, marker: &str) -> (&'a str, &'a str) {
    let at = text
        .match_indices(marker)
        .map(|(at, _)| at)
        .find(|&at| at == 0 || text[..at].ends_with('\n'))
        .unwrap_or(text.len());
    text.split_at(at)
}

/// Prints `text` line by line as the pinned build splits it: `[E]` lines and lines without
/// a level (the loader's, ggml-vulkan's own) on stderr, the rest (`[I]`, `[W]`, the bars and
/// blank lines) on stdout. A line's `\r` redraws stay in it.
fn emit(text: &str) {
    for segment in text.split_inclusive('\n') {
        let line = segment.trim_start_matches('\r');
        let on_stderr = line.starts_with("[E]")
            || !(line.starts_with('[') || line.starts_with("  |") || line.trim().is_empty());
        if on_stderr {
            write_or_exit(&mut std::io::stderr().lock(), segment);
        } else {
            write_or_exit(&mut std::io::stdout().lock(), segment);
        }
    }
}

/// Writes and flushes, so the runner reads each line as it comes; a closed pipe ends the
/// fake as it ends the engine.
fn write_or_exit(out: &mut impl Write, text: &str) {
    if out
        .write_all(text.as_bytes())
        .and_then(|()| out.flush())
        .is_err()
    {
        std::process::exit(1);
    }
}

fn save(path: &Path, bytes: &[u8]) {
    if let Err(error) = std::fs::write(path, bytes) {
        fail(&format!(
            "[E] the fake engine could not write {}: {error}\n",
            path.display()
        ));
    }
}

/// Adds `line` to the file at `path`, creating it.
fn append_line(path: &Path, line: &str) {
    let appended = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{line}"));
    if let Err(error) = appended {
        fail(&format!(
            "[E] the fake engine could not write {}: {error}\n",
            path.display()
        ));
    }
}

fn save_json(path: &Path, value: &impl serde::Serialize) {
    match serde_json::to_vec(value) {
        Ok(bytes) => save(path, &bytes),
        Err(error) => fail(&format!("[E] the fake engine could not record: {error}\n")),
    }
}

/// Prints `message` on stderr and exits 1, as the engine does on a refused argument.
fn fail(message: &str) -> ! {
    write_or_exit(&mut std::io::stderr().lock(), message);
    std::process::exit(1)
}

/// A 1×1 red PNG.
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A 1×1 black lossless WebP, for a clip asked for as WebP (the fallback when WebM does not
/// play), so the fake writes a file of the asked type for every output format. It is
/// private: tests compare the bytes of [`tiny_png`] and [`tiny_webm`], never of a WebP.
const TINY_WEBP: &[u8] = &[
    0x52, 0x49, 0x46, 0x46, 0x1a, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50, 0x56, 0x50, 0x38, 0x4c,
    0x0e, 0x00, 0x00, 0x00, 0x2f, 0x00, 0x00, 0x00, 0x00, 0x07, 0x10, 0x11, 0xfd, 0x0f, 0x44, 0x44,
    0xff, 0x03,
];

/// A WebM (VP8) of one 16×16 black frame, as `ffmpeg -f lavfi -i color=c=black:s=16x16:r=24:d=0.0417
/// -frames:v 1 -c:v libvpx -b:v 10k -an -map_metadata -1 -fflags +bitexact -flags +bitexact -dash 0
/// -cues_to_front 0 -f webm` writes it.
const TINY_WEBM: &[u8] = &[
    0x1a, 0x45, 0xdf, 0xa3, 0x9f, 0x42, 0x86, 0x81, 0x01, 0x42, 0xf7, 0x81, 0x01, 0x42, 0xf2, 0x81,
    0x04, 0x42, 0xf3, 0x81, 0x08, 0x42, 0x82, 0x84, 0x77, 0x65, 0x62, 0x6d, 0x42, 0x87, 0x81, 0x02,
    0x42, 0x85, 0x81, 0x02, 0x18, 0x53, 0x80, 0x67, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xa4,
    0x11, 0x4d, 0x9b, 0x74, 0xba, 0x4d, 0xbb, 0x8b, 0x53, 0xab, 0x84, 0x15, 0x49, 0xa9, 0x66, 0x53,
    0xac, 0x81, 0xa1, 0x4d, 0xbb, 0x8b, 0x53, 0xab, 0x84, 0x16, 0x54, 0xae, 0x6b, 0x53, 0xac, 0x81,
    0xc6, 0x4d, 0xbb, 0x8c, 0x53, 0xab, 0x84, 0x12, 0x54, 0xc3, 0x67, 0x53, 0xac, 0x82, 0x01, 0x0c,
    0x4d, 0xbb, 0x8c, 0x53, 0xab, 0x84, 0x1c, 0x53, 0xbb, 0x6b, 0x53, 0xac, 0x82, 0x01, 0x8e, 0xec,
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x59, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x15, 0x49, 0xa9, 0x66, 0xa0, 0x2a, 0xd7, 0xb1, 0x83, 0x0f, 0x42, 0x40, 0x4d, 0x80, 0x84,
    0x4c, 0x61, 0x76, 0x66, 0x57, 0x41, 0x84, 0x4c, 0x61, 0x76, 0x66, 0x44, 0x89, 0x88, 0x40, 0x45,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x16, 0x54, 0xae, 0x6b, 0xc1, 0xae, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x38, 0xd7, 0x81, 0x01, 0x73, 0xc5, 0x88, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x01, 0x9c, 0x81, 0x00, 0x22, 0xb5, 0x9c, 0x83, 0x75, 0x6e, 0x64, 0x88, 0x81, 0x00, 0x86,
    0x85, 0x56, 0x5f, 0x56, 0x50, 0x38, 0x83, 0x81, 0x01, 0x23, 0xe3, 0x83, 0x84, 0x02, 0x7b, 0xc8,
    0x6a, 0xe0, 0x89, 0xb0, 0x81, 0x10, 0xba, 0x81, 0x10, 0x9a, 0x81, 0x02, 0x12, 0x54, 0xc3, 0x67,
    0xd0, 0x73, 0x73, 0xcd, 0x63, 0xc0, 0x8b, 0x63, 0xc5, 0x88, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x01, 0x67, 0xc8, 0x98, 0x45, 0xa3, 0x87, 0x45, 0x4e, 0x43, 0x4f, 0x44, 0x45, 0x52, 0x44,
    0x87, 0x8b, 0x4c, 0x61, 0x76, 0x63, 0x20, 0x6c, 0x69, 0x62, 0x76, 0x70, 0x78, 0x67, 0xc8, 0xa1,
    0x45, 0xa3, 0x88, 0x44, 0x55, 0x52, 0x41, 0x54, 0x49, 0x4f, 0x4e, 0x44, 0x87, 0x93, 0x30, 0x30,
    0x3a, 0x30, 0x30, 0x3a, 0x30, 0x30, 0x2e, 0x30, 0x34, 0x32, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30,
    0x00, 0x1f, 0x43, 0xb6, 0x75, 0xa8, 0xe7, 0x81, 0x00, 0xa3, 0xa3, 0x81, 0x00, 0x00, 0x80, 0x10,
    0x02, 0x00, 0x9d, 0x01, 0x2a, 0x10, 0x00, 0x10, 0x00, 0x00, 0x47, 0x08, 0x85, 0x85, 0x88, 0x85,
    0x84, 0x88, 0x02, 0x02, 0x00, 0x0c, 0x0d, 0x60, 0x00, 0xfe, 0xff, 0xab, 0x50, 0x80, 0x1c, 0x53,
    0xbb, 0x6b, 0x91, 0xbb, 0x8f, 0xb3, 0x81, 0x00, 0xb7, 0x8a, 0xf7, 0x81, 0x01, 0xf1, 0x82, 0x01,
    0x61, 0xf0, 0x81, 0x03,
];
