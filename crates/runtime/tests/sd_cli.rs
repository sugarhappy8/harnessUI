//! The Studio engine's runner end to end, with a fake engine
//! (docs/dev/0.1.0/studio-local-design.md, "Progress, Stop, failures").
//!
//! This test target has `harness = false`, so this file's `main` runs for it. Started with
//! [`FAKE_MODE_ENV`] set it *is* the engine: [`fake_main`] reads the arguments as `sd-cli`
//! does, prints the pinned build's output, writes tiny files and exits as the mode says.
//! Started any other way (by `cargo test`) it runs the checks, each with this same binary as
//! the engine. A word given as the first argument runs only the checks whose names hold it.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use runtime::sd::args::{OutputFormat, PartRole, SdJob, build_args};
use runtime::sd::devices::parse_list_devices;
use runtime::sd::engine::{Engine, EngineSource};
use runtime::sd::failure::SdFailure;
use runtime::sd::progress::LocalStage;
use runtime::sd::run::LIST_DEVICES_TIMEOUT;
use runtime::sd::testing::{
    FAKE_MODE_ENV, FAKE_RECORD_ENV, SLOW_PAUSE, fake_main, tiny_png, tiny_webm,
};
use runtime::sd::{RunEvent, RunLimits, RunOutput, list_devices, run_job};
use tokio_util::sync::CancellationToken;

/// Each check, by name.
macro_rules! checks {
    ($($check:ident),* $(,)?) => {
        [$((stringify!($check), $check as fn())),*]
    };
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if std::env::var_os(FAKE_MODE_ENV).is_some() {
        fake_main(&args[1..]);
    }
    if args.iter().any(|arg| arg == "--list") {
        println!("sd_cli: test");
        return;
    }
    let filter = args.get(1).filter(|arg| !arg.starts_with('-'));
    let checks = checks![
        a_picture_run_reports_its_stages_and_writes_its_file,
        a_batch_comes_back_in_order,
        the_engine_gets_exactly_the_flags_studio_built_and_the_prompt_by_file,
        the_environment_is_scrubbed,
        stop_kills_the_engine_and_its_child_within_a_second_and_removes_the_partial_folder,
        out_of_memory_retries_once_with_memory_saving,
        a_second_out_of_memory_is_named,
        a_failed_run_removes_its_partial_folder,
        a_crash_a_missing_library_and_a_bad_model_are_named,
        no_progress_for_the_stall_limit_says_so_and_keeps_running,
        an_engine_that_cannot_start_is_named,
        the_devices_are_listed,
        an_exit_0_without_a_file_is_a_failure,
        a_stop_while_retrying_starts_nothing_more,
        a_device_list_that_fails_or_never_comes_is_named,
    ];
    for (name, check) in checks {
        if filter.is_some_and(|word| !name.contains(word.as_str())) {
            continue;
        }
        check();
        println!("sd_cli: {name} ... ok");
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
}

/// A gallery folder and the fake engine's record folder.
struct Folders {
    _tmp: tempfile::TempDir,
    gallery: PathBuf,
    rec: PathBuf,
}

fn folders() -> Folders {
    let tmp = tempfile::tempdir().expect("a temporary folder");
    let gallery = tmp.path().join("gallery");
    let rec = tmp.path().join("record");
    std::fs::create_dir_all(&gallery).expect("the gallery folder");
    std::fs::create_dir_all(&rec).expect("the record folder");
    Folders {
        _tmp: tmp,
        gallery,
        rec,
    }
}

/// This binary as the engine, in `mode`, recording each start into `rec`.
fn fake(mode: &str, rec: &Path) -> Engine {
    let program = std::env::current_exe().expect("this test binary");
    let dir = program.parent().expect("its folder").to_path_buf();
    Engine {
        program,
        dir,
        build: runtime::sd::lock::embedded().sd_cpp.build,
        source: EngineSource::Dev,
        leading_args: Vec::new(),
        extra_env: vec![
            (FAKE_MODE_ENV.into(), mode.into()),
            (
                FAKE_RECORD_ENV.into(),
                rec.to_str().expect("a Unicode path").into(),
            ),
        ],
        vulkan_fallback: None,
    }
}

/// A FLUX.2 klein picture into `<gallery>/.partial/r1`. The prompt holds a line break and a
/// character outside ASCII; the fake never reads the model files.
fn picture_job(gallery: &Path) -> SdJob {
    let models = gallery.join("models");
    SdJob {
        parts: vec![
            (PartRole::Diffusion, models.join("d.gguf")),
            (PartRole::Llm, models.join("q.gguf")),
            (PartRole::Vae, models.join("v.safetensors")),
        ],
        prompt: "a red kite over the sea at dusk,\nin watercolour 風".into(),
        negative: None,
        width: 1024,
        height: 1024,
        steps: 4,
        cfg: 1.0,
        flow_shift: None,
        sampler: None,
        scheduler: None,
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
        out_dir: gallery.join(".partial").join("r1"),
        out_stem: "r1".into(),
        format: OutputFormat::Png,
        env: Vec::new(),
    }
}

/// A Wan 2.2 clip: 49 frames at 24 fps, as WebM.
fn video_job(gallery: &Path) -> SdJob {
    SdJob {
        width: 832,
        height: 480,
        frames: Some(49),
        fps: Some(24),
        format: OutputFormat::Webm,
        ..picture_job(gallery)
    }
}

/// Runs `job` on `engine` to its end: the result and every event, in order.
fn run(
    engine: &Engine,
    job: &SdJob,
    limits: RunLimits,
) -> (Result<RunOutput, SdFailure>, Vec<RunEvent>) {
    let events = Mutex::new(Vec::new());
    let on_event = |event: RunEvent| events.lock().expect("events").push(event);
    let result = rt().block_on(run_job(
        engine,
        job,
        limits,
        CancellationToken::new(),
        &on_event,
    ));
    (result, events.into_inner().expect("events"))
}

/// The stages the progress events walked through, each once.
fn stages(events: &[RunEvent]) -> Vec<LocalStage> {
    let mut stages: Vec<LocalStage> = events
        .iter()
        .filter_map(|event| match event {
            RunEvent::Progress(progress) => Some(progress.stage),
            _ => None,
        })
        .collect();
    stages.dedup();
    stages
}

fn retries(events: &[RunEvent]) -> Vec<SdFailure> {
    events
        .iter()
        .filter_map(|event| match event {
            RunEvent::Retrying(failure) => Some(failure.clone()),
            _ => None,
        })
        .collect()
}

fn stalls(events: &[RunEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, RunEvent::Stalled))
        .count()
}

