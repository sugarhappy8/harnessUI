//! "Where the model goes" and "where the model went" on fixed facts and rows (0.0.6 item
//! 6, WP10d), and the measurement's one-at-a-time rules with a fake fit-params.

use std::sync::atomic::{AtomicUsize, Ordering};

use models::layers::{LayerSize, LayerSizes};
use models::placement::Fits;
use runtime::{BackendKind, FlashAttn, MemoryRow, MemoryRowKind};

use super::*;
use crate::dto::LocalModelSettings;
use crate::load_settings::{
    ContextSize, ContextSource, HeaderFacts, LayerFacts, Machine, ModelFacts, ModelLoadSettings,
    Plan, ResolvedLoad, ThinkingLimit,
};

const GB: u64 = 1_000_000_000;
const GIB: u64 = 1024 * 1024 * 1024;

fn model(blocks: u32, dense: u64, expert: u64, edge: u64) -> ModelFacts {
    let sizes = LayerSizes {
        input_bytes: edge,
        output_bytes: edge,
        layers: vec![
            LayerSize {
                dense_bytes: dense,
                expert_bytes: expert,
            };
            blocks as usize
        ],
        other_bytes: 0,
    };
    let total = sizes.total();
    ModelFacts {
        file_name: "model-Q4_K_M.gguf".into(),
        size_bytes: total,
        weights_bytes: total,
        projector_bytes: 0,
        header: Some(HeaderFacts {
            trained_ctx: Some(40_960),
            kv_f16: u64::from(blocks) * 2048,
            kv_q8: u64::from(blocks) * 1088,
            vocab: 32_000,
            thinking: false,
        }),
        layers: Some(LayerFacts {
            blocks,
            experts: expert > 0,
            sizes: Some(sizes),
            kv_f16: vec![2048; blocks as usize],
            kv_q8: vec![1088; blocks as usize],
        }),
    }
}

/// 40 blocks of 100 MB, input and output 200 MB (41 units, 4.4 GB).
fn dense() -> ModelFacts {
    model(40, 100_000_000, 0, 200_000_000)
}

/// 48 blocks of 20 MB dense and 200 MB expert parts, input and output 100 MB.
fn moe() -> ModelFacts {
    model(48, 20_000_000, 200_000_000, 100_000_000)
}

fn card(free: u64) -> Machine {
    Machine {
        backend: Some(BackendKind::Vulkan),
        gpu_free_bytes: free,
        ram_free_bytes: 30 * GB,
        gpu_description: "AMD Radeon RX 9070 XT".into(),
        gpu_total_bytes: 16 * GIB,
        ram_total_bytes: 64 * GB,
        threads: 16,
        ..Machine::default()
    }
}

fn processor() -> Machine {
    Machine {
        backend: Some(BackendKind::Cpu),
        ram_free_bytes: 30 * GB,
        ram_total_bytes: 64 * GB,
        threads: 16,
        ..Machine::default()
    }
}

fn mac() -> Machine {
    Machine {
        backend: Some(BackendKind::Metal),
        gpu_free_bytes: 10_922 * MIB,
        gpu_total_bytes: 10_922 * MIB,
        gpu_description: "Apple M2".into(),
        ram_free_bytes: 12 * GB,
        ram_total_bytes: 16 * GIB,
        threads: 8,
        ..Machine::default()
    }
}

fn resolved(ctx: u32, gpu: GpuUse, fine: FineLaunch) -> ResolvedLoad {
    ResolvedLoad {
        ctx,
        ctx_choice: ContextSize::Fixed(ctx),
        ctx_source: ContextSource::ModelSetting,
        gpu,
        save_memory: false,
        keep_in_memory: false,
        thinking_limit: ThinkingLimit::NoLimit,
        backend: None,
        device: None,
        mmproj: None,
        fine,
    }
}

fn picture(facts: &ModelFacts, machine: &Machine, r: &ResolvedLoad) -> OffloadPreview {
    estimate("k".into(), facts, machine, r)
}

fn layers(n: u32) -> FineLaunch {
    FineLaunch {
        gpu_layers: Some(n),
        ..FineLaunch::default()
    }
}

const QUICK: PictureSource = PictureSource::Estimate {
    why: EstimateWhy::Quick,
    backend: None,
};

#[test]
fn on_the_processor_only_one_bar_holds_everything() {
    let d = dense();
    let p = picture(
        &d,
        &processor(),
        &resolved(8192, GpuUse::All, FineLaunch::default()),
    );
    assert_eq!(p.source, QUICK);
    assert!(p.graphics.is_none() && !p.unified && !p.custom && !p.layers_about);
    assert_eq!((p.units, p.layers_on_graphics), (Some(41), 0));
    assert_eq!(p.expert_layers_on_processor, None);
    let need = p.computer.need;
    assert_eq!(need.model, 4_400_000_000);
    assert_eq!(need.context, 40 * 2048 * 8192);
    let compute = models::FitInput {
        vocab_size: 32_000,
        ..Default::default()
    }
    .compute_bytes();
    assert_eq!(need.compute, compute);
    assert_eq!(p.computer.total_bytes, 64 * GB);
    assert_eq!(p.computer.free_bytes, 30 * GB);
    assert_eq!(p.computer.other_bytes, 34 * GB);
    assert!(!p.computer.free_after_unload);
    // The processor's verdict is fit.rs's Room::verdict of the total.
    let total = need.total();
    assert_eq!(p.mark, models::fit::Room::processor(30 * GB).verdict(total));
    assert_eq!((p.mark, p.fits), (Verdict::Green, Fits::Fits));
}

