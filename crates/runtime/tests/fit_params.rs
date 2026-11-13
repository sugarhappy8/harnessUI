//! Contract tests for `llama-fit-params` (item 6, section 4.3), the pinned build's memory
//! measurement, against the real program and a tiny model.
//!
//! They read `LLAMA_SERVER_BIN` (fit-params is its sibling in the same folder) and
//! `TEST_GGUF`, and skip with a note when either is unset. A test binary of its own, so
//! `getrusage(RUSAGE_CHILDREN)` sees only fit-params children, never a llama-server.
//!
//! Every run here is on the processor (`--device none`): the flags for graphics cards
//! (`--cpu-moe`, `--n-cpu-moe`, `-nkvo`) are checked for being accepted and measured; what
//! they do on a real card is on the owner checklist.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use runtime::llama::fit_params::{log_records, measure_env};
use runtime::{
    Backend, BackendKind, ExpertsOnCpu, FIT_PARAMS_TIMEOUT, FitParamsError, FlashAttn,
    MeasureConfig, MeasureLayers, MemoryRow, MemoryRowKind, RuntimeLayout, SpawnSpec, StdioMode,
    build_measure_args, discover, measurable, measure, parse_measure_output,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

fn env_paths(test: &str) -> Option<(PathBuf, PathBuf)> {
    let bin = std::env::var_os("LLAMA_SERVER_BIN").filter(|v| !v.is_empty());
    let gguf = std::env::var_os("TEST_GGUF").filter(|v| !v.is_empty());
    match (bin, gguf) {
        (Some(bin), Some(gguf)) => Some((PathBuf::from(bin), PathBuf::from(gguf))),
        _ => {
            eprintln!("skipping {test}: set LLAMA_SERVER_BIN and TEST_GGUF to run it");
            None
        }
    }
}

/// The processor backend of the folder holding `LLAMA_SERVER_BIN`, found the way the app
/// finds a development runtime, with its fit-params file.
fn backend(bin: &Path) -> Backend {
    let dir = bin
        .parent()
        .expect("LLAMA_SERVER_BIN has a parent directory")
        .to_path_buf();
    let layout = RuntimeLayout {
        installed_dir: None,
        packs_dir: dir.join("no-packs-here"),
        dev_override: Some(dir),
    };
    let backend = discover(&layout)
        .into_iter()
        .find(|backend| backend.kind == BackendKind::Cpu)
        .unwrap_or_else(|| panic!("no processor backend next to {}", bin.display()));
    assert!(
        backend.fit_params.is_some(),
        "llama-fit-params is missing next to {}",
        bin.display()
    );
    assert!(
        measurable(&backend),
        "a development folder is measurable in a debug build"
    );
    backend
}

/// What the picture measures on the processor: no device, every layer.
fn config(model: PathBuf, ctx: u32) -> MeasureConfig {
    MeasureConfig {
        model,
        ctx,
        device: None,
        layers: MeasureLayers::All,
        experts_on_cpu: ExpertsOnCpu::None,
        kv_in_computer_memory: false,
        save_memory: false,
        flash_attn: FlashAttn::Auto,
        hip_visible_devices: None,
    }
}

async fn measured(backend: &Backend, cfg: &MeasureConfig) -> Vec<MemoryRow> {
    match measure(backend, cfg, FIT_PARAMS_TIMEOUT, CancellationToken::new()).await {
        Ok(rows) => rows,
        Err(error) => panic!("measure failed for {cfg:?}: {error}"),
    }
}

fn context_bytes(rows: &[MemoryRow]) -> u64 {
    rows.iter().map(|r| r.context_bytes).sum()
}

/// Runs fit-params with exactly `args` and the app's environment for it, through the
/// crate's spawn helper: exit status, stdout and stderr.
async fn run_words(
    backend: &Backend,
    args: &[String],
    env: runtime::EnvSpec,
) -> (Option<i32>, bool, Vec<String>, Vec<String>) {
    let spec = SpawnSpec::new(
        backend.fit_params.as_ref().expect("fit-params"),
        &backend.dir,
    )
    .args(args.iter().cloned())
    .env(env)
    .stdin(StdioMode::Null)
    .stdout(StdioMode::Piped)
    .stderr(StdioMode::Piped);
    let mut child = runtime::spawn(spec).expect("spawn llama-fit-params");
    let stdout = child.take_stdout().expect("stdout");
    let stderr = child.take_stderr().expect("stderr");
    let read = |pipe: Box<dyn tokio::io::AsyncRead + Unpin + Send>| async move {
        let mut lines = BufReader::new(pipe).lines();
        let mut kept = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            kept.push(line);
        }
        kept
    };
    let (out, err, status) = tokio::time::timeout(FIT_PARAMS_TIMEOUT, async {
        tokio::join!(read(Box::new(stdout)), read(Box::new(stderr)), child.wait())
    })
    .await
    .expect("fit-params finishes in time");
    let status = status.expect("exit status");
    (status.code(), status.success(), out, err)
}

