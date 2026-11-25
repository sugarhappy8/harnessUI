//! Running one Studio engine job (docs/dev/0.1.0/studio-local-design.md, "How a run works"
//! and "Progress, Stop, failures").
//!
//! [`run_job`] writes the prompt files, starts one `sd-cli` through the spawn helper (its own
//! process tree, a scrubbed environment, the engine's folder as working directory), reads its
//! progress from both pipes and gives back the files it wrote. Stop kills the whole tree;
//! running out of memory is tried once more with the weights offloaded (and the decoder
//! tiled, where the recipe allows it); a run that shows no progress
//! for [`RunLimits::stall_after`] is reported, never killed; every way a run ends without its
//! files removes the run's folder. [`list_devices`] asks the engine for its devices.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::AsyncRead;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::args::{PartRole, SdJob, build_args, prompt_file_text, prompt_files};
use super::devices::{SdDevice, parse_list_devices};
use super::engine::Engine;
use super::failure::{SdFailure, classify};
use super::progress::{LocalProgress, ProgressParser};
use crate::llama::backend::path_ending_with;
use crate::llama::launch::NO_BACKTRACE_ENV;
use crate::llama::lines::{LineReader, Tail};
use crate::spawn::{self, EnvSpec, ManagedChild, SpawnSpec, StdioMode};

/// How long a run may show no progress before [`RunEvent::Stalled`]: 10 minutes.
pub const STALL_AFTER: Duration = Duration::from_secs(600);

/// How long `sd-cli --list-devices` may take.
pub const LIST_DEVICES_TIMEOUT: Duration = Duration::from_secs(30);

/// Lines of the engine's output kept to name a failure, stdout and stderr merged.
const TAIL_LINES: usize = 60;

/// Lines of `--list-devices` output read from stdout; a computer has a few devices.
const DEVICE_LINES: usize = 256;

/// Lines waiting between the pipe readers and the runner.
const LINE_QUEUE: usize = 256;

/// `PATH`'s separator on this platform.
const PATH_SEPARATOR: char = if cfg!(windows) { ';' } else { ':' };

/// Tries at removing a run's folder, and the pause between them.
const REMOVE_TRIES: u32 = 3;
const REMOVE_PAUSE: Duration = Duration::from_millis(100);

/// The limits of one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLimits {
    /// How long the engine may show no progress before [`RunEvent::Stalled`]. It only
    /// reports: there is no timeout.
    pub stall_after: Duration,
}

impl Default for RunLimits {
    /// [`STALL_AFTER`] (600 s).
    fn default() -> Self {
        Self {
            stall_after: STALL_AFTER,
        }
    }
}

/// What a run tells while it goes.
#[derive(Debug, Clone, PartialEq)]
pub enum RunEvent {
    /// The engine moved on: a stage, a picture, a pass, a step, or a bar showing it at work.
    Progress(LocalProgress),
    /// No progress for [`RunLimits::stall_after`]. The engine keeps running; the card offers
    /// Stop and Keep waiting. Sent once for each silence: the next progress re-arms it.
    Stalled,
    /// The engine failed with this ([`SdFailure::OutOfMemory`]) and starts once more with
    /// the weights offloaded (and the decoder tiled, where the recipe allows it).
    Retrying(SdFailure),
}

/// What a run that succeeded made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    /// The files the engine wrote in the job's `out_dir`: the picture or the clip, or a
    /// batch's pictures in order.
    pub files: Vec<PathBuf>,
    /// The first start ran out of memory, and the files come from the second, with the
    /// weights offloaded.
    pub retried: bool,
    /// Milliseconds from the call to the end, a retry included.
    pub elapsed_ms: u64,
}