#[test]
fn all_of_it_puts_every_unit_on_the_card() {
    let d = dense();
    let p = picture(
        &d,
        &card(15 * GB),
        &resolved(8192, GpuUse::All, FineLaunch::default()),
    );
    let g = p.graphics.clone().expect("a graphics bar");
    assert_eq!(g.name, "AMD Radeon RX 9070 XT");
    assert_eq!((g.total_bytes, g.free_bytes), (16 * GIB, 15 * GB));
    assert_eq!(g.other_bytes, 16 * GIB - 15 * GB);
    assert_eq!(
        g.need.model, 4_200_000_000,
        "every block and the output unit"
    );
    assert_eq!(g.need.context, 40 * 2048 * 8192);
    assert_eq!(p.computer.need.model, 200_000_000, "the input layer stays");
    assert_eq!(p.computer.need.compute, 0);
    assert_eq!((p.units, p.layers_on_graphics), (Some(41), 41));
    assert!(!p.layers_about && !p.custom);
    assert_eq!((p.mark, p.fits), (Verdict::Green, Fits::Fits));
}

#[test]
fn a_custom_placement_is_exact() {
    let d = dense();
    let p = picture(&d, &card(15 * GB), &resolved(8192, GpuUse::Fit, layers(30)));
    assert!(p.custom && !p.layers_about);
    assert_eq!(p.layers_on_graphics, 30);
    let g = p.graphics.unwrap();
    // The output unit and 29 blocks on the card; 11 blocks and the input on the processor.
    assert_eq!(g.need.model, 200_000_000 + 29 * 100_000_000);
    assert_eq!(p.computer.need.model, 200_000_000 + 11 * 100_000_000);
    assert_eq!(p.computer.need.context, 11 * 2048 * 8192);
    assert_eq!(p.computer.need.compute, models::fit::COMPUTE_BASE_BYTES);
    // A custom placement is judged like All of it: never "partial".
    assert_eq!(p.fits, Fits::Fits);
}

#[test]
fn as_much_as_fits_is_about() {
    let d = dense();
    // 3 GB free: not every layer fits.
    let p = picture(
        &d,
        &card(3 * GB),
        &resolved(8192, GpuUse::Fit, FineLaunch::default()),
    );
    assert!(p.layers_about && !p.custom);
    assert!(
        p.layers_on_graphics > 0 && p.layers_on_graphics < 41,
        "{}",
        p.layers_on_graphics
    );
    assert_eq!((p.mark, p.fits), (Verdict::Yellow, Fits::Partial));
    // What it put on the card fits under the free memory less llama.cpp's margin.
    let g = p.graphics.unwrap();
    let on_card = g.need.model + g.need.context + g.need.compute;
    assert!(on_card <= 3 * GB - 1024 * MIB, "{on_card}");
    // Leave free: a larger margin, fewer layers.
    let more_free = picture(
        &d,
        &card(3 * GB),
        &resolved(
            8192,
            GpuUse::Fit,
            FineLaunch {
                fit_margin_mib: Some(2048),
                ..FineLaunch::default()
            },
        ),
    );
    assert!(more_free.layers_on_graphics < p.layers_on_graphics);
    // Free memory below the margin: no layer on the card.
    let none = picture(
        &d,
        &card(512 * MIB),
        &resolved(8192, GpuUse::Fit, FineLaunch::default()),
    );
    assert_eq!(none.layers_on_graphics, 0);
    assert!(none.layers_about);
}

#[test]
fn as_much_as_fits_keeps_expert_parts_of_the_last_layers_on_the_processor() {
    let m = moe();
    // 7 GB free: every dense part fits, not every expert part (48 × 200 MB).
    let p = picture(
        &m,
        &card(7 * GB),
        &resolved(4096, GpuUse::Fit, FineLaunch::default()),
    );
    assert!(p.layers_about);
    assert_eq!((p.units, p.layers_on_graphics), (Some(49), 49));
    let k = p.expert_layers_on_processor.expect("a mixture of experts");
    assert!(k > 0 && k < 48, "{k}");
    assert_eq!(p.fits, Fits::Partial);
    // Chosen expert parts: the effective count.
    let chosen = picture(
        &m,
        &card(15 * GB),
        &resolved(
            4096,
            GpuUse::All,
            FineLaunch {
                experts_on_cpu: runtime::ExpertsOnCpu::FirstLayers(20),
                ..FineLaunch::default()
            },
        ),
    );
    assert_eq!(chosen.expert_layers_on_processor, Some(20));
    assert!(chosen.custom);
    // A dense model has no expert count.
    assert_eq!(
        picture(
            &dense(),
            &card(15 * GB),
            &resolved(4096, GpuUse::All, FineLaunch::default())
        )
        .expert_layers_on_processor,
        None
    );
}

#[test]
fn the_loaded_model_counts_as_free_and_says_so() {
    let d = dense();
    let m = Machine {
        gpu_freed_bytes: 4 * GB,
        ram_free_after_unload: true,
        ..card(10 * GB)
    };
    let p = picture(&d, &m, &resolved(8192, GpuUse::All, FineLaunch::default()));
    assert!(p.graphics.unwrap().free_after_unload);
    assert!(p.computer.free_after_unload);
    let p = picture(
        &d,
        &card(10 * GB),
        &resolved(8192, GpuUse::All, FineLaunch::default()),
    );
    assert!(!p.graphics.unwrap().free_after_unload);
    // On a Mac the freed memory comes back to the one shared memory.
    let apple = Machine {
        gpu_freed_bytes: 3 * GB,
        ..mac()
    };
    let p = picture(
        &d,
        &apple,
        &resolved(8192, GpuUse::All, FineLaunch::default()),
    );
    assert!(p.computer.free_after_unload);
    assert_eq!(p.computer.free_bytes, 15 * GB);
}

