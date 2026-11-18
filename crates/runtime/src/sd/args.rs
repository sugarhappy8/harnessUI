//! The Studio engine's arguments, only from typed fields (docs/dev/0.1.0/studio-local-design.md,
//! "Arguments come only from typed fields").
//!
//! [`build_args`] gives each model part's role one fixed flag ([`flag_for`]), then numbers,
//! then whitelisted words ([`Sampler`], [`Scheduler`], the probed device or `cpu`,
//! `--vae-tiling`, `--offload-to-cpu`), then `-o` and the fixed flags, so a signed catalog
//! cannot add a flag. The prompt never reaches argv: `run_job` writes it to the files
//! [`prompt_files`] names and the engine reads them (`--prompt-file`). The pinned parser
//! matches only `arg == name`, so it has no `--prompt=<text>` form; a string option takes the
//! next argument as its value whatever it starts with, and `--prompt-file` reads the whole
//! file as bytes.
//!
//! [`FLAGS`] lists every flag [`build_args`] can emit; [`missing_from_help`] checks them, and
//! the sampler and scheduler words, against `sd-cli -h`: the unit tests on the pinned build's
//! help in `tests/fixtures/sd_help.txt`, the contract tests (`tests/sd_contract.rs`) on the
//! engine itself.

use std::ffi::OsString;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

/// The longest local prompt, in characters: a sanity cap, as the prompt goes by file and
/// never in argv.
pub const PROMPT_MAX_CHARS: usize = 4_000;

/// Picture and video sides are a multiple of this many pixels.
const SIZE_MULTIPLE: u32 = 16;

/// The sides the engine is given, in pixels.
const SIZE_RANGE: RangeInclusive<u32> = 64..=2048;

/// The prompt's file name in the run's folder.
const PROMPT_FILE: &str = "prompt.txt";

/// The negative prompt's file name in the run's folder.
const NEGATIVE_FILE: &str = "negative.txt";

/// What a model part is to the engine, from the catalog's part role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartRole {
    /// The diffusion model on its own (`--diffusion-model`).
    Diffusion,
    /// The high-noise diffusion model of a two-model video family
    /// (`--high-noise-diffusion-model`).
    HighNoiseDiffusion,
    /// A language model used as the text encoder (`--llm`).
    Llm,
    /// The T5-XXL text encoder (`--t5xxl`).
    T5xxl,
    /// The CLIP-L text encoder (`--clip_l`).
    ClipL,
    /// The CLIP vision encoder (`--clip_vision`).
    ClipVision,
    /// The decoder (`--vae`).
    Vae,
    /// One file holding the whole model (`-m`).
    Single,
}

/// The one flag the engine takes a part of `role` with.
pub fn flag_for(role: PartRole) -> &'static str {
    match role {
        PartRole::Diffusion => "--diffusion-model",
        PartRole::HighNoiseDiffusion => "--high-noise-diffusion-model",
        PartRole::Llm => "--llm",
        PartRole::T5xxl => "--t5xxl",
        PartRole::ClipL => "--clip_l",
        PartRole::ClipVision => "--clip_vision",
        PartRole::Vae => "--vae",
        PartRole::Single => "-m",
    }
}

/// The sampling methods Studio offers, a whitelist of the pinned CLI's
/// `--sampling-method` words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sampler {
    /// `euler`.
    Euler,
    /// `euler_a`.
    EulerA,
    /// `heun`.
    Heun,
    /// `dpm2`.
    Dpm2,
    /// `dpm++2m`.
    DpmPp2m,
    /// `dpm++2mv2`.
    DpmPp2mV2,
    /// `lcm`.
    Lcm,
}

impl Sampler {
    /// Every sampler, in the order the pinned CLI lists them.
    pub const ALL: &'static [Self] = &[
        Self::Euler,
        Self::EulerA,
        Self::Heun,
        Self::Dpm2,
        Self::DpmPp2m,
        Self::DpmPp2mV2,
        Self::Lcm,
    ];

    /// The CLI's word for this sampler.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Euler => "euler",
            Self::EulerA => "euler_a",
            Self::Heun => "heun",
            Self::Dpm2 => "dpm2",
            Self::DpmPp2m => "dpm++2m",
            Self::DpmPp2mV2 => "dpm++2mv2",
            Self::Lcm => "lcm",
        }
    }

    /// The sampler whose CLI word is exactly `word`; `None` for any other word.
    pub fn from_cli(word: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.as_str() == word)
    }
}

