//! "Where the model goes" before a load, and "where the model went" after it (0.0.6 item 6,
//! `docs/dev/0.0.6/item6-spec.md` sections 2.4, 4 and 5).
//!
//! - [`Placer`]: the split of one model file for one set of choices on this computer
//!   (`models::placement`), and the one verdict ([`Placer::judge`],
//!   `placement::judge_in`) that every fit mark of the settings panel, the "too large" line
//!   and the picture use (`load_settings::build_info`).
//! - [`estimate`]: the instant picture from the model file (its tensor table, the
//!   conversation memory from its header, fit.rs's working-space allowance), source
//!   `estimate { quick }`, or `estimate { notReadable }` from the file sizes when the
//!   tensor tables cannot be read.
//! - [`measured`] and [`Measurements`]: llama.cpp's own measurement
//!   (`llama-fit-params -fitp on`, `runtime::measure`) for exactly the placement on screen,
//!   one run at a time app-wide (a newer request cancels the older run; a request for the
//!   run already going joins it), results cached, failures and time-outs cached per file
//!   version and backend build until the panel is opened again (I6-14), only pinned builds
//!   measured (I6-15). Nothing is saved and nothing loads.
//! - [`placement_from_report`]: where the loaded model went, from the ready line's memory
//!   report (I6-7).
//!
//! Nothing here is a tool action: the commands are main-window only and no tool or model
//! output reaches them (I6-17). Every sum on numbers from llama.cpp, the model file or the
//! system saturates (section 2.0).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime};

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use models::Verdict;
use models::placement::{
    self, Capacity, ExpertsPick, Fits, Layers, MemoryKind, Parts, PlacementInput, Split, Where,
};
use runtime::{
    Backend, BackendKind, ExpertsOnCpu, FineLaunch, FitParamsError, GpuUse, MeasureConfig,
    MeasureLayers, MemoryReport, MemoryRow, MemoryRowKind, ProbeResult,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::dto::{GraphicsUse, Placement};
use crate::load_settings::{
    DEFAULT_LEAVE_FREE_MIB, LayerFacts, Machine, ModelFacts, ModelLoadSettings, Plan, ResolvedLoad,
};

const MIB: u64 = 1024 * 1024;

// ---- What the webview gets (contract 2.4) ------------------------------------------------

/// Where the picture's figures come from. JSON `{"kind":"measured"}` or
/// `{"kind":"estimate","why":"quick"}` (`backend` only with `noTool`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PictureSource {
    /// llama.cpp measured exactly these settings.
    Measured,
    /// The app's own estimate, and why it is not a measurement.
    Estimate {
        why: EstimateWhy,
        #[serde(skip_serializing_if = "Option::is_none")]
        backend: Option<BackendKind>,
    },
}

impl PictureSource {
    fn estimate(why: EstimateWhy) -> Self {
        Self::Estimate { why, backend: None }
    }
}

/// Why the picture is an estimate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EstimateWhy {
    /// The instant estimate (no measurement asked for, or one still running).
    Quick,
    /// This download cannot measure (the ROCm pack, another build).
    NoTool,
    /// The measurement failed for this file.
    Failed,
    /// The measurement took too long.
    TimedOut,
    /// The model file's details (its tensor tables) could not be read.
    NotReadable,
}

/// One memory bar of the picture, in bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryBar {
    /// The graphics device's own name; empty for computer memory (the webview words it).
    pub name: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    /// The free figure counts the loaded model as unloaded ("free once the loaded model is
    /// unloaded").
    pub free_after_unload: bool,
    /// Used by other programs: the size less the free figure.
    pub other_bytes: u64,
    /// What the model needs here.
    pub need: Parts,
}

/// "Where the model goes" for one set of choices (contract 2.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OffloadPreview {
    /// The draft it answers ([`draft_key`]); the webview drops an answer whose key is not
    /// the key of the draft on screen.
    pub key: String,
    pub source: PictureSource,
    /// Apple silicon: one bar of shared memory.
    pub unified: bool,
    /// llama.cpp's units ("of 41 layers"); `None` when the layers are not known.
    pub units: Option<u32>,
    pub layers_on_graphics: u32,
    /// As much as fits: the count is the app's own working of llama.cpp's rule ("About").
    pub layers_about: bool,
    /// Blocks on the graphics card whose expert parts are on the processor (the effective
    /// count); `None` for a model without expert parts or without a graphics device.
    pub expert_layers_on_processor: Option<u32>,
    /// `None` on the processor only and on a Mac.
    pub graphics: Option<MemoryBar>,
    pub computer: MemoryBar,
    /// Apple: the most Apple graphics may use, the marker on the single bar.
    pub graphics_limit_bytes: Option<u64>,
    pub mark: Verdict,
    pub fits: Fits,
    /// Fine control chose the layers or expert parts.
    pub custom: bool,
}