/// One `sd-cli` for `job` (and one more with the weights offloaded after a first
/// out-of-memory failure), after writing the prompt files.
///
/// Cancel kills the tree and waits for it; Cancel and every final failure remove
/// `job.out_dir` (the prompt files with it). After a success the folder holds the prompt files
/// and the outputs, and the caller removes it once it has moved the outputs.
///
/// - The files of [`prompt_files`] are written into `job.out_dir` (created), and no other:
///   [`prompt_file_text`] (the prompt without its typed LoRA tags, then the attached LoRAs'
///   tags) and the negative prompt as given, UTF-8 with no newline added. A blank negative
///   prompt has no file, as [`build_args`] passes none.
/// - The engine starts in `engine.dir` with `engine.leading_args` then [`build_args`]. Its
///   environment is the platform allowlist with `GGML_NO_BACKTRACE=1`; `PATH` then ends
///   with [`Engine::vulkan_fallback`] when the engine has one (llama.cpp's bundled Vulkan
///   runtime folder on Windows, so a PC whose driver lacks the Vulkan runtime still finds
///   it); then `job.env`, then `engine.extra_env`. Stdin is the null device.
/// - Both pipes are read with `\r` ending a line too, into one [`ProgressParser`]; each update
///   is a [`RunEvent::Progress`]. The last 60 lines of both, in arrival order, name a
///   failure ([`classify`]).
/// - Exit 0 gives the files in `job.out_dir` named as `sd-cli` names them: `<out_stem>.<ext>`
///   for one picture or a clip, `<out_stem>_<n>.<ext>` for a batch, in the order of `n`. An
///   exit 0 without such a file is a failure named from the output.
/// - [`SdFailure::OutOfMemory`] on a job without `offload_to_cpu`: [`RunEvent::Retrying`],
///   the files the engine left removed (the prompt files stay) and the same job once more
///   with `offload_to_cpu`, and `vae_tiling` as `tiling_allowed` says (the recipes that
///   disable tiling are never tiled). The second start's failure is final.
/// - [`SdFailure::EngineMissing`] when the program or its folder is not there,
///   [`SdFailure::EngineBlocked`] when the system refuses to start it, [`SdFailure::Io`]
///   when the run's folder cannot be written, or when [`build_args`] refuses the job (the
///   caller checks it first, and nothing is written then).
/// - No progress for `limits.stall_after` sends [`RunEvent::Stalled`] once; the next
///   progress re-arms it. It never stops the engine.
pub async fn run_job(
    engine: &Engine,
    job: &SdJob,
    limits: RunLimits,
    cancel: CancellationToken,
    on_event: &(dyn Fn(RunEvent) + Send + Sync),
) -> Result<RunOutput, SdFailure> {
    let started = Instant::now();
    let run = Run {
        engine,
        env: engine_env(engine, &job.env, std::env::var_os("PATH").as_deref()),
        limits,
        cancel: &cancel,
        on_event,
        started,
    };
    match run.with_retry(job).await {
        Ok((files, retried)) => Ok(RunOutput {
            files,
            retried,
            elapsed_ms: millis_since(started),
        }),
        Err(failure) => {
            remove_run_dir(&job.out_dir).await;
            Err(failure)
        }
    }
}

/// The devices the engine sees (`sd-cli --list-devices`), within [`LIST_DEVICES_TIMEOUT`].
///
/// The command is built as [`run_job`] builds it: in `engine.dir`, with `engine.leading_args`
/// then `--list-devices`, the platform allowlist with `GGML_NO_BACKTRACE=1`, `PATH` ending
/// with [`Engine::vulkan_fallback`] when the engine has one (on a PC whose driver lacks the
/// Vulkan runtime the card is listed only with it), and then `engine.extra_env`. The
/// engine's start fails as in [`run_job`]; a non-zero exit is named from its output
/// ([`classify`]); an engine that does not answer in time is stopped and is
/// [`SdFailure::Crashed`] without a code.
pub async fn list_devices(engine: &Engine) -> Result<Vec<SdDevice>, SdFailure> {
    let env = engine_env(engine, &[], std::env::var_os("PATH").as_deref());
    let spec = command(engine, [OsString::from("--list-devices")], env);
    let mut child = spawn::spawn(spec).map_err(not_started)?;
    let stdout = child.take_stdout();
    let stderr = child.take_stderr();
    let finished = async {
        tokio::join!(
            first_lines(stdout, DEVICE_LINES),
            last_lines(stderr, TAIL_LINES),
            child.wait()
        )
    };
    let Ok((stdout, stderr, status)) = tokio::time::timeout(LIST_DEVICES_TIMEOUT, finished).await
    else {
        stop(&mut child).await;
        tracing::warn!(
            seconds = LIST_DEVICES_TIMEOUT.as_secs(),
            "the Studio engine did not list its devices in time and was stopped"
        );
        return Err(SdFailure::Crashed { code: None });
    };
    let status = status.map_err(|error| {
        tracing::warn!(%error, "could not wait for the Studio engine");
        SdFailure::Crashed { code: None }
    })?;
    if status.success() {
        return Ok(parse_list_devices(&stdout.join("\n")));
    }
    let tail: Vec<String> = stdout.into_iter().chain(stderr).collect();
    let failure = classify(&tail, status.code());
    tracing::warn!(?failure, code = ?status.code(), "the Studio engine could not list its devices");
    Err(failure)
}