#[test]
fn the_image_reader_is_added_where_it_runs() {
    let d = ModelFacts {
        projector_bytes: 900_000_000,
        ..dense()
    };
    let p = picture(
        &d,
        &card(15 * GB),
        &resolved(8192, GpuUse::All, FineLaunch::default()),
    );
    assert_eq!(p.graphics.unwrap().need.image_reader, 900_000_000);
    assert_eq!(p.computer.need.image_reader, 0);
    let p = picture(
        &d,
        &processor(),
        &resolved(8192, GpuUse::All, FineLaunch::default()),
    );
    assert_eq!(p.computer.need.image_reader, 900_000_000);
    let p = picture(
        &d,
        &card(15 * GB),
        &resolved(8192, GpuUse::Off, FineLaunch::default()),
    );
    assert!(p.graphics.is_none());
    assert_eq!(p.computer.need.image_reader, 900_000_000);
}

#[test]
fn every_verdict_detail() {
    let d = dense();
    let at = |machine: Machine, gpu, fine| picture(&d, &machine, &resolved(8192, gpu, fine));
    // Card part with 8,192 tokens: 4.2 GB + 0.67 GB + the working space ≈ 5.2 GB.
    let fits = at(card(15 * GB), GpuUse::All, FineLaunch::default());
    assert_eq!((fits.mark, fits.fits), (Verdict::Green, Fits::Fits));
    let tight = at(card(5_600_000_000), GpuUse::All, FineLaunch::default());
    assert_eq!((tight.mark, tight.fits), (Verdict::Yellow, Fits::Tight));
    let wont = at(card(4 * GB), GpuUse::All, FineLaunch::default());
    assert_eq!(
        (wont.mark, wont.fits),
        (Verdict::Yellow, Fits::WontFitGraphics)
    );
    let custom_wont = at(card(4 * GB), GpuUse::All, layers(41));
    assert_eq!(custom_wont.fits, Fits::WontFitGraphics);
    let partial = at(card(4 * GB), GpuUse::Fit, FineLaunch::default());
    assert_eq!(
        (partial.mark, partial.fits),
        (Verdict::Yellow, Fits::Partial)
    );
    let none = Machine {
        ram_free_bytes: 3 * GB,
        ..card(GB)
    };
    let at_all = at(none, GpuUse::All, FineLaunch::default());
    assert_eq!(
        (at_all.mark, at_all.fits),
        (Verdict::Red, Fits::WontFitAtAll)
    );
    // As much as fits is never "won't fit in the graphics card" (llama.cpp fits again).
    let p = at(card(512 * MIB), GpuUse::Fit, FineLaunch::default());
    assert_ne!(p.fits, Fits::WontFitGraphics);
}

#[test]
fn apple_has_one_bar_with_a_marker() {
    let d = dense();
    let p = picture(
        &d,
        &mac(),
        &resolved(8192, GpuUse::All, FineLaunch::default()),
    );
    assert!(p.unified);
    assert!(p.graphics.is_none());
    assert_eq!(p.graphics_limit_bytes, Some(10_922 * MIB));
    assert_eq!(p.computer.total_bytes, 16 * GIB);
    assert_eq!(
        p.computer.need.model, 4_400_000_000,
        "both parts in one bar"
    );
    assert_eq!(p.layers_on_graphics, 41);
    // Don't use it: one bar, no marker.
    let p = picture(
        &d,
        &mac(),
        &resolved(8192, GpuUse::Off, FineLaunch::default()),
    );
    assert!(!p.unified && p.graphics_limit_bytes.is_none());
}

#[test]
fn unreadable_tables_give_the_estimate_from_the_file_sizes() {
    let d = ModelFacts {
        layers: None,
        projector_bytes: 100,
        ..dense()
    };
    let p = picture(
        &d,
        &card(15 * GB),
        &resolved(8192, GpuUse::All, FineLaunch::default()),
    );
    assert_eq!(
        p.source,
        PictureSource::Estimate {
            why: EstimateWhy::NotReadable,
            backend: None
        }
    );
    assert_eq!(p.units, None);
    let need = p.graphics.unwrap().need;
    assert_eq!(need.model, 4_400_000_000);
    assert_eq!(need.context, 40 * 2048 * 8192);
    assert_eq!(need.image_reader, 100);
    let input = d.fit_input(false).unwrap();
    assert_eq!(
        p.mark,
        models::fit::verdict_at(&input, 8192, card(15 * GB).room(GpuUse::All))
    );
    // Without a header: the file sizes only, and the app cannot say.
    let bare = ModelFacts { header: None, ..d };
    let p = picture(
        &bare,
        &card(15 * GB),
        &resolved(8192, GpuUse::All, FineLaunch::default()),
    );
    assert_eq!((p.mark, p.fits), (Verdict::Yellow, Fits::Partial));
    assert_eq!(p.graphics.unwrap().need.context, 0);
}

#[test]
fn the_key_names_the_draft() {
    let a = ModelLoadSettings::default();
    let b = ModelLoadSettings {
        fine: crate::load_settings::FineControl {
            threads: crate::load_settings::ThreadsChoice::Count(2),
            ..Default::default()
        },
        ..ModelLoadSettings::default()
    };
    assert_eq!(draft_key("/m.gguf", &a), draft_key("/m.gguf", &a));
    assert_ne!(draft_key("/m.gguf", &a), draft_key("/m.gguf", &b));
    assert_ne!(draft_key("/m.gguf", &a), draft_key("/n.gguf", &a));
    assert_eq!(draft_key("/m.gguf", &a).len(), 16);
}