/// The key of the draft `overrides` of the model at `path`: the first 16 hex digits of
/// SHA-256 of the path and the overrides' JSON. `model_load_info` (the picture of a draft)
/// and `offload_measure` give the same key for the same draft.
pub fn draft_key(path: &str, overrides: &ModelLoadSettings) -> String {
    let json = serde_json::to_string(overrides).unwrap_or_default();
    let digest = Sha256::digest(format!("{path}\n{json}").as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

// ---- The split and the one verdict -------------------------------------------------------

/// Everything a split of one model file on this computer needs that does not change with
/// the context. Built only when the model's sizes are known (its tensor tables were read).
pub struct Placer<'a> {
    facts: &'a ModelFacts,
    layers: &'a LayerFacts,
    sizes: &'a models::LayerSizes,
    machine: &'a Machine,
    save_memory: bool,
    /// fit.rs's working-space allowance for the device that holds the output unit.
    compute_gpu: u64,
}

/// One placement worked out: the split, where it puts things, and whether the layer count
/// is As much as fits' own ("About").
pub struct Worked {
    pub split: Split,
    pub place: Where,
    pub about: bool,
}

impl<'a> Placer<'a> {
    /// `None` without a header or without the model's sizes.
    pub fn new(facts: &'a ModelFacts, machine: &'a Machine, save_memory: bool) -> Option<Self> {
        let header = facts.header.as_ref()?;
        let layers = facts.layers.as_ref()?;
        let sizes = layers.sizes.as_ref()?;
        Some(Self {
            facts,
            layers,
            sizes,
            machine,
            save_memory,
            compute_gpu: models::FitInput {
                vocab_size: header.vocab,
                ..models::FitInput::default()
            }
            .compute_bytes(),
        })
    }

    fn input(&self, ctx: u64) -> PlacementInput<'a> {
        PlacementInput {
            sizes: self.sizes,
            kv_per_layer: self.layers.kv_at(ctx, self.save_memory),
            compute_gpu: self.compute_gpu,
            compute_cpu: models::fit::COMPUTE_BASE_BYTES,
            projector: self.facts.projector_bytes,
        }
    }

    /// Where the model goes at `ctx` tokens with "Use the graphics card" `gpu` and Fine
    /// control `fine` (as the resolver left it, item 6 4.2 step 4): everything on the
    /// processor without a graphics device; a custom placement as chosen; All of it every
    /// unit; As much as fits the app's working of llama.cpp's fit
    /// (`placement::fit_layers_where`) into the free graphics memory less the margin
    /// (Leave free, else llama.cpp's 1,024 MiB), "About".
    pub fn work(&self, ctx: u64, gpu: GpuUse, fine: &FineLaunch) -> Worked {
        let input = self.input(ctx);
        let (place, about) = self.place(&input, gpu, fine);
        Worked {
            split: placement::split(&input, &place),
            place,
            about,
        }
    }

    fn place(&self, input: &PlacementInput, gpu: GpuUse, fine: &FineLaunch) -> (Where, bool) {
        if !self.machine.graphics(gpu) {
            return (
                Where {
                    graphics: false,
                    layers: Layers::Count(0),
                    experts_on_cpu: ExpertsPick::None,
                    kv_on_gpu: true,
                },
                false,
            );
        }
        let kv_on_gpu = !fine.kv_in_computer_memory;
        if fine.custom_placement() {
            return (
                Where {
                    graphics: true,
                    layers: fine.gpu_layers.map_or(Layers::All, Layers::Count),
                    experts_on_cpu: experts_pick(fine.experts_on_cpu),
                    kv_on_gpu,
                },
                false,
            );
        }
        if gpu == GpuUse::Fit {
            let margin = u64::from(fine.fit_margin_mib.unwrap_or(DEFAULT_LEAVE_FREE_MIB))
                .saturating_mul(MIB);
            let target = placement::fit_target(self.machine.gpu_free_bytes, margin);
            let (layers, experts_on_cpu) = placement::fit_layers_where(input, target, kv_on_gpu);
            return (
                Where {
                    graphics: true,
                    layers,
                    experts_on_cpu,
                    kv_on_gpu,
                },
                true,
            );
        }
        (
            Where {
                graphics: true,
                layers: Layers::All,
                experts_on_cpu: ExpertsPick::None,
                kv_on_gpu,
            },
            false,
        )
    }

    /// The one verdict (section 4.1) for a split worked out for `gpu` and `fine`.
    pub fn judge_split(&self, split: &Split, gpu: GpuUse, fine: &FineLaunch) -> (Verdict, Fits) {
        judge(self.machine, split, gpu, fine)
    }

    /// The one verdict at `ctx` tokens with `gpu` and `fine`.
    pub fn judge(&self, ctx: u64, gpu: GpuUse, fine: &FineLaunch) -> (Verdict, Fits) {
        self.judge_split(&self.work(ctx, gpu, fine).split, gpu, fine)
    }

    /// Automatic (D2b) for a custom placement or the conversation memory in computer
    /// memory (item 6, 3.2): the largest size from the floor to the cap whose split the
    /// verdict marks Green, else Yellow.
    pub fn auto(&self, gpu: GpuUse, fine: &FineLaunch) -> Option<models::fit::AutoCtx> {
        let trained = self.facts.trained_ctx().map(u64::from);
        let floor = trained.map_or(models::fit::AUTO_FLOOR_CTX, |t| {
            t.min(models::fit::AUTO_FLOOR_CTX)
        });
        let cap = trained.map_or(models::fit::AUTO_CAP_CTX, |t| {
            t.min(models::fit::AUTO_CAP_CTX)
        });
        placement::auto_ctx_placed_in(
            |ctx| self.work(ctx, gpu, fine).split,
            self.machine.room(gpu),
            floor,
            cap,
            self.machine.memory_kind(gpu),
            is_fit(gpu, fine),
        )
    }
}