/// What every start of one run shares.
struct Run<'a> {
    engine: &'a Engine,
    env: EnvSpec,
    limits: RunLimits,
    cancel: &'a CancellationToken,
    on_event: &'a (dyn Fn(RunEvent) + Send + Sync),
    started: Instant,
}

impl Run<'_> {
    /// The prompt files, the first start, and the second after running out of memory: the
    /// files, and whether they come from the second start.
    async fn with_retry(&self, job: &SdJob) -> Result<(Vec<PathBuf>, bool), SdFailure> {
        let args = build_args(job).map_err(refused)?;
        if self.cancel.is_cancelled() {
            return Err(SdFailure::Cancelled);
        }
        write_prompt_files(job).await?;
        match self.start(job, args).await {
            Err(SdFailure::OutOfMemory) if !job.offload_to_cpu => {}
            other => return other.map(|files| (files, false)),
        }
        (self.on_event)(RunEvent::Retrying(SdFailure::OutOfMemory));
        remove_outputs(job).await;
        let saving = SdJob {
            offload_to_cpu: true,
            vae_tiling: job.tiling_allowed,
            ..job.clone()
        };
        let args = build_args(&saving).map_err(refused)?;
        self.start(&saving, args).await.map(|files| (files, true))
    }

    /// One engine process, from its start to its end: the files it wrote.
    async fn start(&self, job: &SdJob, args: Vec<OsString>) -> Result<Vec<PathBuf>, SdFailure> {
        if self.cancel.is_cancelled() {
            return Err(SdFailure::Cancelled);
        }
        let spec = command(self.engine, args, self.env.clone());
        let mut child = spawn::spawn(spec).map_err(not_started)?;
        let (sender, mut lines) = mpsc::channel(LINE_QUEUE);
        let stdout = child.take_stdout();
        let stderr = child.take_stderr();
        // Each pipe is read to its end by its own reader (a read is not cancel-safe), and the
        // lines meet in the channel in the order they arrive.
        let readers =
            async move { tokio::join!(forward(stdout, sender.clone()), forward(stderr, sender)) };
        tokio::pin!(readers);
        let mut readers_done = false;
        let mut parser = ProgressParser::new(pictures(job), passes(job));
        let mut tail = Tail::new(TAIL_LINES);
        let stall = tokio::time::sleep_until(deadline(self.limits.stall_after));
        tokio::pin!(stall);
        let mut stalled = false;
        loop {
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => {
                    stop(&mut child).await;
                    return Err(SdFailure::Cancelled);
                }
                line = lines.recv() => {
                    let Some(line) = line else { break };
                    if let Some(progress) = parser.feed(&line, millis_since(self.started)) {
                        stall.as_mut().reset(deadline(self.limits.stall_after));
                        stalled = false;
                        (self.on_event)(RunEvent::Progress(progress));
                    }
                    tail.push(line);
                }
                ((), ()) = &mut readers, if !readers_done => readers_done = true,
                () = &mut stall, if !stalled => {
                    stalled = true;
                    (self.on_event)(RunEvent::Stalled);
                }
            }
        }
        // Both pipes are closed: the engine is exiting.
        let status = tokio::select! {
            biased;
            () = self.cancel.cancelled() => None,
            status = child.wait() => Some(status),
        };
        let Some(status) = status else {
            stop(&mut child).await;
            return Err(SdFailure::Cancelled);
        };
        let tail = tail.to_vec();
        let status = status.map_err(|error| {
            tracing::warn!(%error, "could not wait for the Studio engine");
            SdFailure::Crashed { code: None }
        })?;
        if status.success() {
            let files = outputs(job)
                .await
                .map_err(|error| SdFailure::Io(format!("reading the run's folder: {error}")))?;
            if !files.is_empty() {
                return Ok(files);
            }
        }
        let failure = classify(&tail, status.code());
        tracing::warn!(?failure, code = ?status.code(), "the Studio engine failed");
        tracing::debug!(?tail, "the Studio engine's last lines");
        Err(failure)
    }
}