fn strings(args: Vec<OsString>) -> Vec<String> {
    args.into_iter()
        .map(|arg| arg.into_string().expect("Unicode"))
        .collect()
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The arguments the fake got at start `n` (from 0).
fn recorded_argv(rec: &Path, n: usize) -> Vec<String> {
    read_json(&rec.join(format!("argv-{n}.json")))
}

/// The environment the fake had at start `n`.
fn recorded_env(rec: &Path, n: usize) -> BTreeMap<String, String> {
    read_json(&rec.join(format!("env-{n}.json")))
}

/// What the fake read from `--prompt-file` at start `n`, byte for byte.
fn recorded_prompt(rec: &Path, n: usize) -> String {
    let path = rec.join(format!("prompt-{n}.txt"));
    String::from_utf8(std::fs::read(&path).expect("the prompt's copy")).expect("UTF-8")
}

/// The names in the output folder when start `n` began.
fn recorded_listing(rec: &Path, n: usize) -> Vec<String> {
    read_json(&rec.join(format!("files-{n}.json")))
}

/// How many times the engine was started for a job.
fn starts(rec: &Path) -> usize {
    (0..)
        .take_while(|n| rec.join(format!("argv-{n}.json")).is_file())
        .count()
}

/// The process id a fake engine writes to `path`, once it is there (within 5 s, real time).
fn wait_for_pid(path: &Path) -> Option<u32> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(pid) = std::fs::read_to_string(path) {
            return pid.trim().parse().ok();
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn a_picture_run_reports_its_stages_and_writes_its_file() {
    let f = folders();
    let job = picture_job(&f.gallery);
    let (result, events) = run(&fake("ok", &f.rec), &job, RunLimits::default());
    let out = result.expect("the run succeeds");
    // sd-cli's own naming: <stem>.png for one picture
    assert_eq!(
        out.files,
        [f.gallery.join(".partial").join("r1").join("r1.png")]
    );
    assert_eq!(
        std::fs::read(&out.files[0]).expect("the picture"),
        tiny_png()
    );
    assert!(!out.retried);
    assert_eq!(
        stages(&events),
        [
            LocalStage::Loading,
            LocalStage::ReadingPrompt,
            LocalStage::Drawing,
            LocalStage::Decoding,
            LocalStage::Saving
        ]
    );
    let last = events
        .iter()
        .rev()
        .find_map(|event| match event {
            RunEvent::Progress(progress) => Some(progress.clone()),
            _ => None,
        })
        .expect("progress");
    assert_eq!((last.step, last.steps, last.pictures), (4, 4, 1));
    assert!(last.elapsed_ms <= out.elapsed_ms);
    assert_eq!((stalls(&events), retries(&events)), (0, vec![]));
}

fn a_batch_comes_back_in_order() {
    let f = folders();
    let job = SdJob {
        batch: 2,
        ..picture_job(&f.gallery)
    };
    let out = run(&fake("ok", &f.rec), &job, RunLimits::default())
        .0
        .expect("the batch succeeds");
    assert_eq!(
        out.files,
        [job.out_dir.join("r1_0.png"), job.out_dir.join("r1_1.png")]
    );

    let f = folders();
    let job = video_job(&f.gallery);
    let (result, events) = run(&fake("ok", &f.rec), &job, RunLimits::default());
    let out = result.expect("the clip succeeds");
    assert_eq!(out.files, [job.out_dir.join("r1.webm")]);
    assert_eq!(std::fs::read(&out.files[0]).expect("the clip"), tiny_webm());
    assert_eq!(stages(&events).last(), Some(&LocalStage::Saving));
}

fn the_engine_gets_exactly_the_flags_studio_built_and_the_prompt_by_file() {
    let f = folders();
    let job = SdJob {
        negative: Some("blurry".into()),
        ..picture_job(&f.gallery)
    };
    run(&fake("ok", &f.rec), &job, RunLimits::default())
        .0
        .expect("the run succeeds");
    assert_eq!(
        recorded_argv(&f.rec, 0),
        strings(build_args(&job).expect("the arguments"))
    );
    // the fake read --prompt-file's bytes; no trailing newline
    assert_eq!(recorded_prompt(&f.rec, 0), job.prompt);
    // the prompt files were there when the engine started, and stay until the caller is done
    assert_eq!(recorded_listing(&f.rec, 0), ["negative.txt", "prompt.txt"]);
    assert_eq!(
        std::fs::read_to_string(job.out_dir.join("negative.txt")).expect("the negative prompt"),
        "blurry"
    );
    assert_eq!(starts(&f.rec), 1);

    // a blank negative prompt leaves no file (the files are prompt_files', as build_args'),
    // and the engine is given none
    let f = folders();
    let job = SdJob {
        negative: Some(" \n".into()),
        ..picture_job(&f.gallery)
    };
    run(&fake("ok", &f.rec), &job, RunLimits::default())
        .0
        .expect("the run succeeds");
    assert_eq!(
        (
            recorded_listing(&f.rec, 0),
            recorded_argv(&f.rec, 0).contains(&"--negative-prompt-file".to_owned())
        ),
        (vec!["prompt.txt".to_owned()], false)
    );
}

fn the_environment_is_scrubbed() {
    let f = folders();
    let job = SdJob {
        env: vec![("GGML_METAL_TENSOR_DISABLE".into(), "1".into())],
        ..picture_job(&f.gallery)
    };
    run(&fake("ok", &f.rec), &job, RunLimits::default())
        .0
        .expect("the run succeeds");
    let env = recorded_env(&f.rec, 0);

    let platform = if cfg!(windows) {
        runtime::spawn::WINDOWS_ENV_ALLOWLIST
    } else {
        runtime::spawn::UNIX_ENV_ALLOWLIST
    };
    let mut allowed: Vec<String> = platform.iter().map(|name| (*name).to_owned()).collect();
    allowed.extend(
        [
            runtime::llama::launch::NO_BACKTRACE_ENV,
            FAKE_MODE_ENV,
            FAKE_RECORD_ENV,
        ]
        .map(str::to_owned),
    );
    allowed.extend(job.env.iter().map(|(name, _)| name.clone()));
    let is_allowed = |name: &str| {
        allowed.iter().any(|a| {
            if cfg!(windows) {
                a.eq_ignore_ascii_case(name)
            } else {
                a == name
            }
        })
    };
    for name in env.keys() {
        // Windows keeps a drive's folder in a variable named "=C:"; CoreFoundation sets its
        // text encoding in every macOS process that links it. Neither comes from the runner.
        if (cfg!(windows) && name.starts_with('='))
            || (cfg!(target_os = "macos") && name == "__CF_USER_TEXT_ENCODING")
        {
            continue;
        }
        assert!(is_allowed(name), "{name} reached the engine: {env:?}");
    }
    assert_eq!(
        env.get(runtime::llama::launch::NO_BACKTRACE_ENV)
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        env.get("GGML_METAL_TENSOR_DISABLE").map(String::as_str),
        Some("1")
    );
    // cargo gives this process CARGO_* variables; none of them reached the engine
    let leaked: Vec<String> = std::env::vars()
        .map(|(name, _)| name)
        .filter(|name| name.starts_with("CARGO_"))
        .collect();
    assert!(!leaked.is_empty());
    assert!(leaked.iter().all(|name| !env.contains_key(name)));
}

fn stop_kills_the_engine_and_its_child_within_a_second_and_removes_the_partial_folder() {
    let f = folders();
    let job = picture_job(&f.gallery);
    let cancel = CancellationToken::new();
    let cancelled_at = Mutex::new(None::<Instant>);
    let on_event = |event: RunEvent| {
        let mut at = cancelled_at.lock().expect("the time");
        if matches!(event, RunEvent::Progress(_)) && at.is_none() {
            *at = Some(Instant::now());
            cancel.cancel();
        }
    };
    // mode hang: the fake starts one grandchild through runtime::spawn, and both sleep
    let result = rt().block_on(run_job(
        &fake("hang", &f.rec),
        &job,
        RunLimits::default(),
        cancel.clone(),
        &on_event,
    ));
    let started = cancelled_at
        .into_inner()
        .expect("the time")
        .expect("a progress line came");
    assert!(matches!(result, Err(SdFailure::Cancelled)), "{result:?}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert!(!job.out_dir.exists());
    let grandchild: u32 = std::fs::read_to_string(f.rec.join("grandchild-0.txt"))
        .expect("the grandchild's pid")
        .trim()
        .parse()
        .expect("a pid");
    assert_ne!(grandchild, 0);
    #[cfg(unix)]
    {
        // Killed with the engine's group; on macOS a zombie counts as alive until launchd
        // reaps it, so look again for a moment.
        let deadline = started + Duration::from_secs(1);
        while runtime::process_alive(grandchild) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!runtime::process_alive(grandchild));
    }
}

fn out_of_memory_retries_once_with_memory_saving() {
    let f = folders();
    let job = picture_job(&f.gallery);
    let (result, events) = run(&fake("oom-once", &f.rec), &job, RunLimits::default());
    let out = result.expect("the second start succeeds");
    assert!(out.retried);
    assert_eq!(out.files, [job.out_dir.join("r1.png")]);
    assert_eq!(
        std::fs::read(&out.files[0]).expect("the picture"),
        tiny_png()
    );
    let (first, second) = (recorded_argv(&f.rec, 0), recorded_argv(&f.rec, 1));
    for flag in ["--vae-tiling", "--offload-to-cpu"] {
        assert!(!first.iter().any(|arg| arg == flag), "{flag}");
        assert!(second.iter().any(|arg| arg == flag), "{flag}");
    }
    assert_eq!(
        second,
        strings(
            build_args(&SdJob {
                memory_saving: true,
                ..job.clone()
            })
            .expect("the arguments")
        )
    );
    // the first start's half file was removed before the second; the prompt file stayed
    assert_eq!(recorded_listing(&f.rec, 1), ["prompt.txt"]);
    assert_eq!(recorded_prompt(&f.rec, 1), job.prompt);
    assert_eq!(retries(&events), [SdFailure::OutOfMemory]);
    assert_eq!(starts(&f.rec), 2);
}

fn a_second_out_of_memory_is_named() {
    let f = folders();
    let job = picture_job(&f.gallery);
    let (result, events) = run(&fake("oom", &f.rec), &job, RunLimits::default());
    assert_eq!(result, Err(SdFailure::OutOfMemory));
    assert_eq!(retries(&events), [SdFailure::OutOfMemory]);
    assert_eq!(starts(&f.rec), 2);

    // a job that already saves memory is not tried again
    let f = folders();
    let job = SdJob {
        memory_saving: true,
        ..picture_job(&f.gallery)
    };
    let (result, events) = run(&fake("oom", &f.rec), &job, RunLimits::default());
    assert_eq!(result, Err(SdFailure::OutOfMemory));
    assert_eq!((retries(&events), starts(&f.rec)), (vec![], 1));
}

fn a_failed_run_removes_its_partial_folder() {
    // each mode fails after the fake wrote a half file
    for mode in ["oom", "crash", "bad-model"] {
        let f = folders();
        let job = picture_job(&f.gallery);
        let (result, _) = run(&fake(mode, &f.rec), &job, RunLimits::default());
        assert!(result.is_err(), "{mode}: {result:?}");
        assert!(!job.out_dir.exists(), "{mode}");
        assert!(f.gallery.join(".partial").is_dir(), "{mode}");
    }
}

fn a_crash_a_missing_library_and_a_bad_model_are_named() {
    for mode in ["crash", "missing-dll", "bad-model"] {
        let f = folders();
        let (result, events) = run(
            &fake(mode, &f.rec),
            &picture_job(&f.gallery),
            RunLimits::default(),
        );
        let failure = result.expect_err(mode);
        match mode {
            "crash" => assert!(matches!(failure, SdFailure::Crashed { .. }), "{failure:?}"),
            "missing-dll" => assert!(
                matches!(failure, SdFailure::MissingLibrary(_)),
                "{failure:?}"
            ),
            _ => assert_eq!(failure, SdFailure::BadModel),
        }
        // only running out of memory is tried again
        assert_eq!((retries(&events), starts(&f.rec)), (vec![], 1), "{mode}");
    }
}

fn no_progress_for_the_stall_limit_says_so_and_keeps_running() {
    // mode slow: one progress line, then SLOW_PAUSE (1.2 s) of silence, then the rest. A
    // third of it as the limit (400 ms) leaves room for the fake's start-up and its other
    // gaps on a busy machine. (400 ms of silence and a 200 ms limit were first planned, but
    // the stall timer starts with the engine, so a start-up slower than 200 ms sends a
    // Stalled before the first line and this check would see two.) What it checks is the
    // same: one Stalled after the first progress, more progress after it, then Ok.
    let f = folders();
    let limits = RunLimits {
        stall_after: SLOW_PAUSE / 3,
    };
    let (result, events) = run(&fake("slow", &f.rec), &picture_job(&f.gallery), limits);
    let out = result.expect("a stalled run keeps going");
    assert_eq!(out.files.len(), 1);
    assert_eq!(stalls(&events), 1, "{events:?}");
    let stalled = events
        .iter()
        .position(|event| matches!(event, RunEvent::Stalled))
        .expect("the stall");
    assert!(matches!(events[0], RunEvent::Progress(_)));
    assert!(
        events[stalled + 1..]
            .iter()
            .any(|event| matches!(event, RunEvent::Progress(_)))
    );
    assert_eq!(stages(&events).last(), Some(&LocalStage::Saving));
}

fn an_engine_that_cannot_start_is_named() {
    let f = folders();
    let job = picture_job(&f.gallery);
    let missing = Engine {
        program: f.gallery.join("no-such-engine"),
        dir: f.gallery.clone(),
        ..fake("ok", &f.rec)
    };
    let (result, _) = run(&missing, &job, RunLimits::default());
    assert_eq!(result, Err(SdFailure::EngineMissing));
    assert!(!job.out_dir.exists());
    assert_eq!(
        rt().block_on(list_devices(&missing)),
        Err(SdFailure::EngineMissing)
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let program = f.gallery.join("sd-cli");
        std::fs::write(&program, "#!/bin/sh\nexit 0\n").expect("the program");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o644))
            .expect("no execute bit");
        let blocked = Engine {
            program,
            ..missing.clone()
        };
        let (result, _) = run(&blocked, &job, RunLimits::default());
        assert!(
            matches!(result, Err(SdFailure::EngineBlocked(_))),
            "{result:?}"
        );
        assert!(!job.out_dir.exists());
    }
}