/// The schedulers Studio offers, a whitelist of the pinned CLI's `--scheduler` words.
/// No scheduler (`None` in [`SdJob`]) leaves the engine its model-specific default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheduler {
    /// `discrete`.
    Discrete,
    /// `karras`.
    Karras,
    /// `exponential`.
    Exponential,
    /// `simple`.
    Simple,
}

impl Scheduler {
    /// Every scheduler, in the order the pinned CLI lists them.
    pub const ALL: &'static [Self] = &[
        Self::Discrete,
        Self::Karras,
        Self::Exponential,
        Self::Simple,
    ];

    /// The CLI's word for this scheduler.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discrete => "discrete",
            Self::Karras => "karras",
            Self::Exponential => "exponential",
            Self::Simple => "simple",
        }
    }

    /// The scheduler whose CLI word is exactly `word`; `None` for any other word.
    pub fn from_cli(word: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.as_str() == word)
    }
}

/// The file the engine writes: a PNG picture, or a WebM (VP8) or animated WebP video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// A PNG picture.
    Png,
    /// A WebM (VP8) video.
    Webm,
    /// An animated WebP video, the fallback when WebM does not play.
    Webp,
}

impl OutputFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Webm => "webm",
            Self::Webp => "webp",
        }
    }
}

/// One local run, in typed fields only. The numbers come checked against the catalog's
/// ranges (the engine reads them as 32-bit integers, the seed as a 64-bit one).
#[derive(Debug, Clone, PartialEq)]
pub struct SdJob {
    /// The model's files, each with its role, in the catalog's order.
    pub parts: Vec<(PartRole, PathBuf)>,
    /// The prompt, written to its file by `run_job` exactly as given.
    pub prompt: String,
    /// The negative prompt; a blank one is none.
    pub negative: Option<String>,
    /// Width in pixels: a multiple of 16 from 64 to 2048.
    pub width: u32,
    /// Height in pixels: a multiple of 16 from 64 to 2048.
    pub height: u32,
    /// Sampling steps.
    pub steps: u32,
    /// Guidance scale.
    pub cfg: f32,
    /// The flow shift of flow models; `None` lets the engine choose.
    pub flow_shift: Option<f32>,
    /// The sampling method; `None` lets the engine choose.
    pub sampler: Option<Sampler>,
    /// The scheduler; `None` lets the engine choose.
    pub scheduler: Option<Scheduler>,
    /// The seed Studio picked.
    pub seed: u64,
    /// How many pictures, one after another.
    pub batch: u32,
    /// Frames of a video; `Some` makes the run a video (`-M vid_gen`).
    pub frames: Option<u32>,
    /// Frames a second of a video.
    pub fps: Option<u32>,
    /// The start frame of a video, or the picture a picture starts from.
    pub init_image: Option<PathBuf>,
    /// Reference pictures, each passed with its own `-r`.
    pub ref_images: Vec<PathBuf>,
    /// The device from [`super::devices::match_llama_device`]; `None` lets the engine choose.
    pub backend: Option<String>,
    /// Decode in tiles and keep the weights in memory until needed (`--vae-tiling
    /// --offload-to-cpu`).
    pub memory_saving: bool,
    /// Run on the processor (`--backend cpu`), whatever `backend` says.
    pub processor_only: bool,
    /// Processor threads ([`processor_threads`]); `None` lets the engine choose.
    pub threads: Option<u32>,
    /// The run's folder (`<gallery>/.partial/<run>`): the prompt files and the output go in it.
    pub out_dir: PathBuf,
    /// The output's file name without its extension.
    pub out_stem: String,
    /// The output's format.
    pub format: OutputFormat,
    /// Variables added to the engine's environment, only from compiled-in quirks.
    pub env: Vec<(String, String)>,
}

/// Why [`build_args`] refused a job.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArgsError {
    /// The prompt is empty or only white space.
    #[error("the prompt is empty")]
    EmptyPrompt,
    /// The prompt is longer than [`PROMPT_MAX_CHARS`] characters.
    #[error("the prompt is longer than {PROMPT_MAX_CHARS} characters")]
    PromptTooLong,
    /// A path is not valid Unicode (it is never converted lossily).
    #[error("the path {} is not valid Unicode", .0.display())]
    NotUnicode(PathBuf),
    /// No part is a diffusion model (`--diffusion-model` or `-m`).
    #[error("no part is a diffusion model")]
    NoDiffusionPart,
    /// The width or height is not a multiple of 16 from 64 to 2048.
    #[error("the size is not a multiple of 16 from 64 to 2048")]
    BadSize,
    /// The output path holds a numbering template ([`is_numbering_template`]).
    #[error("the output path holds a %d numbering template")]
    PercentInOutputPath,
}