/// As much as fits decides the placement (no custom placement).
fn is_fit(gpu: GpuUse, fine: &FineLaunch) -> bool {
    gpu == GpuUse::Fit && !fine.custom_placement()
}

fn judge(machine: &Machine, split: &Split, gpu: GpuUse, fine: &FineLaunch) -> (Verdict, Fits) {
    placement::judge_in(
        split,
        machine.room(gpu),
        is_fit(gpu, fine),
        machine.memory_kind(gpu),
    )
}

fn experts_pick(experts: ExpertsOnCpu) -> ExpertsPick {
    match experts {
        ExpertsOnCpu::None | ExpertsOnCpu::FirstLayers(0) => ExpertsPick::None,
        ExpertsOnCpu::All => ExpertsPick::All,
        ExpertsOnCpu::FirstLayers(k) => ExpertsPick::FirstLayers(k),
    }
}

// ---- The picture --------------------------------------------------------------------------

/// The instant picture for the choices `resolved` (item 6, 4.2 steps 1 to 5, 7 and 8): from
/// the model's sizes when they are known (`estimate { quick }`), else from the file sizes
/// with fit.rs's numbers and the layers unknown (`estimate { notReadable }`).
pub fn estimate(
    key: String,
    facts: &ModelFacts,
    machine: &Machine,
    resolved: &ResolvedLoad,
) -> OffloadPreview {
    let gpu = resolved.gpu;
    let fine = &resolved.fine;
    match Placer::new(facts, machine, resolved.save_memory) {
        Some(placer) => {
            let worked = placer.work(u64::from(resolved.ctx), gpu, fine);
            let verdict = placer.judge_split(&worked.split, gpu, fine);
            preview(
                key,
                PictureSource::estimate(EstimateWhy::Quick),
                &worked,
                facts,
                machine,
                resolved,
                verdict,
            )
        }
        None => not_readable(key, facts, machine, resolved),
    }
}

/// The picture of `worked` with `verdict` from `source`.
fn preview(
    key: String,
    source: PictureSource,
    worked: &Worked,
    facts: &ModelFacts,
    machine: &Machine,
    resolved: &ResolvedLoad,
    (mark, fits): (Verdict, Fits),
) -> OffloadPreview {
    let gpu = resolved.gpu;
    let graphics = machine.graphics(gpu);
    let experts = facts.layers.as_ref().is_some_and(|l| l.experts);
    let (graphics_bar, computer_bar) = bars(&worked.split, machine, gpu);
    OffloadPreview {
        key,
        source,
        unified: machine.memory_kind(gpu) == MemoryKind::Unified,
        units: Some(worked.split.units),
        layers_on_graphics: worked.split.layers_on_gpu,
        layers_about: worked.about && graphics,
        expert_layers_on_processor: (graphics && experts)
            .then_some(worked.split.expert_layers_on_cpu),
        graphics: graphics_bar,
        computer: computer_bar,
        graphics_limit_bytes: graphics_limit(machine, gpu),
        mark,
        fits,
        custom: graphics && resolved.fine.custom_placement(),
    }
}