fn the_devices_are_listed() {
    let expected = parse_list_devices(include_str!("fixtures/sd_list_devices_vulkan.txt"));
    assert!(!expected.is_empty());
    for mode in ["ok", "hang", "missing-dll"] {
        let f = folders();
        let devices = rt()
            .block_on(list_devices(&fake(mode, &f.rec)))
            .unwrap_or_else(|e| panic!("{mode}: {e:?}"));
        assert_eq!(devices, expected, "{mode}");
        // listing the devices is not a start of a job
        assert_eq!(starts(&f.rec), 0, "{mode}");
    }
}

fn an_exit_0_without_a_file_is_a_failure() {
    // mode no-output: the whole progress up to the saving lines, then exit 0 and no file
    let f = folders();
    let job = picture_job(&f.gallery);
    let (result, events) = run(&fake("no-output", &f.rec), &job, RunLimits::default());
    // nothing in the output names a cause, so it is a crash with the code the engine gave
    assert_eq!(result, Err(SdFailure::Crashed { code: Some(0) }));
    assert!(!job.out_dir.exists());
    assert!(stages(&events).contains(&LocalStage::Decoding));
    assert_eq!((retries(&events), starts(&f.rec)), (vec![], 1));
}

fn a_stop_while_retrying_starts_nothing_more() {
    // mode oom-once: the second start would succeed, so only the Stop can end the run
    let f = folders();
    let job = picture_job(&f.gallery);
    let cancel = CancellationToken::new();
    let events = Mutex::new(Vec::new());
    let on_event = |event: RunEvent| {
        if matches!(event, RunEvent::Retrying(_)) {
            cancel.cancel();
        }
        events.lock().expect("events").push(event);
    };
    let result = rt().block_on(run_job(
        &fake("oom-once", &f.rec),
        &job,
        RunLimits::default(),
        cancel.clone(),
        &on_event,
    ));
    assert_eq!(result, Err(SdFailure::Cancelled));
    let events = events.into_inner().expect("events");
    assert_eq!(retries(&events), [SdFailure::OutOfMemory]);
    assert_eq!(starts(&f.rec), 1);
    assert!(!job.out_dir.exists());
    assert!(f.gallery.join(".partial").is_dir());
}