/// Every flag [`build_args`] can emit, in its order. The contract tests check each against the
/// pinned `sd-cli -h`.
pub const FLAGS: &[&str] = &[
    "--diffusion-model",
    "--high-noise-diffusion-model",
    "--llm",
    "--t5xxl",
    "--clip_l",
    "--clip_vision",
    "--vae",
    "-m",
    "-M",
    "--prompt-file",
    "--negative-prompt-file",
    "-W",
    "-H",
    "--steps",
    "--cfg-scale",
    "--flow-shift",
    "--sampling-method",
    "--scheduler",
    "--seed",
    "--batch-count",
    "--video-frames",
    "--fps",
    "-i",
    "-r",
    "--backend",
    "--threads",
    "--vae-tiling",
    "--offload-to-cpu",
    "-o",
    "--disable-image-metadata",
    "--log-level",
];

/// The engine's arguments for `job`, in a fixed order: the parts (one fixed flag each),
/// `-M vid_gen` for a video, `--prompt-file` and `--negative-prompt-file` (when there is a
/// negative prompt), `-W`, `-H`, `--steps`, `--cfg-scale`, then `--flow-shift`,
/// `--sampling-method` and `--scheduler` when set, `--seed`, `--batch-count`, then
/// `--video-frames`, `--fps`, `-i` and each `-r` when set, `--backend` (`cpu` when
/// `processor_only`), `--threads` when set, `--vae-tiling --offload-to-cpu` for memory
/// saving, `-o <out_dir>/<out_stem>.<png|webm|webp>`, `--disable-image-metadata` (the
/// engine's own record holds the prompt) and `--log-level info` (the progress reader reads
/// the info lines).
///
/// The prompt is trimmed only to check it is not empty, and never reaches argv.
pub fn build_args(job: &SdJob) -> Result<Vec<OsString>, ArgsError> {
    if job.prompt.trim().is_empty() {
        return Err(ArgsError::EmptyPrompt);
    }
    if job.prompt.chars().count() > PROMPT_MAX_CHARS {
        return Err(ArgsError::PromptTooLong);
    }
    if !job
        .parts
        .iter()
        .any(|(role, _)| matches!(role, PartRole::Diffusion | PartRole::Single))
    {
        return Err(ArgsError::NoDiffusionPart);
    }
    if !(fits(job.width) && fits(job.height)) {
        return Err(ArgsError::BadSize);
    }
    unicode(&job.out_dir)?;
    let output = job
        .out_dir
        .join(format!("{}.{}", job.out_stem, job.format.extension()));
    let output = unicode(&output)?.to_owned();
    if is_numbering_template(&output) {
        return Err(ArgsError::PercentInOutputPath);
    }
    let (prompt_file, negative_file) = prompt_files(job);

    let mut args: Vec<String> = Vec::new();
    let mut push = |flag: &str, value: String| args.extend([flag.to_owned(), value]);
    for (role, path) in &job.parts {
        push(flag_for(*role), unicode(path)?.to_owned());
    }
    if job.frames.is_some() {
        push("-M", "vid_gen".to_owned());
    }
    push("--prompt-file", unicode(&prompt_file)?.to_owned());
    if let Some(file) = &negative_file {
        push("--negative-prompt-file", unicode(file)?.to_owned());
    }
    push("-W", job.width.to_string());
    push("-H", job.height.to_string());
    push("--steps", job.steps.to_string());
    push("--cfg-scale", job.cfg.to_string());
    if let Some(shift) = job.flow_shift {
        push("--flow-shift", shift.to_string());
    }
    if let Some(sampler) = job.sampler {
        push("--sampling-method", sampler.as_str().to_owned());
    }
    if let Some(scheduler) = job.scheduler {
        push("--scheduler", scheduler.as_str().to_owned());
    }
    push("--seed", job.seed.to_string());
    push("--batch-count", job.batch.to_string());
    if let Some(frames) = job.frames {
        push("--video-frames", frames.to_string());
    }
    if let Some(fps) = job.fps {
        push("--fps", fps.to_string());
    }
    if let Some(init) = &job.init_image {
        push("-i", unicode(init)?.to_owned());
    }
    for reference in &job.ref_images {
        push("-r", unicode(reference)?.to_owned());
    }
    let device = if job.processor_only {
        Some("cpu")
    } else {
        job.backend.as_deref()
    };
    if let Some(device) = device {
        push("--backend", device.to_owned());
    }
    if let Some(threads) = job.threads {
        push("--threads", threads.to_string());
    }
    if job.memory_saving {
        args.extend(["--vae-tiling", "--offload-to-cpu"].map(str::to_owned));
    }
    args.extend(
        [
            "-o",
            &output,
            "--disable-image-metadata",
            "--log-level",
            "info",
        ]
        .map(str::to_owned),
    );
    Ok(args.into_iter().map(OsString::from).collect())
}