/// The engine's command: `engine.program` in `engine.dir`, `engine.leading_args` then `args`,
/// stdin from the null device, stdout and stderr piped.
fn command(engine: &Engine, args: impl IntoIterator<Item = OsString>, env: EnvSpec) -> SpawnSpec {
    SpawnSpec::new(&engine.program, &engine.dir)
        .args(engine.leading_args.iter().cloned())
        .args(args)
        .env(env)
        .stdin(StdioMode::Null)
        .stdout(StdioMode::Piped)
        .stderr(StdioMode::Piped)
}

/// The engine's environment, for a job (`job_env` its variables) and for `--list-devices`
/// (none) alike: the platform allowlist and `GGML_NO_BACKTRACE=1`; `PATH`, the app's own
/// (`parent_path`), with [`Engine::vulkan_fallback`] as its last entry when the engine has
/// one, and inherited unchanged when it has none; then `job_env`, then `engine.extra_env`, a
/// later name replacing an earlier one.
fn engine_env(
    engine: &Engine,
    job_env: &[(String, String)],
    parent_path: Option<&OsStr>,
) -> EnvSpec {
    let mut env = EnvSpec::minimal().set(NO_BACKTRACE_ENV, "1");
    if let Some(folder) = &engine.vulkan_fallback {
        let path = path_ending_with(parent_path, folder, PATH_SEPARATOR);
        env = env.set("PATH", path.to_string_lossy().into_owned());
    }
    for (name, value) in job_env.iter().chain(&engine.extra_env) {
        env = env.set(name.clone(), value.clone());
    }
    env
}

/// A start that failed: the program or its folder is not there, or the system refused it.
fn not_started(error: io::Error) -> SdFailure {
    if error.kind() == io::ErrorKind::NotFound {
        SdFailure::EngineMissing
    } else {
        SdFailure::EngineBlocked(error.to_string())
    }
}

/// A job [`build_args`] refused.
fn refused(error: super::args::ArgsError) -> SdFailure {
    SdFailure::Io(format!("the engine's arguments: {error}"))
}

/// Pictures the parser counts: the batch, or one clip.
fn pictures(job: &SdJob) -> u32 {
    if job.frames.is_some() { 1 } else { job.batch }
}

/// Drawing passes: two for a model with a high-noise part.
fn passes(job: &SdJob) -> u32 {
    if job
        .parts
        .iter()
        .any(|(role, _)| *role == PartRole::HighNoiseDiffusion)
    {
        2
    } else {
        1
    }
}

/// Sends each line of `pipe` (`\r` ends a line too) until its end, or until the runner is gone.
async fn forward<R: AsyncRead + Unpin>(pipe: Option<R>, sender: mpsc::Sender<String>) {
    let Some(pipe) = pipe else {
        return;
    };
    let mut reader = LineReader::splitting_cr(pipe);
    while let Some(line) = reader.next_line().await {
        if sender.send(line).await.is_err() {
            return;
        }
    }
}

/// The first `keep` lines of `pipe`, read to its end.
async fn first_lines<R: AsyncRead + Unpin>(pipe: Option<R>, keep: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(pipe) = pipe {
        let mut reader = LineReader::new(pipe);
        while let Some(line) = reader.next_line().await {
            if lines.len() < keep {
                lines.push(line);
            }
        }
    }
    lines
}

/// The last `keep` lines of `pipe`, read to its end.
async fn last_lines<R: AsyncRead + Unpin>(pipe: Option<R>, keep: usize) -> Vec<String> {
    let mut tail = Tail::new(keep);
    if let Some(pipe) = pipe {
        let mut reader = LineReader::new(pipe);
        while let Some(line) = reader.next_line().await {
            tail.push(line);
        }
    }
    tail.to_vec()
}

/// Kills the engine's whole tree and waits for it.
async fn stop(child: &mut ManagedChild) {
    if let Err(error) = child.start_kill() {
        tracing::warn!(%error, "could not stop the Studio engine");
    }
    if let Err(error) = child.wait().await {
        tracing::warn!(%error, "could not wait for the stopped Studio engine");
    }
}