/// Apple: the most Apple graphics may use (the probe's Metal working set).
fn graphics_limit(machine: &Machine, gpu: GpuUse) -> Option<u64> {
    (machine.memory_kind(gpu) == MemoryKind::Unified && machine.gpu_total_bytes > 0)
        .then_some(machine.gpu_total_bytes)
}

/// The bars of `split` (4.2 steps 3 and 8): a graphics card's own bar beside computer
/// memory; on a Mac and on the processor one bar of computer memory holding both parts. The
/// free figures are `Machine`'s, the same as the marks use (4.1). On a Mac the loaded
/// model's memory, counted free for Apple graphics, is also counted free in the single bar.
fn bars(split: &Split, machine: &Machine, gpu: GpuUse) -> (Option<MemoryBar>, MemoryBar) {
    let kind = machine.memory_kind(gpu);
    let mut computer = Capacity {
        total_bytes: machine.ram_total_bytes,
        free_bytes: machine.ram_free_bytes,
    };
    let mut computer_credit = machine.ram_free_after_unload;
    if kind == MemoryKind::Unified && machine.gpu_freed_bytes > 0 {
        computer.free_bytes = computer.free_bytes.saturating_add(machine.gpu_freed_bytes);
        computer_credit = true;
    }
    let graphics = Capacity {
        total_bytes: machine.gpu_total_bytes,
        free_bytes: machine.gpu_free_bytes,
    };
    let (graphics_bar, computer_bar) = placement::bars(split, kind, Some(graphics), computer);
    (
        graphics_bar.map(|bar| {
            memory_bar(
                machine.gpu_description.clone(),
                bar,
                machine.gpu_freed_bytes > 0,
            )
        }),
        memory_bar(String::new(), computer_bar, computer_credit),
    )
}

fn memory_bar(name: String, bar: placement::Bar, free_after_unload: bool) -> MemoryBar {
    MemoryBar {
        name,
        total_bytes: bar.total_bytes,
        free_bytes: bar.free_bytes,
        free_after_unload,
        other_bytes: bar.other_bytes,
        need: bar.need,
    }
}

/// The picture when the model's tensor tables cannot be read (4.2 step 2): the layers
/// unknown, everything on the device that holds the model with fit.rs's numbers (the file
/// sizes, the conversation memory from the header, the working-space allowance), and
/// fit.rs's mark, as WP1b's marks. Without a header only the file sizes are known and the
/// mark is yellow ("the app cannot say").
fn not_readable(
    key: String,
    facts: &ModelFacts,
    machine: &Machine,
    resolved: &ResolvedLoad,
) -> OffloadPreview {
    let gpu = resolved.gpu;
    let graphics = machine.graphics(gpu);
    let room = machine.room(gpu);
    let ctx = u64::from(resolved.ctx);
    let (need, verdict) = match facts.fit_input(resolved.save_memory) {
        Some(input) => (
            Parts {
                model: input.weights_bytes,
                context: input.kv_bytes_per_token.saturating_mul(ctx),
                compute: input.compute_bytes(),
                image_reader: input.projector_bytes,
            },
            Some(room.verdict(input.needed_at(ctx))),
        ),
        None => (
            Parts {
                model: facts.weights_bytes,
                image_reader: facts.projector_bytes,
                ..Parts::default()
            },
            None,
        ),
    };
    let fits = match verdict {
        None => (Verdict::Yellow, Fits::Partial),
        Some(Verdict::Green) => (Verdict::Green, Fits::Fits),
        Some(Verdict::Red) => (Verdict::Red, Fits::WontFitAtAll),
        Some(Verdict::Yellow) if !graphics || is_fit(gpu, &resolved.fine) => {
            (Verdict::Yellow, Fits::Partial)
        }
        Some(Verdict::Yellow) if need.total() <= room.whole => (Verdict::Yellow, Fits::Tight),
        Some(Verdict::Yellow) => (Verdict::Yellow, Fits::WontFitGraphics),
    };
    let split = Split {
        gpu: if graphics { need } else { Parts::default() },
        cpu: if graphics { Parts::default() } else { need },
        layers_on_gpu: 0,
        units: 0,
        expert_layers_on_cpu: 0,
    };
    let (graphics_bar, computer_bar) = bars(&split, machine, gpu);
    OffloadPreview {
        key,
        source: PictureSource::estimate(EstimateWhy::NotReadable),
        unified: machine.memory_kind(gpu) == MemoryKind::Unified,
        units: None,
        layers_on_graphics: 0,
        layers_about: false,
        expert_layers_on_processor: None,
        graphics: graphics_bar,
        computer: computer_bar,
        graphics_limit_bytes: graphics_limit(machine, gpu),
        mark: fits.0,
        fits: fits.1,
        custom: graphics && resolved.fine.custom_placement(),
    }
}