/// The measurement reads the header and tensor table only: rows parse into bytes, the
/// processor's figures include the repacked weights that the plain output line leaves
/// out, and (on Linux) the largest child's peak memory stays below half the model's size,
/// so the weights were never read. It answers well within the app's limit: the first run,
/// which on macOS compiles the Metal library (see `FIT_PARAMS_TIMEOUT`), within half of
/// it, and the run after it, as every later run in the app, within 20 s on every system.
/// Both give the same rows.
#[tokio::test(flavor = "multi_thread")]
async fn fit_params_measures_without_reading_the_weights() {
    let Some((bin, gguf)) = env_paths("fit_params_measures_without_reading_the_weights") else {
        return;
    };
    let backend = backend(&bin);
    let started = Instant::now();
    let rows = measured(&backend, &config(gguf.clone(), 4096)).await;
    let first = started.elapsed();
    assert!(
        first < FIT_PARAMS_TIMEOUT / 2,
        "the first run took {first:?}"
    );
    let started = Instant::now();
    let again = measured(&backend, &config(gguf.clone(), 4096)).await;
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(20),
        "the run after the first took {took:?}"
    );
    assert_eq!(again, rows, "the same settings measure the same");
    eprintln!("note: the first measurement took {first:?}, the next {took:?}: {rows:#?}");
    let host = rows
        .iter()
        .find(|r| r.kind == MemoryRowKind::Host)
        .expect("a Host row");
    assert_eq!(host.name, "Host");
    assert!(host.context_bytes > 0, "{rows:?}");
    assert!(host.compute_bytes > 0, "{rows:?}");
    assert!(
        rows.iter()
            .all(|r| r.kind != MemoryRowKind::Device && r.total_bytes.is_none()),
        "no device on the processor: {rows:?}"
    );
    let model_size = std::fs::metadata(&gguf).expect("model").len();
    let weights: u64 = rows.iter().map(|r| r.model_bytes).sum();
    assert!(
        weights >= model_size / 2 && weights <= model_size.saturating_mul(2),
        "the weights it counts ({weights}) belong to a {model_size}-byte file"
    );

    #[cfg(target_os = "linux")]
    {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
        // SAFETY: getrusage writes one struct we own; zeroed is a valid rusage.
        let ok = unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, usage.as_mut_ptr()) } == 0;
        assert!(ok, "getrusage");
        // SAFETY: getrusage succeeded and filled it.
        let usage = unsafe { usage.assume_init() };
        // Linux reports ru_maxrss in KiB: the largest child that was waited for.
        let peak = u64::try_from(usage.ru_maxrss).unwrap_or(u64::MAX) * 1024;
        eprintln!(
            "note: the largest fit-params child peaked at {peak} bytes for a {model_size}-byte model"
        );
        assert!(peak > 0, "a child was measured");
        assert!(
            peak < model_size / 2,
            "fit-params peaked at {peak} bytes for a {model_size}-byte model: it read the weights"
        );
    }
}