/// Writes the prompt files into the run's folder, creating it: exactly the files of
/// [`prompt_files`], the paths [`build_args`] passes, so a blank negative prompt has none. The
/// prompt's file holds [`prompt_file_text`], UTF-8 with no newline added.
async fn write_prompt_files(job: &SdJob) -> Result<(), SdFailure> {
    let failed = |error: io::Error| SdFailure::Io(format!("writing the run's folder: {error}"));
    tokio::fs::create_dir_all(&job.out_dir)
        .await
        .map_err(failed)?;
    let (prompt, negative) = prompt_files(job);
    tokio::fs::write(&prompt, prompt_file_text(job).as_bytes())
        .await
        .map_err(failed)?;
    if let (Some(path), Some(text)) = (negative, job.negative.as_deref()) {
        tokio::fs::write(&path, text.as_bytes())
            .await
            .map_err(failed)?;
    }
    Ok(())
}

/// The engine's files in the run's folder, in the order of their number.
async fn outputs(job: &SdJob) -> io::Result<Vec<PathBuf>> {
    let mut found: Vec<(Option<u64>, PathBuf)> = Vec::new();
    let mut entries = tokio::fs::read_dir(&job.out_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_type().await?.is_file() {
            continue;
        }
        let path = entry.path();
        if is_prompt_file(job, &path) {
            continue;
        }
        let index = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| output_index(name, &job.out_stem));
        if let Some(index) = index {
            found.push((index, path));
        }
    }
    found.sort();
    Ok(found.into_iter().map(|(_, path)| path).collect())
}

/// Where `name` stands among the engine's files for `stem`: `Some(None)` for `<stem>.<ext>`,
/// `Some(Some(n))` for `<stem>_<n>.<ext>`, `None` for any other name. The extension is
/// letters and digits.
fn output_index(name: &str, stem: &str) -> Option<Option<u64>> {
    let rest = name.strip_prefix(stem)?;
    let (number, extension) = match rest.strip_prefix('_') {
        Some(numbered) => {
            let (digits, extension) = numbered.split_once('.')?;
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            (Some(digits.parse().ok()?), extension)
        }
        None => (None, rest.strip_prefix('.')?),
    };
    let plain = !extension.is_empty() && extension.bytes().all(|b| b.is_ascii_alphanumeric());
    plain.then_some(number)
}

fn is_prompt_file(job: &SdJob, path: &Path) -> bool {
    let (prompt, negative) = prompt_files(job);
    path == prompt || negative.as_deref() == Some(path)
}

/// Removes every file in the run's folder but the prompt files, before the second start. A
/// folder that cannot be read is logged; the second start then finds what is left.
async fn remove_outputs(job: &SdJob) {
    let unreadable = |error: io::Error| {
        tracing::warn!(%error, dir = %job.out_dir.display(), "could not read a Studio run's folder to remove the files the engine left");
    };
    let mut entries = match tokio::fs::read_dir(&job.out_dir).await {
        Ok(entries) => entries,
        Err(error) => return unreadable(error),
    };
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => return,
            Err(error) => return unreadable(error),
        };
        let path = entry.path();
        if is_prompt_file(job, &path) {
            continue;
        }
        if let Err(error) = tokio::fs::remove_file(&path).await {
            tracing::warn!(%error, path = %path.display(), "could not remove a file the Studio engine left");
        }
    }
}

/// Removes the run's folder. A file the system still holds for a moment (an antivirus scan
/// of a new picture on Windows) is tried again; a folder still there after that is logged.
async fn remove_run_dir(dir: &Path) {
    for attempt in 1..=REMOVE_TRIES {
        match tokio::fs::remove_dir_all(dir).await {
            Ok(()) => return,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
            Err(_) if attempt < REMOVE_TRIES => tokio::time::sleep(REMOVE_PAUSE).await,
            Err(error) => {
                tracing::warn!(%error, dir = %dir.display(), "could not remove a Studio run's folder");
            }
        }
    }
}

/// `after` from now, or a year from now when that is beyond the clock.
fn deadline(after: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(after)
        .unwrap_or_else(|| now + Duration::from_secs(365 * 24 * 60 * 60))
}