// ---- llama.cpp's measurement ----------------------------------------------------------------

/// A model file as one backend build measures it: a failure or time-out is remembered per
/// file version and build until the panel is opened again (I6-14).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileVersion {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub backend: BackendKind,
    pub build: String,
}

/// One measurement as it is cached: the file version and every setting that reaches
/// fit-params' command line (4.2 step 6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasureKey {
    pub file: FileVersion,
    pub device: Option<String>,
    pub ctx: u32,
    pub save_memory: bool,
    pub flash_attn: runtime::FlashAttn,
    pub kv_in_computer_memory: bool,
    pub layers: MeasureLayers,
    pub experts: ExpertsOnCpu,
}

impl MeasureKey {
    /// The key of `cfg` on `backend`; `None` when the file is not there.
    pub fn of(backend: &Backend, cfg: &MeasureConfig) -> Option<Self> {
        let meta = std::fs::metadata(&cfg.model).ok()?;
        Some(Self {
            file: FileVersion {
                path: cfg.model.clone(),
                size: meta.len(),
                modified: meta.modified().ok(),
                backend: backend.kind,
                build: backend.build.clone(),
            },
            device: cfg.device.clone(),
            ctx: cfg.ctx,
            save_memory: cfg.save_memory,
            flash_attn: cfg.flash_attn,
            kv_in_computer_memory: cfg.kv_in_computer_memory,
            layers: cfg.layers,
            experts: cfg.experts_on_cpu,
        })
    }
}

/// What one measurement request ended with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// llama.cpp's rows, in bytes.
    Rows(Vec<MemoryRow>),
    /// No figures: the picture stays the estimate, with this reason.
    Failed(EstimateWhy),
    /// A newer request cancelled this run: its answer is stale anyway.
    Replaced,
}

type MeasureResult = Result<Vec<MemoryRow>, FitParamsError>;

/// Runs one measurement: `runtime::measure` in the app, a fake in tests.
pub type Runner = Arc<
    dyn Fn(Backend, MeasureConfig, Duration, CancellationToken) -> BoxFuture<'static, MeasureResult>
        + Send
        + Sync,
>;

/// Results kept (I6-14).
const DONE_KEPT: usize = 32;
/// Failed file versions kept.
const FAILED_KEPT: usize = 32;

struct Running {
    id: u64,
    key: MeasureKey,
    cancel: CancellationToken,
    result: Shared<BoxFuture<'static, MeasureResult>>,
}

#[derive(Default)]
struct MeasureState {
    done: VecDeque<(MeasureKey, Vec<MemoryRow>)>,
    failed: VecDeque<(FileVersion, EstimateWhy)>,
    running: Option<Running>,
    next_id: u64,
}

/// The app's one measurement at a time, and what earlier ones gave (`AppState::offload`).
pub struct Measurements {
    runner: Runner,
    timeout: Duration,
    state: StdMutex<MeasureState>,
}

impl Default for Measurements {
    fn default() -> Self {
        Self::with_runner(
            Arc::new(|backend, cfg, timeout, cancel| {
                async move { runtime::measure(&backend, &cfg, timeout, cancel).await }.boxed()
            }),
            runtime::FIT_PARAMS_TIMEOUT,
        )
    }
}

impl Measurements {
    /// Measurements run by `runner`, each killed after `timeout`.
    pub fn with_runner(runner: Runner, timeout: Duration) -> Self {
        Self {
            runner,
            timeout,
            state: StdMutex::new(MeasureState::default()),
        }
    }

    /// The settings panel was opened for the model at `path`: its failures and time-outs
    /// may be tried once more (I6-14: a file that crashes fit-params runs at most once per
    /// opening).
    pub fn forget_failures(&self, path: &Path) {
        if let Ok(mut state) = self.state.lock() {
            state.failed.retain(|(file, _)| file.path != path);
        }
    }