fn a_device_list_that_fails_or_never_comes_is_named() {
    // mode devices-fail: a non-zero exit is named from the engine's output
    let f = folders();
    let result = rt().block_on(list_devices(&fake("devices-fail", &f.rec)));
    assert!(
        matches!(result, Err(SdFailure::MissingLibrary(_))),
        "{result:?}"
    );
    assert_eq!(starts(&f.rec), 0);

    // mode devices-hang: the engine never answers, and the call gives up after
    // LIST_DEVICES_TIMEOUT. The check runs on a paused clock and moves it past the limit
    // itself. Tokio moves a paused clock on by itself only while no blocking task runs, and
    // on Windows it reads a child's pipes on its blocking pool, so there the clock would
    // stand still until the fake ended its sleep. Waiting for the fake's process id on a
    // blocking task holds the clock in the same way everywhere, so the limit cannot pass
    // before the fake has started and recorded it; by then the call has set its limit, which
    // it does in the same step as starting the engine.
    let f = folders();
    let started = Instant::now();
    let paused = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .expect("a runtime");
    let engine = fake("devices-hang", &f.rec);
    let pid_file = f.rec.join("devices-hang.txt");
    let (result, waited, pid) = paused.block_on(async {
        let clock = tokio::time::Instant::now();
        let (result, pid) = tokio::join!(list_devices(&engine), async {
            let pid = tokio::task::spawn_blocking(move || wait_for_pid(&pid_file))
                .await
                .expect("the wait for the fake's process id");
            tokio::time::advance(LIST_DEVICES_TIMEOUT + Duration::from_millis(1)).await;
            pid
        });
        (result, clock.elapsed(), pid)
    });
    // the limit passing is named as a crash without an exit code
    assert_eq!(result, Err(SdFailure::Crashed { code: None }));
    assert!(waited >= LIST_DEVICES_TIMEOUT, "{waited:?}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(starts(&f.rec), 0);
    // the fake would sleep for minutes; the call came back with it stopped
    let pid = pid.expect("the fake's process id");
    assert!(!runtime::process_alive(pid), "{pid} still runs");

    // in both modes a job still runs as in mode ok
    for mode in ["devices-fail", "devices-hang"] {
        let f = folders();
        let job = picture_job(&f.gallery);
        let out = run(&fake(mode, &f.rec), &job, RunLimits::default())
            .0
            .unwrap_or_else(|e| panic!("{mode}: {e:?}"));
        assert_eq!(out.files, [job.out_dir.join("r1.png")], "{mode}");
    }
}