fn millis_since(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    use super::*;
    use crate::sd::args::{LoraTag, OutputFormat};
    use crate::sd::engine::EngineSource;

    fn engine(dir: &Path) -> Engine {
        Engine {
            program: dir.join("sd-cli"),
            dir: dir.to_path_buf(),
            build: 1106,
            source: EngineSource::Dev,
            leading_args: Vec::new(),
            extra_env: Vec::new(),
            vulkan_fallback: None,
        }
    }

    fn job(out_dir: &Path) -> SdJob {
        SdJob {
            parts: vec![(PartRole::Diffusion, out_dir.join("d.gguf"))],
            prompt: "a lighthouse".into(),
            negative: None,
            width: 512,
            height: 512,
            size_multiple: 16,
            steps: 4,
            cfg: 1.0,
            guidance: None,
            img_cfg: None,
            flow_shift: None,
            timestep_shift: None,
            sampler: None,
            scheduler: None,
            sigmas: Vec::new(),
            high_noise: None,
            moe_boundary: None,
            strength: None,
            seed: 1,
            batch: 1,
            frames: None,
            fps: None,
            init_image: None,
            end_image: None,
            mask_image: None,
            control_image: None,
            vae_format: None,
            model_args: None,
            lora_dir: None,
            loras: Vec::new(),
            ref_images: Vec::new(),
            backend: None,
            vae_tiling: false,
            offload_to_cpu: false,
            tiling_allowed: true,
            processor_only: false,
            threads: None,
            out_dir: out_dir.to_path_buf(),
            out_stem: "r1".into(),
            format: OutputFormat::Png,
            env: Vec::new(),
        }
    }

    /// The environment `env` gives a child of a parent holding `parent`.
    fn resolved(env: &EnvSpec, parent: &[(&str, &str)]) -> BTreeMap<String, String> {
        env.resolve_from(
            parent
                .iter()
                .map(|(name, value)| (OsString::from(name), OsString::from(value))),
        )
        .into_iter()
        .map(|(name, value)| {
            (
                name.into_string().expect("Unicode"),
                value.into_string().expect("Unicode"),
            )
        })
        .collect()
    }

    #[test]
    fn the_command_starts_with_the_leading_arguments_in_the_engines_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = Engine {
            leading_args: vec!["fake_sd_process".into(), "--".into()],
            ..engine(tmp.path())
        };
        let spec = command(
            &engine,
            [OsString::from("--list-devices")],
            EnvSpec::empty(),
        );
        assert_eq!(spec.program, engine.program);
        assert_eq!(spec.cwd, engine.dir);
        assert_eq!(spec.args, ["fake_sd_process", "--", "--list-devices"]);
        assert_eq!(
            (spec.stdin, spec.stdout, spec.stderr),
            (StdioMode::Null, StdioMode::Piped, StdioMode::Piped)
        );
    }

    #[test]
    fn the_environment_is_the_allowlist_then_the_job_then_the_engine() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = Engine {
            extra_env: vec![("SHARED".into(), "engine".into())],
            ..engine(tmp.path())
        };
        let job_env = [
            ("SHARED".to_owned(), "job".to_owned()),
            ("GGML_METAL_TENSOR_DISABLE".to_owned(), "1".to_owned()),
        ];
        let env = engine_env(&engine, &job_env, Some(OsStr::new("/bin")));
        let child = resolved(&env, &[("PATH", "/bin"), ("HF_TOKEN", "secret")]);
        assert_eq!(child.get(NO_BACKTRACE_ENV).map(String::as_str), Some("1"));
        assert_eq!(child.get("SHARED").map(String::as_str), Some("engine"));
        assert_eq!(
            child.get("GGML_METAL_TENSOR_DISABLE").map(String::as_str),
            Some("1")
        );
        assert!(!child.contains_key("HF_TOKEN"));
    }

    /// The engine's Vulkan fallback folder (recorded by discovery on Windows) ends `PATH` for
    /// a job and for `--list-devices` alike; an engine without one leaves `PATH` as it is.
    #[test]
    fn path_ends_with_the_engines_vulkan_folder_for_a_job_and_for_the_devices() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("llama").join("main").join("vulkan");
        let with = Engine {
            vulkan_fallback: Some(folder.clone()),
            ..engine(tmp.path())
        };
        let without = engine(tmp.path());
        let parent_path = "/usr/bin";
        let parent = [("PATH", parent_path), ("HF_TOKEN", "secret")];
        let path_of = |engine: &Engine, job_env: &[(String, String)]| {
            let env = engine_env(engine, job_env, Some(OsStr::new(parent_path)));
            resolved(&env, &parent).remove("PATH")
        };
        let job_env = [("GGML_METAL_TENSOR_DISABLE".to_owned(), "1".to_owned())];
        // run_job passes the job's variables; list_devices passes none
        for job_env in [&job_env[..], &[]] {
            let path = path_of(&with, job_env).expect("PATH");
            assert!(
                path.ends_with(folder.to_str().unwrap()),
                "{path} does not end with {}",
                folder.display()
            );
            assert_eq!(
                path,
                format!("{parent_path}{PATH_SEPARATOR}{}", folder.display())
            );
            assert_eq!(path_of(&without, job_env).as_deref(), Some(parent_path));
        }
        // no PATH in the app's environment: the folder alone
        let env = engine_env(&with, &[], None);
        assert_eq!(
            resolved(&env, &[]).remove("PATH"),
            Some(folder.display().to_string())
        );
        assert_eq!(
            resolved(&engine_env(&without, &[], None), &[]).get("PATH"),
            None
        );
    }

    #[test]
    fn the_engines_files_are_its_stem_with_an_optional_number() {
        for (name, index) in [
            ("r1.png", Some(None)),
            ("r1.webm", Some(None)),
            ("r1_0.png", Some(Some(0))),
            ("r1_12.png", Some(Some(12))),
            ("r10.png", None),
            ("r1_.png", None),
            ("r1_+1.png", None),
            ("r1_x.png", None),
            ("r1.png.part", None),
            ("r1.", None),
            ("prompt.txt", None),
        ] {
            assert_eq!(output_index(name, "r1"), index, "{name}");
        }
    }

    #[tokio::test]
    async fn a_batch_is_listed_in_the_order_of_its_numbers_without_the_prompt_files() {
        let tmp = tempfile::tempdir().unwrap();
        let job = SdJob {
            negative: Some("blurry".into()),
            out_stem: "prompt".into(),
            ..job(tmp.path())
        };
        write_prompt_files(&job).await.unwrap();
        for name in ["prompt_10.png", "prompt_2.png", "prompt_0.png", "other.png"] {
            std::fs::write(tmp.path().join(name), b"x").unwrap();
        }
        assert_eq!(
            outputs(&job).await.unwrap(),
            ["prompt_0.png", "prompt_2.png", "prompt_10.png"].map(|name| tmp.path().join(name))
        );
        remove_outputs(&job).await;
        let mut left: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["negative.txt", "prompt.txt"]);
    }

    #[tokio::test]
    async fn the_prompt_files_hold_the_prompt_exactly() {
        let tmp = tempfile::tempdir().unwrap();
        let job = SdJob {
            prompt: "two lines\nand 風, no newline at the end".into(),
            negative: Some(" ".into()),
            ..job(&tmp.path().join(".partial").join("r1"))
        };
        write_prompt_files(&job).await.unwrap();
        assert_eq!(
            std::fs::read(job.out_dir.join("prompt.txt")).unwrap(),
            job.prompt.as_bytes()
        );
        // a blank negative prompt has no file
        assert!(!job.out_dir.join("negative.txt").exists());
    }

    /// The spans the engine's pattern `<lora:([^:>]+):([^>]+)>` finds in `text`
    /// (`extract_and_remove_lora`, `examples/common/common.cpp:2356` at the pinned commit),
    /// applied by hand as `std::regex_search` applies it to the file's bytes: the leftmost
    /// match, then on from its end. Each is its name and its weight.
    fn engine_lora_spans(text: &[u8]) -> Vec<(String, String)> {
        let open = b"<lora:";
        let mut spans = Vec::new();
        let mut at = 0;
        while at < text.len() {
            if !text[at..].starts_with(open) {
                at += 1;
                continue;
            }
            let name_start = at + open.len();
            let name_end = (name_start..text.len())
                .find(|&i| text[i] == b':' || text[i] == b'>')
                .filter(|&i| i > name_start && text[i] == b':');
            let Some(name_end) = name_end else {
                at += 1;
                continue;
            };
            let weight_start = name_end + 1;
            let weight_end = (weight_start..text.len())
                .find(|&i| text[i] == b'>')
                .filter(|&i| i > weight_start);
            let Some(weight_end) = weight_end else {
                at += 1;
                continue;
            };
            let utf8 = |bytes: &[u8]| String::from_utf8(bytes.to_vec()).expect("UTF-8");
            spans.push((
                utf8(&text[name_start..name_end]),
                utf8(&text[weight_start..weight_end]),
            ));
            at = weight_end + 1;
        }
        spans
    }

    /// A typed image marker would make the engine load any file it names, and one left open
    /// makes it throw (ruling S2-image-markers): neither reaches the prompt's file.
    #[tokio::test]
    async fn typed_image_markers_never_reach_the_prompt_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = SdJob {
            prompt: "a fox <image:/etc/passwd> in snow <image:x".into(),
            ..job(dir.path())
        };
        j.lora_dir = Some(dir.path().join("@loras"));
        j.loras = vec![LoraTag {
            name: "style".into(),
            weight: 1.0,
            high_noise: false,
        }];
        write_prompt_files(&j).await.unwrap();
        let text = std::fs::read_to_string(dir.path().join("prompt.txt")).unwrap();
        assert_eq!(text, "a fox  in snow x <lora:style:1>");
        assert!(!text.contains("<image:"));
    }

    #[tokio::test]
    async fn lora_tags_reach_the_prompt_file_with_unicode_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = SdJob {
            prompt: "a red fox".into(),
            ..job(dir.path())
        };
        j.lora_dir = Some(dir.path().join("@loras"));
        j.loras = vec![
            LoraTag {
                name: "local/pixel art ü".into(),
                weight: 0.8,
                high_noise: false,
            },
            LoraTag {
                name: "o/r/日本語 スタイル".into(),
                weight: 1.0,
                high_noise: true,
            },
        ];
        write_prompt_files(&j).await.unwrap();
        let text = std::fs::read(dir.path().join("prompt.txt")).unwrap();
        assert_eq!(
            String::from_utf8(text.clone()).unwrap(),
            "a red fox <lora:local/pixel art ü:0.8> <lora:|high_noise|o/r/日本語 スタイル:1>"
        );
        // the engine's pattern <lora:([^:>]+):([^>]+)> (common.cpp:2356) finds both, names intact
        assert_eq!(
            engine_lora_spans(&text),
            [
                ("local/pixel art ü".into(), "0.8".into()),
                ("|high_noise|o/r/日本語 スタイル".into(), "1".into())
            ]
        );
        // a tag typed into the prompt, whole or left open, is not among them
        j.prompt = "a red fox <lora:evil:3> and <lora:open:2".into();
        write_prompt_files(&j).await.unwrap();
        let text = std::fs::read(dir.path().join("prompt.txt")).unwrap();
        assert_eq!(
            engine_lora_spans(&text)
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            ["local/pixel art ü", "|high_noise|o/r/日本語 スタイル"]
        );
    }

    #[tokio::test]
    async fn a_job_the_arguments_refuse_and_a_cancelled_one_write_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let out_dir = tmp.path().join(".partial").join("r1");
        let refused = SdJob {
            prompt: " ".into(),
            ..job(&out_dir)
        };
        let result = run_job(
            &engine(tmp.path()),
            &refused,
            RunLimits::default(),
            CancellationToken::new(),
            &|_| {},
        )
        .await;
        assert!(matches!(result, Err(SdFailure::Io(_))), "{result:?}");
        assert!(!out_dir.exists());

        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = run_job(
            &engine(tmp.path()),
            &job(&out_dir),
            RunLimits::default(),
            cancel,
            &|_| {},
        )
        .await;
        assert_eq!(result, Err(SdFailure::Cancelled));
        assert!(!out_dir.exists());
    }

    #[test]
    fn the_default_stall_limit_is_ten_minutes() {
        assert_eq!(RunLimits::default().stall_after, Duration::from_secs(600));
        assert_eq!(LIST_DEVICES_TIMEOUT, Duration::from_secs(30));
    }

    #[test]
    fn a_run_can_be_moved_to_another_task() {
        fn sendable<T: Send>(_: &T) {}
        let tmp = tempfile::tempdir().unwrap();
        let (engine, job) = (engine(tmp.path()), job(tmp.path()));
        let on_event = |_: RunEvent| {};
        let run = run_job(
            &engine,
            &job,
            RunLimits::default(),
            CancellationToken::new(),
            &on_event,
        );
        sendable(&run);
        let devices = list_devices(&engine);
        sendable(&devices);
    }
}