// ---- The measurement ------------------------------------------------------------------------

fn row(kind: MemoryRowKind, name: &str, model: u64, context: u64, compute: u64) -> MemoryRow {
    MemoryRow {
        kind,
        name: name.into(),
        description: String::new(),
        total_bytes: None,
        free_bytes: None,
        model_bytes: model,
        context_bytes: context,
        compute_bytes: compute,
    }
}

fn file(path: &str) -> FileVersion {
    FileVersion {
        path: PathBuf::from(path),
        size: 1,
        modified: None,
        backend: BackendKind::Cpu,
        build: "b1".into(),
    }
}

fn key(path: &str, ctx: u32) -> MeasureKey {
    MeasureKey {
        file: file(path),
        device: None,
        ctx,
        save_memory: false,
        flash_attn: FlashAttn::Auto,
        kv_in_computer_memory: false,
        layers: MeasureLayers::Count(0),
        experts: ExpertsOnCpu::None,
    }
}

fn config(path: &str, ctx: u32) -> MeasureConfig {
    MeasureConfig {
        model: PathBuf::from(path),
        ctx,
        device: None,
        layers: MeasureLayers::Count(0),
        experts_on_cpu: ExpertsOnCpu::None,
        kv_in_computer_memory: false,
        save_memory: false,
        flash_attn: FlashAttn::Auto,
        hip_visible_devices: None,
    }
}

fn dev_backend(kind: BackendKind, fit_params: bool) -> Backend {
    Backend {
        kind,
        dir: PathBuf::from("/dev-llama"),
        server: PathBuf::from("/dev-llama/llama-server"),
        fit_params: fit_params.then(|| PathBuf::from("/dev-llama/llama-fit-params")),
        build: "dev".into(),
        source: runtime::BackendSource::DevOverride,
    }
}

/// A fake fit-params: counts its runs, keeps their settings and answers with `answer`.
fn fake(
    calls: Arc<AtomicUsize>,
    seen: Arc<StdMutex<Vec<MeasureConfig>>>,
    answer: MeasureResult,
) -> Runner {
    Arc::new(move |_backend, cfg, _timeout, _cancel| {
        calls.fetch_add(1, Ordering::SeqCst);
        seen.lock().unwrap().push(cfg);
        let answer = answer.clone();
        async move { answer }.boxed()
    })
}