    /// Measures `cfg` on `backend`: a cached result, a failure remembered for this file
    /// version, the run already going for the same key (joined), or a new run that cancels
    /// the one going for another key.
    pub async fn measure(&self, backend: Backend, cfg: MeasureConfig, key: MeasureKey) -> Outcome {
        let (id, result) = {
            let Ok(mut state) = self.state.lock() else {
                return Outcome::Failed(EstimateWhy::Failed);
            };
            if let Some((_, rows)) = state.done.iter().find(|(k, _)| *k == key) {
                return Outcome::Rows(rows.clone());
            }
            if let Some((_, why)) = state.failed.iter().find(|(f, _)| *f == key.file) {
                return Outcome::Failed(*why);
            }
            match &state.running {
                Some(running) if running.key == key => (running.id, running.result.clone()),
                _ => {
                    if let Some(older) = state.running.take() {
                        older.cancel.cancel();
                    }
                    let cancel = CancellationToken::new();
                    let task =
                        tokio::spawn((self.runner)(backend, cfg, self.timeout, cancel.clone()));
                    let result =
                        async move { task.await.unwrap_or(Err(FitParamsError::Cancelled)) }
                            .boxed()
                            .shared();
                    state.next_id = state.next_id.wrapping_add(1);
                    let id = state.next_id;
                    state.running = Some(Running {
                        id,
                        key: key.clone(),
                        cancel,
                        result: result.clone(),
                    });
                    (id, result)
                }
            }
        };
        let result = result.await;
        let Ok(mut state) = self.state.lock() else {
            return Outcome::Failed(EstimateWhy::Failed);
        };
        if state.running.as_ref().is_some_and(|r| r.id == id) {
            state.running = None;
        }
        match result {
            Ok(rows) => {
                if !state.done.iter().any(|(k, _)| *k == key) {
                    if state.done.len() >= DONE_KEPT {
                        state.done.pop_front();
                    }
                    state.done.push_back((key, rows.clone()));
                }
                Outcome::Rows(rows)
            }
            Err(FitParamsError::Cancelled) => Outcome::Replaced,
            Err(error) => {
                let why = match error {
                    FitParamsError::Timeout => EstimateWhy::TimedOut,
                    FitParamsError::NoTool => EstimateWhy::NoTool,
                    _ => EstimateWhy::Failed,
                };
                if !state.failed.iter().any(|(f, _)| *f == key.file) {
                    if state.failed.len() >= FAILED_KEPT {
                        state.failed.pop_front();
                    }
                    state.failed.push_back((key.file, why));
                }
                Outcome::Failed(why)
            }
        }
    }
}

/// fit-params' settings for the placement `place` of the model at `path` (4.3): the
/// candidate's device only when a graphics device takes part; layers as placed; expert
/// parts as chosen, or for As much as fits' own result (the expert parts of the last K
/// blocks on the card on the processor) the first blocks up to the same count, `--n-cpu-moe`
/// counting the blocks already on the processor too (the same amount when the layers weigh
/// the same, hence "About").
pub fn measure_config(
    path: &str,
    resolved: &ResolvedLoad,
    place: &Where,
    units: u32,
    candidate: &runtime::Candidate,
) -> MeasureConfig {
    let graphics = place.graphics;
    let layers = match (graphics, place.layers) {
        (false, _) => MeasureLayers::Count(0),
        (true, Layers::All) => MeasureLayers::All,
        (true, Layers::Count(n)) => MeasureLayers::Count(n),
    };
    let experts_on_cpu = if graphics {
        match place.experts_on_cpu {
            ExpertsPick::None => ExpertsOnCpu::None,
            ExpertsPick::All => ExpertsOnCpu::All,
            ExpertsPick::FirstLayers(k) => ExpertsOnCpu::FirstLayers(k),
            ExpertsPick::LastLayers(k) => {
                let blocks = units.saturating_sub(1);
                let on_card = match place.layers {
                    Layers::All => units,
                    Layers::Count(n) => n.min(units),
                };
                let on_processor = if on_card == 0 {
                    blocks
                } else {
                    units - on_card
                };
                ExpertsOnCpu::FirstLayers(on_processor.saturating_add(k).min(blocks))
            }
        }
    } else {
        ExpertsOnCpu::None
    };
    MeasureConfig {
        model: PathBuf::from(path),
        ctx: resolved.ctx,
        device: if graphics {
            candidate.device.clone()
        } else {
            None
        },
        layers,
        experts_on_cpu,
        kv_in_computer_memory: graphics && !place.kv_on_gpu,
        save_memory: resolved.save_memory,
        flash_attn: resolved.fine.flash_attn,
        hip_visible_devices: if graphics {
            candidate.hip_visible_devices.clone()
        } else {
            None
        },
    }
}