/// Save memory's q8_0 conversation memory measures at about half of f16 (34 bytes where
/// f16 takes 64, plus padding): 478 against 896 MiB at 8,192 tokens for the small model on
/// b12110.
#[tokio::test(flavor = "multi_thread")]
async fn fit_params_q8_0_halves_the_context() {
    let Some((bin, gguf)) = env_paths("fit_params_q8_0_halves_the_context") else {
        return;
    };
    let backend = backend(&bin);
    let plain = context_bytes(&measured(&backend, &config(gguf.clone(), 8192)).await);
    let saved = context_bytes(
        &measured(
            &backend,
            &MeasureConfig {
                save_memory: true,
                ..config(gguf.clone(), 8192)
            },
        )
        .await,
    );
    assert!(plain > 0);
    assert!(
        saved * 100 >= plain * 50 && saved * 100 <= plain * 57,
        "q8_0 should measure about half: {saved} against {plain} bytes"
    );
    // The context scales with the number of tokens.
    let half = context_bytes(&measured(&backend, &config(gguf, 4096)).await);
    assert!(
        half * 100 >= plain * 45 && half * 100 <= plain * 55,
        "{half} at 4,096 tokens against {plain} at 8,192"
    );
}

/// The Fine control words are accepted by the pinned build and give one record: the exact
/// words `--device none -ngl all --cpu-moe -nkvo` as the app builds them, and
/// `-ngl 10 --n-cpu-moe 5 -t 2 -fa off` (threads are no part of a measurement, so they are
/// added here by hand), each through the crate's spawn with the app's environment.
#[tokio::test(flavor = "multi_thread")]
async fn fit_params_accepts_the_fine_control_flags() {
    let Some((bin, gguf)) = env_paths("fit_params_accepts_the_fine_control_flags") else {
        return;
    };
    let backend = backend(&bin);
    let experts_all = MeasureConfig {
        experts_on_cpu: ExpertsOnCpu::All,
        kv_in_computer_memory: true,
        ..config(gguf.clone(), 4096)
    };
    let args = build_measure_args(&experts_all);
    assert!(
        args.join(" ")
            .contains("--device none -ngl all --cpu-moe -nkvo"),
        "{args:?}"
    );
    let first = MeasureConfig {
        layers: MeasureLayers::Count(10),
        experts_on_cpu: ExpertsOnCpu::FirstLayers(5),
        flash_attn: FlashAttn::Off,
        ..config(gguf.clone(), 4096)
    };
    let mut by_hand = build_measure_args(&first);
    let at = by_hand
        .iter()
        .position(|a| a == "-fa")
        .expect("-fa in the words");
    by_hand.splice(at..at, ["-t".to_owned(), "2".to_owned()]);
    assert!(
        by_hand
            .join(" ")
            .contains("-ngl 10 --n-cpu-moe 5 -t 2 -fa off"),
        "{by_hand:?}"
    );
    for (cfg, words) in [(&experts_all, args), (&first, by_hand)] {
        let (code, success, out, err) =
            run_words(&backend, &words, measure_env(&backend, cfg)).await;
        assert!(success, "exit {code:?} for {words:?}\n{}", err.join("\n"));
        let records = out
            .iter()
            .filter(|l| l.contains("\"type\":\"fit_memory_breakdown\""))
            .count();
        if records == 0 {
            // llama.cpp lost its queued record at exit (about 1 run in 80 under load);
            // `measure` runs again for this, a bare run does not. The plain line shows the
            // words were accepted and measured.
            assert!(
                out.iter().any(|l| l.starts_with("Host ")),
                "neither a record nor the plain line for {words:?}: {out:?}"
            );
            eprintln!("note: the record was lost at exit this time for {words:?}");
            continue;
        }
        assert_eq!(records, 1, "{out:?}");
        let rows = parse_measure_output(&out).unwrap_or_else(|e| {
            panic!("{e} for {words:?}\n{}", out.join("\n"));
        });
        assert!(!rows.is_empty());
    }
    // And through `measure` itself.
    measured(&backend, &first).await;
    measured(&backend, &experts_all).await;
    // A device the build does not have is refused at once.
    let vulkan = MeasureConfig {
        device: Some("Vulkan0".into()),
        ..config(gguf, 4096)
    };
    if cfg!(target_os = "linux") {
        let started = Instant::now();
        let result = measure(
            &backend,
            &vulkan,
            FIT_PARAMS_TIMEOUT,
            CancellationToken::new(),
        )
        .await;
        assert!(
            matches!(result, Err(FitParamsError::Failed { .. })),
            "{result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}

/// A broken model file makes fit-params abort, and the run reports `Failed` within the
/// timeout. With the app's environment (`GGML_NO_BACKTRACE=1`) no debugger runs: neither
/// stream has a line from `gdb` (its `[New LWP …]` thread lines go to stdout). The reason
/// is in the log records on stdout when llama.cpp wrote it before the abort. Where a
/// debugger is installed, the same run without the variable shows the difference.
#[tokio::test(flavor = "multi_thread")]
async fn fit_params_on_a_broken_file_fails_without_a_debugger() {
    let Some((bin, gguf)) = env_paths("fit_params_on_a_broken_file_fails_without_a_debugger")
    else {
        return;
    };
    let backend = backend(&bin);
    let dir = tempfile::tempdir().expect("tempdir");
    let broken = dir.path().join("truncated.gguf");
    // The header and tensor table, then the data cut short: "data is not within the file
    // bounds". Only the first 20 MB are read: the tests of this binary run side by side,
    // and on Linux a child's peak memory starts from this process's size when it is forked,
    // so holding the whole model here would show up in the peak that
    // fit_params_measures_without_reading_the_weights checks.
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(std::fs::File::open(&gguf).expect("model"), 20_000_000),
        &mut bytes,
    )
    .expect("read the start of the model");
    std::fs::write(&broken, &bytes).expect("write");
    drop(bytes);
    let cfg = config(broken, 4096);

    let started = Instant::now();
    let result = measure(&backend, &cfg, FIT_PARAMS_TIMEOUT, CancellationToken::new()).await;
    assert!(
        matches!(result, Err(FitParamsError::Failed { .. })),
        "{result:?}"
    );
    assert!(started.elapsed() < FIT_PARAMS_TIMEOUT);

    let words = build_measure_args(&cfg);
    let (code, success, out, err) = run_words(&backend, &words, measure_env(&backend, &cfg)).await;
    assert!(!success, "exit {code:?}");
    let debugger = |lines: &[String]| {
        lines
            .iter()
            .filter(|l| {
                let lower = l.to_ascii_lowercase();
                lower.contains("gdb") || l.contains("LWP") || lower.contains("lldb")
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    assert!(debugger(&out).is_empty(), "stdout: {out:#?}");
    assert!(debugger(&err).is_empty(), "stderr: {err:#?}");
    // llama.cpp writes its log from a worker thread, and the abort after the exception can
    // come before that thread has written the error records (6 runs in 40 on the build
    // machine): then stdout holds only the first info record. When any error record made
    // it out, the reason is among them.
    let (records, _) = log_records(&out, &err);
    if records.is_empty() {
        eprintln!(
            "note: the abort came before llama.cpp's log thread wrote the reason this time; \
             the run still reports Failed"
        );
    } else {
        assert!(
            records
                .iter()
                .any(|r| r.contains("not within the file bounds")),
            "the reason is in the log records: {records:?}"
        );
    }

    // The control: the scrubbed environment without the variable (Linux, debugger found).
    #[cfg(target_os = "linux")]
    {
        let found = std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("gdb").is_file()));
        if found {
            let (_, _, out, err) = run_words(&backend, &words, runtime::EnvSpec::minimal()).await;
            let seen = [debugger(&out), debugger(&err)].concat();
            if seen.is_empty() {
                eprintln!(
                    "note: gdb is installed but did not attach without GGML_NO_BACKTRACE \
                     (ptrace may be restricted here), so the control shows nothing"
                );
            } else {
                eprintln!(
                    "note: without GGML_NO_BACKTRACE a debugger ran ({} lines), with it none",
                    seen.len()
                );
            }
        } else {
            eprintln!("note: no gdb on PATH, so the control run is skipped");
        }
    }
}