#[tokio::test]
async fn results_are_cached_and_failures_run_once_per_opening() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let rows = vec![row(MemoryRowKind::Host, "Host", 1, 2, 3)];
    let m = Measurements::with_runner(
        fake(calls.clone(), seen.clone(), Ok(rows.clone())),
        Duration::from_secs(5),
    );
    let backend = dev_backend(BackendKind::Cpu, true);
    for _ in 0..3 {
        assert_eq!(
            m.measure(backend.clone(), config("/a", 4096), key("/a", 4096))
                .await,
            Outcome::Rows(rows.clone())
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "a cache hit runs nothing");
    // A failure is remembered per file version and build, whatever the settings.
    let calls = Arc::new(AtomicUsize::new(0));
    let m = Measurements::with_runner(
        fake(
            calls.clone(),
            seen.clone(),
            Err(FitParamsError::Failed { code: Some(134) }),
        ),
        Duration::from_secs(5),
    );
    assert_eq!(
        m.measure(backend.clone(), config("/b", 4096), key("/b", 4096))
            .await,
        Outcome::Failed(EstimateWhy::Failed)
    );
    assert_eq!(
        m.measure(backend.clone(), config("/b", 8192), key("/b", 8192))
            .await,
        Outcome::Failed(EstimateWhy::Failed)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "a crashing file runs once");
    // Another file is not affected; opening the panel again allows one more run.
    m.measure(backend.clone(), config("/c", 4096), key("/c", 4096))
        .await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    m.forget_failures(Path::new("/b"));
    m.measure(backend.clone(), config("/b", 4096), key("/b", 4096))
        .await;
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    // A time-out is remembered the same way, with its own reason.
    let calls = Arc::new(AtomicUsize::new(0));
    let m = Measurements::with_runner(
        fake(calls.clone(), seen, Err(FitParamsError::Timeout)),
        Duration::from_secs(5),
    );
    for ctx in [4096, 8192] {
        assert_eq!(
            m.measure(backend.clone(), config("/d", ctx), key("/d", ctx))
                .await,
            Outcome::Failed(EstimateWhy::TimedOut)
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_same_settings_join_the_run_and_others_cancel_it() {
    let calls = Arc::new(AtomicUsize::new(0));
    let cancelled = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let runner: Runner = {
        let calls = calls.clone();
        let cancelled = cancelled.clone();
        let release = release.clone();
        Arc::new(move |_b, cfg, _t, cancel: CancellationToken| {
            calls.fetch_add(1, Ordering::SeqCst);
            let cancelled = cancelled.clone();
            let release = release.clone();
            async move {
                tokio::select! {
                    () = cancel.cancelled() => {
                        cancelled.fetch_add(1, Ordering::SeqCst);
                        Err(FitParamsError::Cancelled)
                    }
                    () = release.notified() => Ok(vec![row(MemoryRowKind::Host, "Host", u64::from(cfg.ctx), 0, 0)]),
                }
            }
            .boxed()
        })
    };
    let m = Arc::new(Measurements::with_runner(runner, Duration::from_secs(5)));
    let backend = dev_backend(BackendKind::Cpu, true);
    // Two requests for the same settings: one run, both answered.
    let first = tokio::spawn({
        let (m, b) = (m.clone(), backend.clone());
        async move { m.measure(b, config("/a", 4096), key("/a", 4096)).await }
    });
    let second = tokio::spawn({
        let (m, b) = (m.clone(), backend.clone());
        async move { m.measure(b, config("/a", 4096), key("/a", 4096)).await }
    });
    while calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    release.notify_waiters();
    let expected = Outcome::Rows(vec![row(MemoryRowKind::Host, "Host", 4096, 0, 0)]);
    assert_eq!(first.await.unwrap(), expected);
    assert_eq!(second.await.unwrap(), expected);
    assert_eq!(calls.load(Ordering::SeqCst), 1, "joined, not run twice");
    // A newer request for other settings cancels the run going.
    let older = tokio::spawn({
        let (m, b) = (m.clone(), backend.clone());
        async move { m.measure(b, config("/a", 8192), key("/a", 8192)).await }
    });
    while calls.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    let newer = tokio::spawn({
        let (m, b) = (m.clone(), backend.clone());
        async move { m.measure(b, config("/a", 16_384), key("/a", 16_384)).await }
    });
    assert_eq!(older.await.unwrap(), Outcome::Replaced);
    assert_eq!(cancelled.load(Ordering::SeqCst), 1);
    while calls.load(Ordering::SeqCst) < 3 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    release.notify_waiters();
    assert_eq!(
        newer.await.unwrap(),
        Outcome::Rows(vec![row(MemoryRowKind::Host, "Host", 16_384, 0, 0)])
    );
    // A replaced run is not remembered as a failure: asked again, it runs.
    let again = tokio::spawn({
        let (m, b) = (m.clone(), backend.clone());
        async move { m.measure(b, config("/a", 8192), key("/a", 8192)).await }
    });
    while calls.load(Ordering::SeqCst) < 4 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    release.notify_waiters();
    assert!(matches!(again.await.unwrap(), Outcome::Rows(_)));
}

/// A file on disk standing for a model (the measurement's key reads its size and time).
struct TempModel(PathBuf);

impl TempModel {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("offload-{}.gguf", uuid::Uuid::new_v4()));
        std::fs::write(&p, b"model").unwrap();
        Self(p)
    }
    fn path(&self) -> String {
        self.0.display().to_string()
    }
}

impl Drop for TempModel {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn plan(facts: ModelFacts, machine: Machine, r: ResolvedLoad) -> Plan {
    Plan {
        resolved: r,
        facts: Some(facts),
        machine,
        overrides: ModelLoadSettings::default(),
        defaults: LocalModelSettings::default(),
        saved: false,
    }
}

fn probed(backend: Backend, devices: &[(&str, &str, u64, u64)]) -> (Backend, ProbeResult) {
    let kind = backend.kind;
    (
        backend,
        ProbeResult {
            backend_kind: kind,
            devices: devices
                .iter()
                .map(|(name, description, total, free)| runtime::Device {
                    name: (*name).into(),
                    description: (*description).into(),
                    total_mib: *total,
                    free_mib: *free,
                })
                .collect(),
            error: None,
            output: Vec::new(),
        },
    )
}

#[tokio::test]
async fn a_measurement_replaces_the_estimate() {
    let model = TempModel::new();
    let d = ModelFacts {
        projector_bytes: 50,
        ..dense()
    };
    // On the processor: the host and the repacked weights, the projector added.
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let m = Measurements::with_runner(
        fake(
            calls.clone(),
            seen.clone(),
            Ok(vec![
                row(MemoryRowKind::Host, "Host", 157 * MIB, 448 * MIB, 312 * MIB),
                row(MemoryRowKind::BufferType, "CPU_REPACK", 674 * MIB, 0, 0),
            ]),
        ),
        Duration::from_secs(5),
    );
    let found = vec![probed(
        dev_backend(BackendKind::Cpu, true),
        &[("CPU", "CPU", 0, 0)],
    )];
    let p = plan(
        d.clone(),
        processor(),
        resolved(4096, GpuUse::All, FineLaunch::default()),
    );
    let estimate = estimate(
        draft_key(&model.path(), &p.overrides),
        &d,
        &p.machine,
        &p.resolved,
    );
    let got = measured(&m, &model.path(), &p, &d, &found).await;
    assert_eq!(got.source, PictureSource::Measured);
    assert_eq!(got.key, estimate.key, "the same draft, the same key");
    assert_eq!(
        got.computer.need,
        Parts {
            model: 831 * MIB,
            context: 448 * MIB,
            compute: 312 * MIB,
            image_reader: 50,
        }
    );
    assert!(got.graphics.is_none());
    assert_eq!((got.units, got.layers_on_graphics), (Some(41), 0));
    let cfg = seen.lock().unwrap().pop().unwrap();
    assert_eq!(
        (cfg.device.clone(), cfg.layers, cfg.ctx),
        (None, MeasureLayers::Count(0), 4096)
    );
    // On a graphics card: device rows on its bar, with the probe's device and the layers.
    let calls = Arc::new(AtomicUsize::new(0));
    let m = Measurements::with_runner(
        fake(
            calls,
            seen.clone(),
            Ok(vec![
                row(
                    MemoryRowKind::Device,
                    "Vulkan0",
                    9_900 * MIB,
                    1_200 * MIB,
                    600 * MIB,
                ),
                row(
                    MemoryRowKind::Host,
                    "Host",
                    4_100 * MIB,
                    400 * MIB,
                    300 * MIB,
                ),
            ]),
        ),
        Duration::from_secs(5),
    );
    let found = vec![
        probed(
            dev_backend(BackendKind::Vulkan, true),
            &[("Vulkan0", "AMD Radeon RX 9070 XT", 16_304, 15_000)],
        ),
        probed(dev_backend(BackendKind::Cpu, true), &[("CPU", "CPU", 0, 0)]),
    ];
    let p = plan(
        d.clone(),
        card(15 * GB),
        resolved(16_384, GpuUse::All, layers(30)),
    );
    let got = measured(&m, &model.path(), &p, &d, &found).await;
    assert_eq!(got.source, PictureSource::Measured);
    let g = got.graphics.unwrap();
    assert_eq!(
        g.need,
        Parts {
            model: 9_900 * MIB,
            context: 1_200 * MIB,
            compute: 600 * MIB,
            image_reader: 50,
        }
    );
    assert_eq!(got.computer.need.model, 4_100 * MIB);
    assert_eq!(got.layers_on_graphics, 30);
    assert!(got.custom);
    let cfg = seen.lock().unwrap().pop().unwrap();
    assert_eq!(cfg.device.as_deref(), Some("Vulkan0"));
    assert_eq!(cfg.layers, MeasureLayers::Count(30));
}

#[tokio::test]
async fn reasons_for_an_estimate() {
    let model = TempModel::new();
    let d = dense();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    // A backend that cannot measure: noTool at once, with the backend; nothing runs.
    let calls = Arc::new(AtomicUsize::new(0));
    let m = Measurements::with_runner(
        fake(calls.clone(), seen.clone(), Ok(Vec::new())),
        Duration::from_secs(5),
    );
    let found = vec![
        probed(
            dev_backend(BackendKind::Vulkan, false),
            &[("Vulkan0", "AMD Radeon RX 9070 XT", 16_304, 15_000)],
        ),
        probed(
            dev_backend(BackendKind::Cpu, false),
            &[("CPU", "CPU", 0, 0)],
        ),
    ];
    let p = plan(
        d.clone(),
        card(15 * GB),
        resolved(4096, GpuUse::All, FineLaunch::default()),
    );
    let got = measured(&m, &model.path(), &p, &d, &found).await;
    assert_eq!(
        got.source,
        PictureSource::Estimate {
            why: EstimateWhy::NoTool,
            backend: Some(BackendKind::Vulkan)
        }
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    // A failure and a time-out: the estimate with the reason.
    for (error, why) in [
        (FitParamsError::Failed { code: None }, EstimateWhy::Failed),
        (FitParamsError::NoReport, EstimateWhy::Failed),
        (FitParamsError::Timeout, EstimateWhy::TimedOut),
    ] {
        let m = Measurements::with_runner(
            fake(Arc::new(AtomicUsize::new(0)), seen.clone(), Err(error)),
            Duration::from_secs(5),
        );
        let found = vec![probed(
            dev_backend(BackendKind::Cpu, true),
            &[("CPU", "CPU", 0, 0)],
        )];
        let p = plan(
            d.clone(),
            processor(),
            resolved(4096, GpuUse::All, FineLaunch::default()),
        );
        let got = measured(&m, &model.path(), &p, &d, &found).await;
        assert_eq!(got.source, PictureSource::Estimate { why, backend: None });
        assert_eq!(
            got.computer.need.model, 4_400_000_000,
            "the estimate's figures"
        );
    }
    // Unreadable tables: the estimate says so and nothing runs.
    let calls = Arc::new(AtomicUsize::new(0));
    let m = Measurements::with_runner(
        fake(calls.clone(), seen, Ok(Vec::new())),
        Duration::from_secs(5),
    );
    let unreadable = ModelFacts {
        layers: None,
        ..dense()
    };
    let found = vec![probed(
        dev_backend(BackendKind::Cpu, true),
        &[("CPU", "CPU", 0, 0)],
    )];
    let p = plan(
        unreadable.clone(),
        processor(),
        resolved(4096, GpuUse::All, FineLaunch::default()),
    );
    let got = measured(&m, &model.path(), &p, &unreadable, &found).await;
    assert_eq!(
        got.source,
        PictureSource::Estimate {
            why: EstimateWhy::NotReadable,
            backend: None
        }
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn as_much_as_fits_is_measured_with_the_first_expert_layers() {
    let candidate = runtime::Candidate {
        backend: dev_backend(BackendKind::Rocm, true),
        device: Some("ROCm0".into()),
        hip_visible_devices: Some("1".into()),
    };
    let r = resolved(8192, GpuUse::Fit, FineLaunch::default());
    let place = |layers, experts_on_cpu| Where {
        graphics: true,
        layers,
        experts_on_cpu,
        kv_on_gpu: true,
    };
    // Every unit on the card, the expert parts of the last 19 blocks on the processor.
    let cfg = measure_config(
        "/m",
        &r,
        &place(Layers::All, ExpertsPick::LastLayers(19)),
        49,
        &candidate,
    );
    assert_eq!(cfg.layers, MeasureLayers::All);
    assert_eq!(cfg.experts_on_cpu, ExpertsOnCpu::FirstLayers(19));
    assert_eq!(cfg.device.as_deref(), Some("ROCm0"));
    assert_eq!(cfg.hip_visible_devices.as_deref(), Some("1"));
    // 30 of 49 units: 19 blocks already on the processor, plus 28 on the card.
    let cfg = measure_config(
        "/m",
        &r,
        &place(Layers::Count(30), ExpertsPick::LastLayers(28)),
        49,
        &candidate,
    );
    assert_eq!(cfg.experts_on_cpu, ExpertsOnCpu::FirstLayers(47));
    // Never more than the blocks.
    let cfg = measure_config(
        "/m",
        &r,
        &place(Layers::Count(10), ExpertsPick::LastLayers(40)),
        49,
        &candidate,
    );
    assert_eq!(cfg.experts_on_cpu, ExpertsOnCpu::FirstLayers(48));
    // Without a graphics device: no device, no layers, no mask.
    let cfg = measure_config(
        "/m",
        &r,
        &Where {
            graphics: false,
            layers: Layers::Count(0),
            experts_on_cpu: ExpertsPick::None,
            kv_on_gpu: true,
        },
        49,
        &candidate,
    );
    assert_eq!(
        (
            cfg.device,
            cfg.layers,
            cfg.hip_visible_devices,
            cfg.experts_on_cpu
        ),
        (None, MeasureLayers::Count(0), None, ExpertsOnCpu::None)
    );
}

// ---- Where the loaded model went ------------------------------------------------------------

fn report(rows: Vec<MemoryRow>) -> MemoryReport {
    MemoryReport {
        n_ctx: Some(8192),
        rows,
    }
}

fn launch<'a>(
    sizes: Option<&'a LayerSizes>,
    gpu: GpuUse,
    fine: &'a FineLaunch,
    description: Option<&str>,
) -> LaunchFacts<'a> {
    LaunchFacts {
        sizes,
        graphics: gpu != GpuUse::Off,
        gpu,
        fine,
        device_description: description.map(str::to_owned),
        apple: false,
        projector_bytes: 0,
    }
}

/// The model bytes on the card for a placement of `sizes`.
fn model_on_card(sizes: &LayerSizes, layers: Layers, experts: ExpertsPick) -> u64 {
    placement::split(
        &PlacementInput {
            sizes,
            kv_per_layer: Vec::new(),
            compute_gpu: 0,
            compute_cpu: 0,
            projector: 0,
        },
        &Where {
            graphics: true,
            layers,
            experts_on_cpu: experts,
            kv_on_gpu: true,
        },
    )
    .gpu
    .model
}

#[test]
fn placement_from_memory_report() {
    let d = dense();
    let dense_sizes = d.layers.as_ref().unwrap().sizes.clone().unwrap();
    let none = FineLaunch::default();
    // The Vulkan shape, All of it: every unit, named by the probe's description.
    let vulkan = report(vec![
        row(MemoryRowKind::Host, "Host", 200_000_000, 0, 30_000_000),
        row(
            MemoryRowKind::Device,
            "Vulkan0",
            4_200_000_000,
            671_000_000,
            330_000_000,
        ),
    ]);
    let p = placement_from_report(
        &vulkan,
        &LaunchFacts {
            projector_bytes: 900,
            ..launch(
                Some(&dense_sizes),
                GpuUse::All,
                &none,
                Some("AMD Radeon RX 9070 XT"),
            )
        },
    );
    let g = p.graphics.clone().unwrap();
    assert_eq!(g.name, "AMD Radeon RX 9070 XT");
    assert_eq!(
        g.parts,
        Parts {
            model: 4_200_000_000,
            context: 671_000_000,
            compute: 330_000_000,
            image_reader: 900,
        }
    );
    assert_eq!(p.computer.model, 200_000_000);
    assert_eq!((p.units, p.layers_on_graphics), (Some(41), Some(41)));
    assert!(!p.layers_about && !p.custom);
    assert_eq!(p.expert_layers_on_processor, None);
    // No description known: plain words.
    let p = placement_from_report(
        &vulkan,
        &launch(Some(&dense_sizes), GpuUse::All, &none, None),
    );
    assert_eq!(p.graphics.unwrap().name, "Graphics card");
    let p = placement_from_report(
        &vulkan,
        &LaunchFacts {
            apple: true,
            ..launch(Some(&dense_sizes), GpuUse::All, &none, Some(""))
        },
    );
    assert_eq!(p.graphics.unwrap().name, "Apple graphics");
    // The processor shape: the host and the repacked weights, nothing on a device.
    let cpu = report(vec![
        row(
            MemoryRowKind::Host,
            "Host",
            165_568_512,
            469_762_048,
            33_572_896,
        ),
        row(MemoryRowKind::BufferType, "AMX", 707_731_456, 0, 0),
    ]);
    let p = placement_from_report(
        &cpu,
        &LaunchFacts {
            projector_bytes: 7,
            ..launch(Some(&dense_sizes), GpuUse::Off, &none, None)
        },
    );
    assert!(p.graphics.is_none());
    assert_eq!(p.computer.model, 165_568_512 + 707_731_456);
    assert_eq!(p.computer.image_reader, 7);
    assert_eq!(p.layers_on_graphics, Some(0));
    // A custom placement: the chosen count, marked custom.
    let chosen = layers(30);
    let p = placement_from_report(
        &vulkan,
        &launch(Some(&dense_sizes), GpuUse::Fit, &chosen, None),
    );
    assert_eq!(p.layers_on_graphics, Some(30));
    assert!(p.custom && !p.layers_about);
    // As much as fits on a dense model: the count from the bytes, "About" (WP2's partial
    // note: fewer than all).
    let thirty = model_on_card(&dense_sizes, Layers::Count(30), ExpertsPick::None);
    let fit = report(vec![row(
        MemoryRowKind::Device,
        "Vulkan0",
        thirty + 1_000,
        0,
        0,
    )]);
    let p = placement_from_report(&fit, &launch(Some(&dense_sizes), GpuUse::Fit, &none, None));
    assert_eq!((p.layers_on_graphics, p.units), (Some(30), Some(41)));
    assert!(p.layers_about && !p.custom);
    // A mixture of experts with As much as fits: every unit, expert parts of the last 19.
    let m = moe();
    let moe_sizes = m.layers.as_ref().unwrap().sizes.clone().unwrap();
    let bytes = model_on_card(&moe_sizes, Layers::All, ExpertsPick::LastLayers(19));
    let fit = report(vec![row(MemoryRowKind::Device, "Vulkan0", bytes, 0, 0)]);
    let p = placement_from_report(&fit, &launch(Some(&moe_sizes), GpuUse::Fit, &none, None));
    assert_eq!(p.layers_on_graphics, Some(49));
    assert_eq!(p.expert_layers_on_processor, Some(19));
    assert!(p.layers_about);
    // Chosen expert parts: the effective count.
    let first20 = FineLaunch {
        experts_on_cpu: ExpertsOnCpu::FirstLayers(20),
        ..FineLaunch::default()
    };
    let p = placement_from_report(
        &vulkan,
        &launch(Some(&moe_sizes), GpuUse::All, &first20, None),
    );
    assert_eq!(p.expert_layers_on_processor, Some(20));
    assert!(p.custom);
    // Bytes the order cannot explain: amounts only.
    let between = model_on_card(&dense_sizes, Layers::Count(30), ExpertsPick::None) + 50_000_000;
    let odd = report(vec![row(MemoryRowKind::Device, "Vulkan0", between, 0, 0)]);
    let p = placement_from_report(&odd, &launch(Some(&dense_sizes), GpuUse::Fit, &none, None));
    assert_eq!(
        (p.layers_on_graphics, p.expert_layers_on_processor),
        (None, None)
    );
    assert!(p.graphics.is_some());
    // The model's sizes unknown: All of it gives no count either.
    let p = placement_from_report(&vulkan, &launch(None, GpuUse::All, &none, None));
    assert_eq!((p.units, p.layers_on_graphics), (None, None));
}

#[test]
fn the_report_says_what_unloading_frees() {
    let r = report(vec![
        row(MemoryRowKind::Device, "Vulkan0", 100, 20, 3),
        row(MemoryRowKind::Device, "Vulkan1", 1, 0, 0),
        row(MemoryRowKind::Host, "Host", 1_000, 40, 5),
        row(MemoryRowKind::BufferType, "CPU_REPACK", 700, 0, 0),
    ]);
    assert_eq!(report_device_bytes(&r), 124);
    // The host's weights are mapped from the file; its conversation memory, working space
    // and the repacked weights are not.
    assert_eq!(report_freed_ram(&r), 40 + 5 + 700);
    let huge = report(vec![row(
        MemoryRowKind::Device,
        "Vulkan0",
        u64::MAX,
        u64::MAX,
        1,
    )]);
    assert_eq!(report_device_bytes(&huge), u64::MAX);
}

/// The JSON the webview reads (src/lib/types.ts: OffloadPreview, MemoryBar, Parts,
/// PictureSource, Placement).
#[test]
fn the_json_the_webview_reads() {
    let d = ModelFacts {
        projector_bytes: 5,
        ..dense()
    };
    let p = picture(&d, &card(15 * GB), &resolved(8192, GpuUse::All, layers(30)));
    let v = serde_json::to_value(&p).unwrap();
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "computer",
            "custom",
            "expertLayersOnProcessor",
            "fits",
            "graphics",
            "graphicsLimitBytes",
            "key",
            "layersAbout",
            "layersOnGraphics",
            "mark",
            "source",
            "unified",
            "units"
        ]
    );
    assert_eq!(
        v["source"],
        serde_json::json!({"kind": "estimate", "why": "quick"})
    );
    assert_eq!(v["fits"], serde_json::json!("fits"));
    assert_eq!(v["mark"], serde_json::json!("green"));
    let bar = &v["graphics"];
    for key in [
        "name",
        "totalBytes",
        "freeBytes",
        "freeAfterUnload",
        "otherBytes",
        "need",
    ] {
        assert!(bar.get(key).is_some(), "{key}");
    }
    assert_eq!(bar["need"]["imageReader"], serde_json::json!(5));
    assert_eq!(
        serde_json::to_value(PictureSource::Estimate {
            why: EstimateWhy::NoTool,
            backend: Some(BackendKind::Rocm)
        })
        .unwrap(),
        serde_json::json!({"kind": "estimate", "why": "noTool", "backend": "rocm"})
    );
    assert_eq!(
        serde_json::to_value(PictureSource::Measured).unwrap(),
        serde_json::json!({"kind": "measured"})
    );
    for (fits, word) in [
        (Fits::Tight, "tight"),
        (Fits::Partial, "partial"),
        (Fits::WontFitGraphics, "wontFitGraphics"),
        (Fits::WontFitAtAll, "wontFitAtAll"),
    ] {
        assert_eq!(serde_json::to_value(fits).unwrap(), serde_json::json!(word));
    }
    let placed = placement_from_report(
        &report(vec![row(MemoryRowKind::Device, "Vulkan0", 1, 2, 3)]),
        &launch(None, GpuUse::All, &FineLaunch::default(), Some("Card")),
    );
    assert_eq!(
        serde_json::to_value(&placed).unwrap(),
        serde_json::json!({
            "graphics": {"name": "Card", "use": {"model": 1, "context": 2, "compute": 3, "imageReader": 0}},
            "computer": {"model": 0, "context": 0, "compute": 0, "imageReader": 0},
            "units": null,
            "layersOnGraphics": null,
            "expertLayersOnProcessor": null,
            "layersAbout": false,
            "custom": false
        })
    );
}