/// The picture for `plan` with llama.cpp's own figures (`offload_measure`, 4.2 step 6):
/// the estimate when the file's details cannot be read; `noTool` at once when the backend a
/// load would use cannot measure; the measured rows (device rows on the graphics bar, the
/// host and buffer types on the computer bar, the projector added as in the estimate, the
/// free figures `Machine`'s); on a failure or time-out the estimate with that reason.
pub async fn measured(
    measurements: &Measurements,
    path: &str,
    plan: &Plan,
    facts: &ModelFacts,
    found: &[(Backend, ProbeResult)],
) -> OffloadPreview {
    let resolved = &plan.resolved;
    let machine = &plan.machine;
    let mut picture = estimate(draft_key(path, &plan.overrides), facts, machine, resolved);
    let Some(placer) = Placer::new(facts, machine, resolved.save_memory) else {
        return picture;
    };
    let Some(candidate) = crate::runtime_manager::planned_candidate(found, resolved) else {
        picture.source = PictureSource::Estimate {
            why: EstimateWhy::NoTool,
            backend: None,
        };
        return picture;
    };
    if !runtime::measurable(&candidate.backend) {
        picture.source = PictureSource::Estimate {
            why: EstimateWhy::NoTool,
            backend: Some(candidate.backend.kind),
        };
        return picture;
    }
    let gpu = resolved.gpu;
    let fine = &resolved.fine;
    let worked = placer.work(u64::from(resolved.ctx), gpu, fine);
    let cfg = measure_config(
        path,
        resolved,
        &worked.place,
        worked.split.units,
        &candidate,
    );
    let Some(key) = MeasureKey::of(&candidate.backend, &cfg) else {
        return picture;
    };
    match measurements
        .measure(candidate.backend.clone(), cfg, key)
        .await
    {
        Outcome::Rows(rows) => {
            let split = measured_split(&rows, &worked.split, facts, machine.graphics(gpu));
            let verdict = placer.judge_split(&split, gpu, fine);
            let worked = Worked { split, ..worked };
            preview(
                picture.key,
                PictureSource::Measured,
                &worked,
                facts,
                machine,
                resolved,
                verdict,
            )
        }
        Outcome::Failed(why) => {
            picture.source = PictureSource::Estimate {
                why,
                backend: (why == EstimateWhy::NoTool).then_some(candidate.backend.kind),
            };
            picture
        }
        Outcome::Replaced => picture,
    }
}

/// The split llama.cpp measured: device rows on the graphics side, the host and every
/// buffer type in computer memory, the projector's file size as the estimate places it
/// (llama.cpp does not measure it), the layer counts of the estimate's placement.
fn measured_split(
    rows: &[MemoryRow],
    estimate: &Split,
    facts: &ModelFacts,
    graphics: bool,
) -> Split {
    let (mut gpu, mut cpu) = rows_to_parts(rows);
    if graphics {
        gpu.image_reader = facts.projector_bytes;
    } else {
        cpu.image_reader = facts.projector_bytes;
    }
    Split {
        gpu,
        cpu,
        layers_on_gpu: estimate.layers_on_gpu,
        units: estimate.units,
        expert_layers_on_cpu: estimate.expert_layers_on_cpu,
    }
}

/// Device rows summed, and the host and buffer-type rows summed, in bytes.
fn rows_to_parts(rows: &[MemoryRow]) -> (Parts, Parts) {
    let mut gpu = Parts::default();
    let mut cpu = Parts::default();
    for row in rows {
        let parts = Parts {
            model: row.model_bytes,
            context: row.context_bytes,
            compute: row.compute_bytes,
            image_reader: 0,
        };
        match row.kind {
            MemoryRowKind::Device => gpu = gpu.plus(&parts),
            MemoryRowKind::Host | MemoryRowKind::BufferType => cpu = cpu.plus(&parts),
        }
    }
    (gpu, cpu)
}

// ---- Where the loaded model went (I6-7) -------------------------------------------------------

/// What the loaded model was launched with, as far as the ready report is explained by it.
pub struct LaunchFacts<'a> {
    /// What each part of the model weighs, when known.
    pub sizes: Option<&'a models::LayerSizes>,
    /// A graphics device took part (a graphics backend with a device, not Don't use it).
    pub graphics: bool,
    /// "Use the graphics card" as this attempt used it (As much as fits after a retry).
    pub gpu: GpuUse,
    /// Fine control as this attempt used it (no custom placement after that retry).
    pub fine: &'a FineLaunch,
    /// The probe's description of the device the model runs on.
    pub device_description: Option<String>,
    /// Apple graphics.
    pub apple: bool,
    /// The image projector loaded with it (llama.cpp's report leaves it out).
    pub projector_bytes: u64,
}