/// Where `run_job` writes the prompt (and the negative prompt, when there is one) before the
/// engine starts: inside `out_dir`, as `prompt.txt` and `negative.txt`. A blank negative
/// prompt has no file.
pub fn prompt_files(job: &SdJob) -> (PathBuf, Option<PathBuf>) {
    let negative = job
        .negative
        .as_deref()
        .filter(|negative| !negative.trim().is_empty())
        .map(|_| job.out_dir.join(NEGATIVE_FILE));
    (job.out_dir.join(PROMPT_FILE), negative)
}

/// Threads for a processor-only run: physical cores minus one, at least 1; physical cores
/// are the logical count halved on x86-64.
pub fn processor_threads(logical_cores: usize, x86_64: bool) -> u32 {
    let physical = if x86_64 {
        logical_cores / 2
    } else {
        logical_cores
    };
    u32::try_from(physical.saturating_sub(1).max(1)).unwrap_or(u32::MAX)
}

/// Whether `path` holds `%` followed by up to three digits and `d` (`%d`, `%3d`, `%03d`):
/// `sd-cli` reads an `-o` path holding one as a numbering template (the pinned CLI's
/// `format_specifier_regex`). Stricter than the engine, which skips a `%` escaped as `%%`.
pub fn is_numbering_template(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes
        .iter()
        .enumerate()
        .filter(|&(_, &b)| b == b'%')
        .any(|(i, _)| {
            let rest = &bytes[i + 1..];
            let digits = rest
                .iter()
                .take(3)
                .take_while(|d| d.is_ascii_digit())
                .count();
            rest.get(digits) == Some(&b'd')
        })
}

/// What Studio can send that the `sd-cli -h` text `help` does not list: each [`FLAGS`] entry
/// that is no option's name, and each [`Sampler`] and [`Scheduler`] word missing from its
/// option's `one of [...]` list. Empty for the pinned build.
pub fn missing_from_help(help: &str) -> Vec<&'static str> {
    let options = help_options(help);
    let listed = |flag: &str| options.iter().find(|option| option.names.contains(&flag));
    let choices = |flag: &str| listed(flag).map(HelpOption::choices).unwrap_or_default();
    let samplers = choices("--sampling-method");
    let schedulers = choices("--scheduler");
    let mut missing: Vec<&'static str> = FLAGS
        .iter()
        .copied()
        .filter(|flag| listed(flag).is_none())
        .collect();
    missing.extend(
        Sampler::ALL
            .iter()
            .map(|sampler| sampler.as_str())
            .filter(|word| !samplers.contains(word)),
    );
    missing.extend(
        Scheduler::ALL
            .iter()
            .map(|scheduler| scheduler.as_str())
            .filter(|word| !schedulers.contains(word)),
    );
    missing
}

/// One option of `sd-cli -h`: its names (`-o`, `--output`) and its description, its wrapped
/// lines joined.
struct HelpOption<'a> {
    names: Vec<&'a str>,
    description: String,
}

impl HelpOption<'_> {
    /// The words of the description's `one of [a, b, c]` list.
    fn choices(&self) -> Vec<&str> {
        self.description
            .split_once("one of [")
            .and_then(|(_, rest)| rest.split_once(']'))
            .map(|(list, _)| list.split(',').map(str::trim).collect())
            .unwrap_or_default()
    }
}

/// The options of `sd-cli -h`. The pinned CLI prints each as two spaces, its names
/// (`-o, --output <string>`), at least two spaces and its description, wrapped onto lines
/// indented further.
fn help_options(help: &str) -> Vec<HelpOption<'_>> {
    let mut options: Vec<HelpOption<'_>> = Vec::new();
    for line in help.lines() {
        if let Some(entry) = line.strip_prefix("  ").filter(|rest| rest.starts_with('-')) {
            let (names, description) = entry.split_once("  ").unwrap_or((entry, ""));
            options.push(HelpOption {
                names: names
                    .split(", ")
                    .filter_map(|name| name.split_whitespace().next())
                    .collect(),
                description: description.trim().to_owned(),
            });
        } else if line.starts_with(' ')
            && !line.trim().is_empty()
            && let Some(option) = options.last_mut()
        {
            option.description.push(' ');
            option.description.push_str(line.trim());
        }
    }
    options
}