/// Where the loaded model went (section 5), from the ready line's memory `report`: device
/// rows on the graphics side, named by the probe's description of the device (else
/// "Graphics card", on a Mac "Apple graphics"), the host and buffer types in computer
/// memory, the projector's file size where it runs. The layer count is exact for All of it
/// (every unit), a chosen count and the processor (0); for As much as fits
/// `placement::explain` works it out from the model bytes on the card in llama.cpp's own
/// order ("About"), or it stays unknown and only amounts are given. Expert layers on the
/// processor are counted for a model with expert parts on a graphics device.
pub fn placement_from_report(report: &MemoryReport, launch: &LaunchFacts) -> Placement {
    let (mut gpu, mut cpu) = rows_to_parts(&report.rows);
    let on_device = report.rows.iter().any(|r| r.kind == MemoryRowKind::Device);
    if launch.graphics && on_device {
        gpu.image_reader = launch.projector_bytes;
    } else {
        cpu.image_reader = launch.projector_bytes;
    }
    let units = launch.sizes.map(models::LayerSizes::units);
    let experts = launch.sizes.is_some_and(models::LayerSizes::has_experts);
    let custom = launch.graphics && launch.fine.custom_placement();
    let about = launch.graphics && !custom && launch.gpu == GpuUse::Fit;
    let chosen: Option<(Layers, ExpertsPick)> = if !launch.graphics {
        Some((Layers::Count(0), ExpertsPick::None))
    } else if custom {
        Some((
            launch.fine.gpu_layers.map_or(Layers::All, Layers::Count),
            experts_pick(launch.fine.experts_on_cpu),
        ))
    } else if about {
        launch
            .sizes
            .and_then(|sizes| placement::explain(sizes, gpu.model))
    } else {
        Some((Layers::All, ExpertsPick::None))
    };
    let layers_on_graphics = chosen.and_then(|(layers, _)| match (layers, units) {
        (Layers::All, Some(u)) => Some(u),
        (Layers::All, None) => None,
        (Layers::Count(n), Some(u)) => Some(n.min(u)),
        (Layers::Count(n), None) => Some(n),
    });
    let expert_layers_on_processor = match (launch.sizes, chosen) {
        (Some(sizes), Some((layers, pick))) if experts && launch.graphics => {
            Some(expert_layers_moved(sizes, layers, pick))
        }
        _ => None,
    };
    let name = launch
        .device_description
        .clone()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| {
            if launch.apple {
                "Apple graphics".to_owned()
            } else {
                "Graphics card".to_owned()
            }
        });
    Placement {
        graphics: on_device.then_some(GraphicsUse { name, parts: gpu }),
        computer: cpu,
        units,
        layers_on_graphics,
        expert_layers_on_processor,
        layers_about: about,
        custom,
    }
}

/// Blocks on the card whose expert parts `pick` moves to the processor, with `layers` on
/// the card (the effective count).
fn expert_layers_moved(sizes: &models::LayerSizes, layers: Layers, pick: ExpertsPick) -> u32 {
    let input = PlacementInput {
        sizes,
        kv_per_layer: Vec::new(),
        compute_gpu: 0,
        compute_cpu: 0,
        projector: 0,
    };
    placement::split(
        &input,
        &Where {
            graphics: true,
            layers,
            experts_on_cpu: pick,
            kv_on_gpu: true,
        },
    )
    .expert_layers_on_cpu
}

/// The bytes the loaded model takes on its graphics device by the ready report: every
/// device row's weights, conversation memory and working space.
pub fn report_device_bytes(report: &MemoryReport) -> u64 {
    rows_to_parts(&report.rows).0.total()
}

/// The loaded model's memory in computer memory that unloading it gives back, by the
/// ready report: the host's conversation memory and working space, and every buffer
/// type's (repacked weights are not mapped from the file, so the system does not count
/// them as memory it can reclaim). The host's own weights are mapped from the file.
pub fn report_freed_ram(report: &MemoryReport) -> u64 {
    report.rows.iter().fold(0u64, |sum, row| match row.kind {
        MemoryRowKind::Host => sum
            .saturating_add(row.context_bytes)
            .saturating_add(row.compute_bytes),
        MemoryRowKind::BufferType => sum
            .saturating_add(row.model_bytes)
            .saturating_add(row.context_bytes)
            .saturating_add(row.compute_bytes),
        MemoryRowKind::Device => sum,
    })
}

#[cfg(test)]
mod tests;