fn fits(side: u32) -> bool {
    side.is_multiple_of(SIZE_MULTIPLE) && SIZE_RANGE.contains(&side)
}

fn unicode(path: &Path) -> Result<&str, ArgsError> {
    path.to_str()
        .ok_or_else(|| ArgsError::NotUnicode(path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::*;

    fn p(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|arg| arg.into_string().expect("Unicode"))
            .collect()
    }

    /// The argument after `flag`.
    fn after<'a>(args: &'a [String], flag: &str) -> &'a str {
        let at = args
            .iter()
            .position(|arg| arg == flag)
            .unwrap_or_else(|| panic!("{flag} is not in {args:?}"));
        &args[at + 1]
    }

    /// A FLUX.2 klein picture: four steps, no negative prompt, on the default device.
    fn job() -> SdJob {
        SdJob {
            parts: vec![
                (PartRole::Diffusion, p("/m/d.gguf")),
                (PartRole::Llm, p("/m/q.gguf")),
                (PartRole::Vae, p("/m/v.safetensors")),
            ],
            prompt: "a red kite over the sea".into(),
            negative: None,
            width: 1024,
            height: 1024,
            steps: 4,
            cfg: 1.0,
            flow_shift: None,
            sampler: Some(Sampler::Euler),
            scheduler: Some(Scheduler::Simple),
            seed: 81723,
            batch: 1,
            frames: None,
            fps: None,
            init_image: None,
            ref_images: Vec::new(),
            backend: None,
            memory_saving: false,
            processor_only: false,
            threads: None,
            out_dir: p("/g/.partial/r1"),
            out_stem: "r1".into(),
            format: OutputFormat::Png,
            env: Vec::new(),
        }
    }

    fn job_with(parts: Vec<(PartRole, PathBuf)>) -> SdJob {
        SdJob { parts, ..job() }
    }

    /// A Wan 2.2 TI2V video from a start frame: 49 frames at 24 fps with a flow shift.
    fn video() -> SdJob {
        SdJob {
            parts: vec![
                (PartRole::Diffusion, p("/m/wan.gguf")),
                (PartRole::T5xxl, p("/m/umt5.gguf")),
                (PartRole::Vae, p("/m/wan_vae.safetensors")),
            ],
            width: 832,
            height: 480,
            steps: 8,
            cfg: 5.0,
            flow_shift: Some(3.0),
            sampler: None,
            scheduler: None,
            frames: Some(49),
            fps: Some(24),
            init_image: Some(p("/g/refs/start.png")),
            format: OutputFormat::Webm,
            ..job()
        }
    }

    #[test]
    fn each_part_role_has_one_fixed_flag() {
        let a = strings(
            build_args(&job_with(vec![
                (PartRole::Diffusion, p("/m/d.gguf")),
                (PartRole::Llm, p("/m/q.gguf")),
                (PartRole::Vae, p("/m/v.safetensors")),
            ]))
            .unwrap(),
        );
        assert_eq!(
            &a[..6],
            [
                "--diffusion-model",
                "/m/d.gguf",
                "--llm",
                "/m/q.gguf",
                "--vae",
                "/m/v.safetensors"
            ]
        );
        let roles = [
            (PartRole::Diffusion, "--diffusion-model"),
            (PartRole::HighNoiseDiffusion, "--high-noise-diffusion-model"),
            (PartRole::Llm, "--llm"),
            (PartRole::T5xxl, "--t5xxl"),
            (PartRole::ClipL, "--clip_l"),
            (PartRole::ClipVision, "--clip_vision"),
            (PartRole::Vae, "--vae"),
            (PartRole::Single, "-m"),
        ];
        for (role, flag) in roles {
            assert_eq!(flag_for(role), flag, "{role:?}");
        }
    }

    #[test]
    fn the_prompt_goes_by_file_and_never_reaches_argv() {
        let job = SdJob {
            prompt: "-o /etc/x".into(),
            negative: Some("--vae evil".into()),
            ..job()
        };
        let a = strings(build_args(&job).unwrap());
        let (prompt, negative) = prompt_files(&job);
        assert_eq!(after(&a, "--prompt-file"), prompt.to_str().unwrap());
        assert_eq!(
            after(&a, "--negative-prompt-file"),
            negative.unwrap().to_str().unwrap()
        );
        assert_eq!(prompt, job.out_dir.join("prompt.txt"));
        assert!(
            !a.iter()
                .any(|x| x.contains("/etc/x") || x.contains("evil") || x.starts_with("--prompt="))
        );
        assert_eq!(a.iter().filter(|x| *x == "-o").count(), 1); // only the output's own
    }

    #[test]
    fn a_blank_negative_prompt_is_no_negative_prompt() {
        for negative in [None, Some(String::new()), Some(" \n".into())] {
            let job = SdJob { negative, ..job() };
            assert_eq!(prompt_files(&job).1, None);
            let a = strings(build_args(&job).unwrap());
            assert!(!a.contains(&"--negative-prompt-file".into()));
        }
        let job = SdJob {
            negative: Some("blurry".into()),
            ..job()
        };
        assert_eq!(prompt_files(&job).1, Some(job.out_dir.join("negative.txt")));
    }

    #[test]
    fn the_prompt_is_capped_at_4000_characters() {
        assert!(
            build_args(&SdJob {
                prompt: "a".repeat(4_000),
                ..job()
            })
            .is_ok()
        );
        assert_eq!(
            build_args(&SdJob {
                prompt: "a".repeat(4_001),
                ..job()
            }),
            Err(ArgsError::PromptTooLong)
        );
        // Characters, not bytes: 4,000 three-byte characters are allowed.
        assert!(
            build_args(&SdJob {
                prompt: "風".repeat(4_000),
                ..job()
            })
            .is_ok()
        );
    }

    #[test]
    fn an_empty_prompt_is_refused() {
        for prompt in ["", "  \n\t"] {
            assert_eq!(
                build_args(&SdJob {
                    prompt: prompt.into(),
                    ..job()
                }),
                Err(ArgsError::EmptyPrompt),
                "{prompt:?}"
            );
        }
    }

    #[test]
    fn an_output_folder_that_looks_like_a_numbering_template_is_refused() {
        for dir in ["/g/shots %d", "/g/%03d", "/g/a%12d"] {
            assert_eq!(
                build_args(&SdJob {
                    out_dir: dir.into(),
                    ..job()
                }),
                Err(ArgsError::PercentInOutputPath),
                "{dir}"
            );
        }
        assert!(
            build_args(&SdJob {
                out_dir: "/g/100% real".into(),
                ..job()
            })
            .is_ok()
        );
    }

    #[test]
    fn the_metadata_flag_is_always_there() {
        for job in [job(), video()] {
            assert!(
                strings(build_args(&job).unwrap()).contains(&"--disable-image-metadata".into())
            );
        }
    }

    #[test]
    fn memory_saving_adds_tiling_and_offload_and_processor_only_picks_cpu_and_threads() {
        let a = strings(
            build_args(&SdJob {
                memory_saving: true,
                ..job()
            })
            .unwrap(),
        );
        assert!(a.contains(&"--vae-tiling".into()) && a.contains(&"--offload-to-cpu".into()));
        let a = strings(
            build_args(&SdJob {
                processor_only: true,
                threads: Some(5),
                backend: Some("vulkan0".into()),
                ..job()
            })
            .unwrap(),
        );
        assert_eq!(
            (after(&a, "--backend"), after(&a, "--threads")),
            ("cpu", "5")
        );
        assert_eq!(
            (
                processor_threads(12, true),
                processor_threads(10, false),
                processor_threads(2, true)
            ),
            (5, 9, 1)
        );
        assert_eq!(
            (processor_threads(1, false), processor_threads(0, true)),
            (1, 1)
        );
    }

    #[test]
    fn the_probed_device_is_passed_and_no_device_means_the_engines_choice() {
        let a = strings(
            build_args(&SdJob {
                backend: Some("vulkan0".into()),
                ..job()
            })
            .unwrap(),
        );
        assert_eq!(after(&a, "--backend"), "vulkan0");
        let a = strings(build_args(&job()).unwrap());
        assert!(!a.contains(&"--backend".into()) && !a.contains(&"--threads".into()));
        assert!(!a.contains(&"--vae-tiling".into()) && !a.contains(&"--offload-to-cpu".into()));
    }

    #[test]
    fn no_scheduler_means_the_engines_own() {
        let a = strings(
            build_args(&SdJob {
                scheduler: None,
                sampler: None,
                ..job()
            })
            .unwrap(),
        );
        assert!(!a.contains(&"--scheduler".into()) && !a.contains(&"--sampling-method".into()));
        let a = strings(build_args(&job()).unwrap());
        assert_eq!(
            (after(&a, "--sampling-method"), after(&a, "--scheduler")),
            ("euler", "simple")
        );
    }

    #[test]
    fn a_video_job_asks_for_frames_fps_shift_and_webm() {
        let a = strings(build_args(&video()).unwrap());
        assert_eq!(after(&a, "-M"), "vid_gen");
        assert_eq!(after(&a, "--video-frames"), "49");
        assert_eq!(after(&a, "--fps"), "24");
        assert_eq!(after(&a, "--flow-shift"), "3");
        assert_eq!(after(&a, "-i"), "/g/refs/start.png");
        assert!(after(&a, "-o").ends_with(".webm"), "{a:?}");
        let picture = strings(build_args(&job()).unwrap());
        for flag in ["-M", "--video-frames", "--fps", "--flow-shift", "-i"] {
            assert!(!picture.contains(&flag.into()), "{flag}");
        }
        let webp = SdJob {
            format: OutputFormat::Webp,
            ..video()
        };
        assert!(after(&strings(build_args(&webp).unwrap()), "-o").ends_with(".webp"));
    }

    #[test]
    fn the_arguments_come_in_their_fixed_order() {
        let job = SdJob {
            negative: Some("blurry".into()),
            cfg: 3.5,
            batch: 2,
            ref_images: vec![p("/g/refs/a.png"), p("/g/refs/b.png")],
            backend: Some("vulkan0".into()),
            threads: Some(7),
            memory_saving: true,
            ..video()
        };
        let out = |name: &str| job.out_dir.join(name).to_str().unwrap().to_owned();
        assert_eq!(
            strings(build_args(&job).unwrap()),
            [
                "--diffusion-model",
                "/m/wan.gguf",
                "--t5xxl",
                "/m/umt5.gguf",
                "--vae",
                "/m/wan_vae.safetensors",
                "-M",
                "vid_gen",
                "--prompt-file",
                &out("prompt.txt"),
                "--negative-prompt-file",
                &out("negative.txt"),
                "-W",
                "832",
                "-H",
                "480",
                "--steps",
                "8",
                "--cfg-scale",
                "3.5",
                "--flow-shift",
                "3",
                "--seed",
                "81723",
                "--batch-count",
                "2",
                "--video-frames",
                "49",
                "--fps",
                "24",
                "-i",
                "/g/refs/start.png",
                "-r",
                "/g/refs/a.png",
                "-r",
                "/g/refs/b.png",
                "--backend",
                "vulkan0",
                "--threads",
                "7",
                "--vae-tiling",
                "--offload-to-cpu",
                "-o",
                &out("r1.webm"),
                "--disable-image-metadata",
                "--log-level",
                "info",
            ]
        );
    }

    #[test]
    fn a_job_needs_a_diffusion_model() {
        for parts in [
            vec![
                (PartRole::Llm, p("/m/q.gguf")),
                (PartRole::Vae, p("/m/v.gguf")),
            ],
            vec![(PartRole::HighNoiseDiffusion, p("/m/high.gguf"))],
            Vec::new(),
        ] {
            assert_eq!(
                build_args(&job_with(parts)),
                Err(ArgsError::NoDiffusionPart)
            );
        }
        let a = strings(
            build_args(&job_with(vec![(PartRole::Single, p("/m/all.safetensors"))])).unwrap(),
        );
        assert_eq!(&a[..2], ["-m", "/m/all.safetensors"]);
    }

    #[test]
    fn sizes_are_multiples_of_16_from_64_to_2048() {
        for (width, height) in [(64, 64), (2048, 2048), (832, 480)] {
            assert!(
                build_args(&SdJob {
                    width,
                    height,
                    ..job()
                })
                .is_ok(),
                "{width}x{height}"
            );
        }
        for (width, height) in [(1000, 1024), (1024, 1000), (48, 64), (64, 2064), (0, 0)] {
            assert_eq!(
                build_args(&SdJob {
                    width,
                    height,
                    ..job()
                }),
                Err(ArgsError::BadSize),
                "{width}x{height}"
            );
        }
    }

    /// A path that is not valid Unicode: never converted lossily, always refused.
    #[cfg(unix)]
    fn not_unicode() -> PathBuf {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(b"/m/\xff.gguf"))
    }

    /// A path that is not valid Unicode: never converted lossily, always refused.
    #[cfg(windows)]
    fn not_unicode() -> PathBuf {
        use std::os::windows::ffi::OsStringExt;
        PathBuf::from(OsString::from_wide(&[0x43, 0x3a, 0x5c, 0xd800]))
    }

    #[test]
    fn a_path_that_is_not_unicode_is_refused() {
        let bad = not_unicode();
        let jobs = [
            job_with(vec![(PartRole::Diffusion, bad.clone())]),
            SdJob {
                init_image: Some(bad.clone()),
                ..job()
            },
            SdJob {
                ref_images: vec![p("/g/refs/a.png"), bad.clone()],
                ..job()
            },
            SdJob {
                out_dir: bad.clone(),
                ..job()
            },
        ];
        for job in jobs {
            assert_eq!(build_args(&job), Err(ArgsError::NotUnicode(bad.clone())));
        }
    }

    #[test]
    fn the_whitelisted_words_are_the_pinned_cli_words() {
        let samplers: Vec<&str> = Sampler::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            samplers,
            [
                "euler",
                "euler_a",
                "heun",
                "dpm2",
                "dpm++2m",
                "dpm++2mv2",
                "lcm"
            ]
        );
        let schedulers: Vec<&str> = Scheduler::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(schedulers, ["discrete", "karras", "exponential", "simple"]);
        for sampler in Sampler::ALL {
            assert_eq!(Sampler::from_cli(sampler.as_str()), Some(*sampler));
        }
        for scheduler in Scheduler::ALL {
            assert_eq!(Scheduler::from_cli(scheduler.as_str()), Some(*scheduler));
        }
        for word in ["", "Euler", "ddim_trailing", "euler "] {
            assert_eq!(Sampler::from_cli(word), None, "{word:?}");
        }
        for word in ["", "Karras", "normal", "ays"] {
            assert_eq!(Scheduler::from_cli(word), None, "{word:?}");
        }
    }

    /// `FLAGS` is exactly what `build_args` can emit: a job with every field set gives each
    /// flag, and nothing outside the list.
    #[test]
    fn flags_lists_every_flag_build_args_emits() {
        let all_parts = vec![
            (PartRole::Diffusion, p("/m/d.gguf")),
            (PartRole::HighNoiseDiffusion, p("/m/h.gguf")),
            (PartRole::Llm, p("/m/q.gguf")),
            (PartRole::T5xxl, p("/m/t.gguf")),
            (PartRole::ClipL, p("/m/l.safetensors")),
            (PartRole::ClipVision, p("/m/cv.safetensors")),
            (PartRole::Vae, p("/m/v.safetensors")),
            (PartRole::Single, p("/m/all.safetensors")),
        ];
        let everything = SdJob {
            parts: all_parts,
            negative: Some("blurry".into()),
            sampler: Some(Sampler::DpmPp2m),
            scheduler: Some(Scheduler::Karras),
            ref_images: vec![p("/g/refs/a.png")],
            backend: Some("vulkan0".into()),
            threads: Some(3),
            memory_saving: true,
            ..video()
        };
        let emitted: Vec<String> = strings(build_args(&everything).unwrap())
            .into_iter()
            .filter(|arg| arg.starts_with('-'))
            .collect();
        for arg in &emitted {
            assert!(FLAGS.contains(&arg.as_str()), "{arg} is not in FLAGS");
        }
        for flag in FLAGS {
            assert!(
                emitted.iter().any(|arg| arg == flag),
                "{flag} is never emitted"
            );
        }
    }

    #[test]
    fn every_flag_is_in_the_help_text() {
        let help = include_str!("../../tests/fixtures/sd_help.txt");
        assert_eq!(missing_from_help(help), Vec::<&str>::new());
        for flag in FLAGS {
            assert!(help.contains(flag), "{flag}");
        }
    }

    #[test]
    fn the_help_check_reads_option_names_and_choices_whole() {
        // `--fps-max` does not list `--fps`, nor does a description that mentions it.
        let help = "Generation Options:\n  --fps-max <int>             most frames a second\n  \
                    --sampling-method           sampling method, one of [euler, heun,\n                              \
                    dpm2], default: euler (see --fps)\n";
        let missing = missing_from_help(help);
        assert!(
            missing.contains(&"--fps") && missing.contains(&"-o"),
            "{missing:?}"
        );
        assert!(!missing.contains(&"--sampling-method"), "{missing:?}");
        for word in ["euler_a", "dpm++2m", "lcm", "simple"] {
            assert!(missing.contains(&word), "{word}: {missing:?}");
        }
        for word in ["euler", "heun", "dpm2"] {
            assert!(!missing.contains(&word), "{word}: {missing:?}");
        }
    }
}
