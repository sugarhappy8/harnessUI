//! Studio's runs on this computer, behind the same [`MediaProvider`] as the cloud companies
//! (studio-local-design.md L4 and L5; 0.1.0 Task L14).
//!
//! - **The models** ([`LocalMediaProvider::installed`]): each installed picture or video model
//!   with its catalog entry (a searched model's rebuilt from its saved parts and its family
//!   record, `local_models::entry_from_installed`), offered as a free [`MediaModel`]
//!   ([`Price::Free`]) with the controls of [`LocalMediaProvider::params_for`]. Extra fields
//!   are never offered: they would become engine flags. Listing the models never touches the
//!   network.
//! - **The exact command** ([`LocalProvider::preview_local`]): the engine's arguments as the
//!   run would pass them, with the files' paths, and the prompt beside them (the engine reads
//!   it from a file, never from its arguments); the graphics card is named among the engine's
//!   devices, which [`LocalProvider::list_devices_for`] has it list first, as a run does
//!   (waiting 5 s at most; the listing, kept once per engine even when it fails, goes on for
//!   the run).
//! - **Before the engine starts** ([`LocalProvider::start_local`]): an engine is found (looked
//!   for again while none is known or the one known is gone, so one a download installs runs
//!   the next job), every part file is there at the size it was downloaded at, the model
//!   file's own header is a model the entry runs, the prompt fits and the arguments
//!   build. Then one `sd-cli` runs in a task of its own (`runtime::sd::run_job`) with what the
//!   lane decided ([`LocalJobOptions`]): the processor only, memory saving, and llama.cpp's
//!   graphics card found among the engine's own devices by its description.
//! - **While it runs** ([`MediaProvider::check`], [`LocalProvider::status`]): its progress, a
//!   stall and the out-of-memory retry, kept in memory. It ends done and free, or failed and
//!   never charged, with its failure named.
//! - **After**: [`MediaProvider::download`] moves each result out of the run's folder;
//!   [`LocalProvider::release`] removes that folder, the prompt files with it, and forgets the
//!   run; [`MediaProvider::cancel`] stops the engine's whole process tree first.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use agent::media::{
    Canonical, Choice, Company, Cost, FailureKind, Fetched, JobDone, JobState, Layer, LocalIo,
    MediaError, MediaFailure, MediaKind, MediaModel, MediaProvider, MediaRequest, ParamKind,
    ParamSpec, Price, RawCatalog, RefRole, RequestPreview, SEED_MAX, Saved, SizeRules, Started,
};
use async_trait::async_trait;
use models::gen_catalog::{FrameRule, GenCatalog, GenEntry, GenInput, GenKind, GenRole, Quirk};
use models::gen_fit::{GenSize, SHAPES, at_shape, shape_of};
use rand::Rng;
use runtime::sd::args::{
    ArgsError, OutputFormat, PROMPT_MAX_CHARS, PartRole, Sampler, Scheduler, SdJob, build_args,
};
use runtime::sd::devices::{SdDevice, match_llama_device};
use runtime::sd::engine::Engine;
use runtime::sd::failure::SdFailure;
use runtime::sd::progress::{LocalProgress, overall};
use runtime::sd::{RunEvent as SdRunEvent, RunLimits, RunOutput, list_devices, run_job};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tauri::AppHandle;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::errors;
use super::local_models::{current_gen_catalog, entry_from_installed};

/// What a local run's remote id starts with: `local:<run id>`.
const REMOTE_PREFIX: &str = "local:";

/// The engine's name, where a cloud request shows its URL.
const ENGINE_NAME: &str = "sd-cli";

/// What a local run costs: nothing, and that is known.
const FREE: Cost = Cost {
    usd: 0.0,
    reported: true,
};

/// The keys of the controls a local model offers, named as the cloud models name theirs.
mod key {
    pub const SHAPE: &str = "aspect_ratio";
    pub const COUNT: &str = "n";
    pub const RESOLUTION: &str = "resolution";
    pub const LENGTH: &str = "duration";
    pub const START_FRAME: &str = "first_frame";
    pub const REFERENCES: &str = "input_references";
    pub const SEED: &str = "seed";
    pub const STEPS: &str = "steps";
    pub const GUIDANCE: &str = "cfg_scale";
    pub const SAMPLER: &str = "sampler";
    pub const SCHEDULER: &str = "scheduler";
    pub const SIZE: &str = "size";
    pub const NEGATIVE: &str = "negative_prompt";
    pub const MEMORY: &str = "memory_saving";
    pub const RUN_ON: &str = "run_on";
}

/// A clip's resolutions, by their shorter side.
const RESOLUTIONS: [(&str, u32); 2] = [("480p", 480), ("720p", 720)];

/// The resolution a clip starts at.
const FIRST_RESOLUTION: &str = "480p";

/// The length a clip starts at, in seconds.
const FIRST_SECONDS: u32 = 2;

/// The most pictures one run makes.
const MAX_PICTURES: u32 = 4;

/// The most reference pictures a run takes.
const MAX_REFERENCES: u32 = 4;

/// The most steps offered.
const MAX_STEPS: u32 = 50;

/// The guidance offered: from 1 to 20, by halves.
const GUIDANCE_RANGE: (f64, f64, f64) = (1.0, 20.0, 0.5);

/// The frames a second the pinned engine makes when `--fps` is not given, for a video entry
/// that states none.
const ENGINE_FPS: u32 = 24;

/// The longest side the engine takes.
const ENGINE_MAX_SIDE: u32 = 2048;

/// An exact size's sides are at most four times one another; with at least 128 × 128 pixels,
/// the shorter side is then 64 or more, the engine's least.
const MAX_SIDE_RATIO: f64 = 4.0;
const MIN_PIXELS: u64 = 128 * 128;

/// The `run_on` field's values.
const ON_THE_CARD: &str = "graphicsCard";
const ON_THE_PROCESSOR: &str = "processor";

/// The gallery and run the exact command shows before a run has them.
const PREVIEW_GALLERY: &str = "<gallery>";
const PREVIEW_RUN: &str = "<run>";

/// How long the exact command waits for the engine to list its devices (the listing itself
/// may take `runtime::sd::run::LIST_DEVICES_TIMEOUT`, 30 s). A command made before the list
/// comes leaves `--backend` out; the listing goes on, and the run uses what it finds.
const PREVIEW_DEVICES_WAIT: Duration = Duration::from_secs(5);

/// What the engine answered when asked for its devices: the devices, or `None` when it could
/// not list them.
type Listed = Option<Vec<SdDevice>>;

/// The engine last asked for its devices and what it answered, shared with a listing still
/// going ([`LocalMediaProvider::devices_of`]).
type DeviceCache = Arc<tokio::sync::Mutex<Option<(Engine, Listed)>>>;

/// The picture and video models of the signed catalog.
#[async_trait]
pub trait GenCatalogSource: Send + Sync {
    /// The catalog's models as this build's Studio engine runs them.
    async fn gen_catalog(&self) -> GenCatalog;
}

/// The app's picture and video models: the current signed catalog (`hub::current_catalog`)
/// read for the build in `runtime::sd::lock`, so a bump of the pin moves the entries' minimum
/// engine build with it.
pub struct AppGenCatalog(pub AppHandle);

#[async_trait]
impl GenCatalogSource for AppGenCatalog {
    async fn gen_catalog(&self) -> GenCatalog {
        current_gen_catalog(&self.0).await
    }
}

/// Finds the Studio engine on this computer as it is now (in the app,
/// `engine_cmd::current_engine`).
pub type DiscoverEngine = Arc<dyn Fn() -> Option<Engine> + Send + Sync>;

/// The `memory_saving` field. The manager resolves Automatic from the fit
/// (`manager::job_options`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryChoice {
    /// Memory saving when the run would not fit without it.
    Automatic,
    /// Never.
    Off,
    /// Always.
    On,
}

impl MemoryChoice {
    const ALL: [Self; 3] = [Self::Automatic, Self::Off, Self::On];

    /// The choice a request's `params` make; Automatic when they set none.
    pub fn of(params: &Map<String, Value>) -> Self {
        params
            .get(key::MEMORY)
            .and_then(Value::as_str)
            .and_then(|word| Self::ALL.into_iter().find(|c| c.word() == word))
            .unwrap_or(Self::Automatic)
    }

    fn word(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Off => "off",
            Self::On => "on",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Off => "Off",
            Self::On => "On",
        }
    }
}

/// Whether a request asks to run on the processor (the `run_on` field). Never for a clip: a
/// video model runs on the graphics card only and does not offer the field, so a processor
/// sent with one anyway is ignored.
pub fn runs_on_processor(req: &MediaRequest) -> bool {
    req.kind != MediaKind::Video
        && req.params.get(key::RUN_ON).and_then(Value::as_str) == Some(ON_THE_PROCESSOR)
}

/// What the lane decided for one run, after the lease's grant and the fit: everything the
/// engine job takes beyond the request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LocalJobOptions {
    /// Run on the processor (`--backend cpu`).
    pub processor_only: bool,
    /// Decode in tiles and keep the weights in memory until needed.
    pub memory_saving: bool,
    /// Processor threads, for a run on the processor; `None` lets the engine choose.
    pub threads: Option<u32>,
    /// llama.cpp's probed device, matched to the engine's own `--list-devices` for
    /// `--backend`; `None` lets the engine choose.
    pub device_description: Option<String>,
}

/// Where a local run is, from what the engine last told.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalRunStatus {
    /// The engine's progress, once it has shown any.
    pub progress: Option<LocalProgress>,
    /// No progress for 10 minutes: the card offers Stop and Keep waiting.
    pub stalled: bool,
    /// The first start ran out of memory, and the engine runs again with memory saving.
    pub retrying: bool,
    /// The results made, once the run is done.
    pub outputs: usize,
    /// Why the run failed, once it has.
    pub failure: Option<SdFailure>,
}

/// What the manager's local lane needs beyond [`MediaProvider`] (the manager's tests stand in
/// for it).
#[async_trait]
pub trait LocalProvider: MediaProvider {
    /// [`MediaProvider::start`] with the lane's options. `scratch` is the run's own folder
    /// (`<gallery>/.partial/<run id>`): the prompt files and the results go in it, and its name
    /// is the run's, `local:<run id>`. [`MediaProvider::start`] itself is this with
    /// [`LocalJobOptions::default`], in a folder named by a fresh id inside its `scratch`.
    async fn start_local(
        &self,
        req: &MediaRequest,
        scratch: &Path,
        opts: &LocalJobOptions,
    ) -> Result<Started, MediaError>;

    /// Has the engine list its devices when a run with `opts` would match llama.cpp's
    /// graphics card among them (a run on the card with a device description), once per
    /// engine, as [`Self::start_local`] does before it starts the engine; what it answers is
    /// kept for the runs and the commands that follow, a failure too (then neither asks it
    /// again, and both leave the card to the engine). The manager calls it before
    /// [`Self::preview_local`], so a first command names the device the run would. It waits
    /// 5 s at most: a listing still going then goes on for the run, and the command leaves
    /// `--backend` out.
    async fn list_devices_for(&self, opts: &LocalJobOptions);

    /// The command this run would use: the engine's arguments, with the files' paths, and
    /// the prompt and negative prompt beside them. The run's folder is shown as
    /// `<gallery>/.partial/<run>`; the device is matched among those the engine last listed
    /// ([`Self::list_devices_for`] lists them first).
    fn preview_local(&self, req: &MediaRequest, opts: &LocalJobOptions) -> RequestPreview;

    /// After the lane moved the outputs (or the run failed): removes `.partial/<run>` (the
    /// prompt files with it) and forgets the run. A run still going is stopped first.
    async fn release(&self, remote_id: &str);

    /// Where run `remote_id` is; `None` for a run this provider does not know (never started
    /// here, released or stopped).
    fn status(&self, remote_id: &str) -> Option<LocalRunStatus>;

    /// Installed model `model_id` with its entry.
    async fn entry_of(&self, model_id: &str) -> Option<(store::GenModel, GenEntry)>;

    /// The build of the engine found on this computer, if one is.
    fn engine_build(&self) -> Option<u32>;
}

/// The Studio engine's processes running now (runs and device listings), and whether its
/// folder is being replaced (a pack install) or removed: the two never overlap, so the folder
/// an engine runs from is never deleted under it. Cheap to clone: every clone is the same
/// count. The provider counts its processes; the engine's download and *Remove* claim the
/// folder (`engine_cmd`).
#[derive(Clone, Default)]
pub struct EngineUse(Arc<Mutex<EngineUseState>>);

#[derive(Default)]
struct EngineUseState {
    processes: usize,
    replacing: bool,
}

impl EngineUse {
    /// One engine process begins, counted until the guard is dropped; `None` while the
    /// engine's folder is being replaced or removed.
    pub fn process(&self) -> Option<EngineProcess> {
        let mut state = lock(&self.0);
        if state.replacing {
            return None;
        }
        state.processes += 1;
        Some(EngineProcess(self.clone()))
    }

    /// The engine's folder may be replaced or removed: no engine process runs, and none starts
    /// until the guard is dropped. `None` while one runs (or another claim holds it).
    pub fn replace(&self) -> Option<EngineReplace> {
        let mut state = lock(&self.0);
        if state.processes > 0 || state.replacing {
            return None;
        }
        state.replacing = true;
        Some(EngineReplace(self.clone()))
    }

    /// Whether an engine process runs now.
    #[cfg(test)]
    pub fn in_use(&self) -> bool {
        lock(&self.0).processes > 0
    }
}

/// An engine process counted by [`EngineUse::process`]; uncounted when dropped.
pub struct EngineProcess(EngineUse);

impl Drop for EngineProcess {
    fn drop(&mut self) {
        let mut state = lock(&(self.0).0);
        state.processes = state.processes.saturating_sub(1);
    }
}

/// The engine's folder claimed by [`EngineUse::replace`]; freed when dropped.
pub struct EngineReplace(EngineUse);

impl Drop for EngineReplace {
    fn drop(&mut self) {
        lock(&(self.0).0).replacing = false;
    }
}

/// Studio's runs on this computer: the installed models, and one Studio engine process per
/// run.
pub struct LocalMediaProvider {
    store: store::Store,
    catalog: Arc<dyn GenCatalogSource>,
    discover: DiscoverEngine,
    /// The engine last found or set; looked for again while there is none, or when its
    /// program is gone.
    engine: RwLock<Option<Engine>>,
    /// The installed models as last read, which the exact command (that cannot wait for the
    /// store) is built from.
    installed: RwLock<Vec<(store::GenModel, GenEntry)>>,
    /// The engine last asked for its devices, and the devices it listed (`None` when it
    /// could not list them).
    devices: DeviceCache,
    /// The runs not yet released, by remote id.
    runs: Mutex<HashMap<String, LocalRun>>,
    /// The engine processes running now; a pack install or *Remove* waits for none.
    in_use: EngineUse,
}

/// One run: its folder, what its results are, what stops it, what it has told and its task.
struct LocalRun {
    out_dir: PathBuf,
    kind: MediaKind,
    results: ResultShape,
    cancel: CancellationToken,
    state: Arc<Mutex<RunState>>,
    task: JoinHandle<()>,
}

/// What a run's results are, from its job.
#[derive(Debug, Clone, Copy)]
struct ResultShape {
    mime: &'static str,
    width: u32,
    height: u32,
    duration_ms: Option<u64>,
}

impl ResultShape {
    fn of(job: &SdJob) -> Self {
        let mime = match job.format {
            OutputFormat::Png => "image/png",
            OutputFormat::Webm => "video/webm",
            OutputFormat::Webp => "image/webp",
        };
        let duration_ms = job
            .frames
            .zip(job.fps)
            .filter(|&(_, fps)| fps > 0)
            .map(|(frames, fps)| u64::from(frames) * 1000 / u64::from(fps));
        Self {
            mime,
            width: job.width,
            height: job.height,
            duration_ms,
        }
    }
}

/// What a run has told so far, and how it ended.
#[derive(Debug, Default)]
struct RunState {
    progress: Option<LocalProgress>,
    stalled: bool,
    retrying: bool,
    ended: Option<Result<RunOutput, SdFailure>>,
}

impl RunState {
    fn heard(&mut self, event: SdRunEvent) {
        match event {
            SdRunEvent::Progress(progress) => {
                self.progress = Some(progress);
                self.stalled = false;
            }
            SdRunEvent::Stalled => self.stalled = true,
            SdRunEvent::Retrying(_) => {
                self.retrying = true;
                self.stalled = false;
            }
        }
    }
}

/// A part of an installed model as its row keeps it: the file on disk and its size.
#[derive(Deserialize)]
struct InstalledPart {
    role: GenRole,
    file: PathBuf,
    size: u64,
}

impl LocalMediaProvider {
    /// The provider over the installed models in `store`, with their entries from `catalog`,
    /// running the engine `discover` finds.
    pub fn new(
        store: store::Store,
        catalog: Arc<dyn GenCatalogSource>,
        discover: DiscoverEngine,
    ) -> Self {
        Self {
            store,
            catalog,
            discover,
            engine: RwLock::new(None),
            installed: RwLock::new(Vec::new()),
            devices: Arc::new(tokio::sync::Mutex::new(None)),
            runs: Mutex::new(HashMap::new()),
            in_use: EngineUse::default(),
        }
    }

    /// The count of the engine's processes this provider starts, which the engine's download
    /// and *Remove* claim its folder from.
    pub fn engine_use(&self) -> EngineUse {
        self.in_use.clone()
    }

    /// Sets the engine. A `None`, or an engine whose program is gone, is looked for again
    /// at the next use. The app never sets one: it finds the engine (`discover`).
    #[cfg(test)]
    pub fn set_engine(&self, engine: Option<Engine>) {
        *write(&self.engine) = engine;
    }

    /// The engine: the one known while its program is there, else the one found now.
    pub fn engine(&self) -> Option<Engine> {
        let known = read(&self.engine).clone();
        if let Some(engine) = known.filter(|e| e.program.is_file()) {
            return Some(engine);
        }
        let found = (self.discover)();
        write(&self.engine).clone_from(&found);
        found
    }

    /// Installed models with their catalog entries (a searched model's entry rebuilt from its
    /// saved parts and the family record), newest first. A model whose entry cannot be made
    /// (a family this build does not run) is left out.
    pub async fn installed(&self) -> Vec<(store::GenModel, GenEntry)> {
        let catalog = self.catalog.gen_catalog().await;
        let rows = match self.store.list_gen_models().await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!("the installed picture and video models could not be read: {e}");
                Vec::new()
            }
        };
        let installed: Vec<_> = rows
            .into_iter()
            .filter_map(|row| match entry_from_installed(&row, &catalog) {
                Ok(entry) => Some((row, entry)),
                Err(e) => {
                    tracing::debug!(model = %row.id, "an installed model is not offered: {}", e.details);
                    None
                }
            })
            .collect();
        write(&self.installed).clone_from(&installed);
        installed
    }

    /// The controls of `entry`, with the cloud models' `canonical` roles so a value carries
    /// between local and cloud models. In front: Shape; How many (pictures); Resolution and
    /// Length (video, from 480p and 2 s); the start frame and reference pictures when the
    /// entry takes them. Under Advanced: the seed, steps, guidance (only when the default is
    /// above 1, which 4-step models are not), the sampler and scheduler, an exact size, the
    /// negative prompt (with guidance above 1), Memory saving and Run on (pictures: a clip
    /// runs on the graphics card only). No Extra fields.
    pub fn params_for(entry: &GenEntry) -> Vec<ParamSpec> {
        let video = entry.kind == GenKind::Video;
        let shapes: Vec<String> = SHAPES.into_iter().map(shape_name).collect();
        let mut params = vec![ParamSpec {
            company_default: Some(shape_name(default_shape(entry)).into()),
            ..field(
                key::SHAPE,
                Some(Canonical::Shape),
                "Shape",
                Layer::Simple,
                words(shapes.iter().map(|shape| (shape.as_str(), shape.as_str()))),
            )
        }];
        if video {
            params.push(ParamSpec {
                initial: Some(FIRST_RESOLUTION.into()),
                ..field(
                    key::RESOLUTION,
                    Some(Canonical::Resolution),
                    "Resolution",
                    Layer::Simple,
                    words(RESOLUTIONS.iter().map(|&(name, _)| (name, name))),
                )
            });
            let longest = longest_seconds(entry);
            let lengths = (1..=longest)
                .map(|seconds| Choice {
                    value: seconds.into(),
                    label: format!("{seconds} s"),
                })
                .collect();
            params.push(ParamSpec {
                initial: Some(FIRST_SECONDS.min(longest).into()),
                ..field(
                    key::LENGTH,
                    Some(Canonical::Length),
                    "Length",
                    Layer::Simple,
                    ParamKind::Enum { values: lengths },
                )
            });
        } else {
            params.push(ParamSpec {
                company_default: Some(1.into()),
                ..field(
                    key::COUNT,
                    Some(Canonical::Count),
                    "How many",
                    Layer::Simple,
                    whole_range(1, MAX_PICTURES),
                )
            });
        }
        if entry.inputs.contains(&GenInput::StartFrame) {
            params.push(field(
                key::START_FRAME,
                Some(Canonical::StartFrame),
                "Start frame",
                Layer::Simple,
                ParamKind::Images {
                    min: 0,
                    max: 1,
                    role: RefRole::StartFrame,
                },
            ));
        }
        if entry.inputs.contains(&GenInput::Reference) {
            params.push(field(
                key::REFERENCES,
                Some(Canonical::References),
                "Reference pictures",
                Layer::Simple,
                ParamKind::Images {
                    min: 0,
                    max: MAX_REFERENCES,
                    role: RefRole::Reference,
                },
            ));
        }

        let result = if video { "video" } else { "picture" };
        params.push(ParamSpec {
            help: Some(format!(
                "A seed is not a saved copy; the same seed on another model or version gives a \
                 different {result}. The same seed on another graphics card, or on the \
                 processor, can give a slightly different {result}."
            )),
            ..field(
                key::SEED,
                Some(Canonical::Seed),
                "Seed",
                Layer::Advanced,
                ParamKind::Seed { max: SEED_MAX },
            )
        });
        params.push(ParamSpec {
            company_default: Some(entry.defaults.steps.into()),
            initial: Some(entry.defaults.steps.into()),
            ..field(
                key::STEPS,
                None,
                "Steps",
                Layer::Advanced,
                whole_range(1, MAX_STEPS),
            )
        });
        let guided = entry.defaults.cfg > 1.0;
        if guided {
            let (min, max, step) = GUIDANCE_RANGE;
            params.push(ParamSpec {
                company_default: Some(json!(decimal(entry.defaults.cfg))),
                ..field(
                    key::GUIDANCE,
                    None,
                    "Guidance",
                    Layer::Advanced,
                    ParamKind::Range {
                        min,
                        max,
                        step: Some(step),
                        unit: None,
                    },
                )
            });
        }
        let sampler = entry
            .defaults
            .sampler
            .as_deref()
            .and_then(Sampler::from_cli);
        params.push(ParamSpec {
            company_default: sampler.map(|s| s.as_str().into()),
            ..field(
                key::SAMPLER,
                None,
                "Sampler",
                Layer::Advanced,
                words(Sampler::ALL.iter().map(|&s| (s.as_str(), sampler_label(s)))),
            )
        });
        let scheduler = entry
            .defaults
            .scheduler
            .as_deref()
            .and_then(Scheduler::from_cli);
        params.push(ParamSpec {
            company_default: scheduler.map(|s| s.as_str().into()),
            ..field(
                key::SCHEDULER,
                None,
                "Scheduler",
                Layer::Advanced,
                words(
                    Scheduler::ALL
                        .iter()
                        .map(|&s| (s.as_str(), scheduler_label(s))),
                ),
            )
        });
        let unused = if video {
            "When set, Shape and Resolution are not used."
        } else {
            "When set, Shape is not used."
        };
        params.push(ParamSpec {
            help: Some(unused.to_owned()),
            ..field(
                key::SIZE,
                Some(Canonical::Size),
                "Exact size",
                Layer::Advanced,
                ParamKind::Size {
                    rules: size_rules(entry),
                    values: Vec::new(),
                },
            )
        });
        if guided {
            params.push(ParamSpec {
                help: Some(format!("What the {result} should not show.")),
                ..field(
                    key::NEGATIVE,
                    None,
                    "Negative prompt",
                    Layer::Advanced,
                    ParamKind::Text {
                        max_chars: u32::try_from(PROMPT_MAX_CHARS).unwrap_or(u32::MAX),
                    },
                )
            });
        }
        params.push(ParamSpec {
            company_default: Some(MemoryChoice::Automatic.word().into()),
            help: Some(
                "Slower, but needs less graphics memory. Automatic uses it when the model would \
                 not fit otherwise."
                    .to_owned(),
            ),
            ..field(
                key::MEMORY,
                None,
                "Memory saving",
                Layer::Advanced,
                words(MemoryChoice::ALL.iter().map(|c| (c.word(), c.label()))),
            )
        });
        // A clip runs on the graphics card only.
        if !video {
            params.push(ParamSpec {
                company_default: Some(ON_THE_CARD.into()),
                help: Some("The processor is much slower, and needs no graphics card.".to_owned()),
                ..field(
                    key::RUN_ON,
                    None,
                    "Run on",
                    Layer::Advanced,
                    words(
                        [
                            (ON_THE_CARD, "Graphics card"),
                            (ON_THE_PROCESSOR, "Processor"),
                        ]
                        .into_iter(),
                    ),
                )
            });
        }
        params
    }

    /// The engine job for a validated request: Shape → width and height on the entry's
    /// multiple (an exact size wins), How many → the batch, Length → frames by the family's
    /// rule ([`frames_for`]), the start frame → the init image, references → reference
    /// pictures, the entry's flow shift; `opts` → processor only (`cpu`, with its threads),
    /// memory saving and `backend` (the device the engine names llama.cpp's card by, which
    /// [`LocalProvider::start_local`] finds); quirks → environment variables. A setting the
    /// request leaves out is the entry's default; a seed it leaves out is picked here. A value
    /// the job cannot take is `BadRequest`, never charged.
    #[expect(
        clippy::too_many_arguments,
        reason = "the lane's interface: the run's entry, row, request, options, device, folder \
                  and name each come from a different step"
    )]
    pub fn job_for(
        &self,
        entry: &GenEntry,
        model: &store::GenModel,
        req: &MediaRequest,
        opts: &LocalJobOptions,
        backend: Option<String>,
        out_dir: &Path,
        run_id: &str,
    ) -> Result<SdJob, MediaError> {
        let parts = installed_parts(model)?
            .into_iter()
            .map(|part| (part_role(part.role), part.file))
            .collect();
        let negative = text_param(req, key::NEGATIVE)?
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned);
        if negative
            .as_ref()
            .is_some_and(|text| text.chars().count() > PROMPT_MAX_CHARS)
        {
            return Err(refused(
                FailureKind::BadRequest,
                errors::LOCAL_NEGATIVE_TOO_LONG,
            ));
        }
        let shape = run_shape(entry, req)?;
        let seed = match req.params.get(key::SEED) {
            Some(value) => value
                .as_u64()
                .filter(|&seed| seed <= SEED_MAX)
                .ok_or_else(|| bad_value(key::SEED))?,
            None => rand::rng().random_range(0..=SEED_MAX),
        };
        let (width, height) = (shape.size.width, shape.size.height);
        let (frames, fps) = if entry.kind == GenKind::Video {
            (Some(shape.size.frames), Some(fps_of(entry)))
        } else {
            (None, None)
        };
        let added = |role: RefRole| {
            req.refs
                .iter()
                .filter(move |r| r.role == role)
                .map(|r| r.path.clone())
        };
        let takes = |input: GenInput| entry.inputs.contains(&input);
        let format = match entry.kind {
            GenKind::Video
                if cfg!(target_os = "macos") && entry.quirks.contains(&Quirk::VideoWebpOnMac) =>
            {
                OutputFormat::Webp
            }
            GenKind::Video => OutputFormat::Webm,
            GenKind::Image | GenKind::ImageEdit => OutputFormat::Png,
        };
        Ok(SdJob {
            parts,
            prompt: req.prompt.clone(),
            negative,
            width,
            height,
            size_multiple: entry.limits.multiple_of,
            steps: shape.steps,
            cfg: number_param(req, key::GUIDANCE)?.map_or(entry.defaults.cfg, |x| x as f32),
            guidance: None,
            img_cfg: None,
            flow_shift: entry.defaults.flow_shift,
            timestep_shift: None,
            sampler: word_param(req, key::SAMPLER, Sampler::from_cli)?.or_else(|| {
                entry
                    .defaults
                    .sampler
                    .as_deref()
                    .and_then(Sampler::from_cli)
            }),
            scheduler: word_param(req, key::SCHEDULER, Scheduler::from_cli)?.or_else(|| {
                entry
                    .defaults
                    .scheduler
                    .as_deref()
                    .and_then(Scheduler::from_cli)
            }),
            sigmas: Vec::new(),
            high_noise: None,
            moe_boundary: None,
            strength: None,
            seed,
            batch: shape.pictures,
            frames,
            fps,
            init_image: takes(GenInput::StartFrame)
                .then(|| added(RefRole::StartFrame).next())
                .flatten(),
            end_image: None,
            mask_image: None,
            control_image: None,
            ref_images: if takes(GenInput::Reference) {
                added(RefRole::Reference).collect()
            } else {
                Vec::new()
            },
            vae_format: None,
            model_args: None,
            lora_dir: None,
            loras: Vec::new(),
            backend,
            // 0.1.x's memory saving, both halves; Task S11 splits them.
            vae_tiling: opts.memory_saving,
            offload_to_cpu: opts.memory_saving,
            tiling_allowed: true,
            processor_only: opts.processor_only,
            threads: opts.threads.filter(|_| opts.processor_only),
            out_dir: out_dir.to_path_buf(),
            out_stem: run_id.to_owned(),
            format,
            env: quirk_env(entry),
        })
    }

    /// The devices `engine` lists, listed once per engine and kept; `None` when it cannot
    /// list them, which is kept too, so it is not asked again (the runs leave the card to
    /// it). The listing runs in a task of its own that holds the list until it is kept, so a
    /// caller that stops waiting ([`LocalProvider::list_devices_for`]) leaves it to finish
    /// for the run, and no second listing starts meanwhile.
    async fn devices_of(&self, engine: &Engine) -> Option<Vec<SdDevice>> {
        let cache = Arc::clone(&self.devices);
        let engine = engine.clone();
        let in_use = self.in_use.clone();
        let listing = tokio::spawn(async move {
            let mut listed = cache.lock_owned().await;
            if listed.as_ref().is_none_or(|(by, _)| *by != engine) {
                // The engine's folder is being replaced or removed: nothing is listed, and it
                // is asked again next time.
                let process = in_use.process()?;
                let devices = list_devices(&engine)
                    .await
                    .inspect_err(|failure| {
                        tracing::warn!(
                            ?failure,
                            "the Studio engine did not list its devices; it picks its own"
                        );
                    })
                    .ok();
                drop(process);
                *listed = Some((engine, devices));
            }
            listed.as_ref().and_then(|(_, devices)| devices.clone())
        });
        listing.await.ok().flatten()
    }

    /// The `--backend` word for llama.cpp's device described as `description`, among the
    /// devices `engine` lists ([`Self::devices_of`]); `None` (the engine's own choice) when
    /// none matches or the engine cannot list them.
    async fn backend_for(&self, engine: &Engine, description: &str) -> Option<String> {
        self.devices_of(engine)
            .await
            .and_then(|devices| backend_from(&devices, description))
    }

    /// [`Self::backend_for`] from the devices the engine last listed, without listing them
    /// or waiting for a listing still going.
    fn listed_backend(&self, description: &str) -> Option<String> {
        let engine = self.engine()?;
        let listed = self.devices.try_lock().ok()?;
        let (by, devices) = listed.as_ref()?;
        if *by != engine {
            return None;
        }
        backend_from(devices.as_deref()?, description)
    }

    /// The command `req` would run, from the installed models as last read: its arguments,
    /// and its negative prompt.
    fn command(
        &self,
        req: &MediaRequest,
        opts: &LocalJobOptions,
    ) -> Result<(Vec<String>, Option<String>), MediaError> {
        let found = read(&self.installed)
            .iter()
            .find(|(row, _)| row.id == req.model)
            .cloned();
        let (model, entry) =
            found.ok_or_else(|| refused(FailureKind::NotFound, errors::LOCAL_NOT_INSTALLED))?;
        let backend = matched_device(opts).and_then(|d| self.listed_backend(d));
        let out_dir = Path::new(PREVIEW_GALLERY)
            .join(".partial")
            .join(PREVIEW_RUN);
        let job = self.job_for(&entry, &model, req, opts, backend, &out_dir, PREVIEW_RUN)?;
        let args = build_args(&job).map_err(|e| args_refused(&e))?;
        let args = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        Ok((args, job.negative))
    }

    /// Starts `job` in a task of its own, as run `remote_id`.
    fn spawn_run(
        &self,
        engine: Engine,
        job: SdJob,
        remote_id: &str,
        kind: MediaKind,
    ) -> Result<(), MediaError> {
        let mut runs = lock(&self.runs);
        if runs.contains_key(remote_id) {
            return Err(refused(
                FailureKind::BadRequest,
                errors::LOCAL_ALREADY_RUNNING,
            ));
        }
        // Counted until the engine's process has ended.
        let process = self.in_use.process().ok_or_else(|| {
            refused(
                FailureKind::EngineMissing,
                errors::LOCAL_ENGINE_BEING_REPLACED,
            )
        })?;
        let state = Arc::new(Mutex::new(RunState::default()));
        let cancel = CancellationToken::new();
        let run = LocalRun {
            out_dir: job.out_dir.clone(),
            kind,
            results: ResultShape::of(&job),
            cancel: cancel.clone(),
            state: Arc::clone(&state),
            task: tokio::spawn(run(engine, job, cancel, state, process)),
        };
        runs.insert(remote_id.to_owned(), run);
        Ok(())
    }

    /// Stops run `remote_id` if it still goes, waits for it, removes its folder and forgets
    /// it. A run not known is nothing to do.
    async fn forget(&self, remote_id: &str) {
        let run = lock(&self.runs).remove(remote_id);
        let Some(run) = run else {
            return;
        };
        run.cancel.cancel();
        if let Err(e) = run.task.await {
            tracing::warn!(run = remote_id, "a local Studio run's task failed: {e}");
        }
        match tokio::fs::remove_dir_all(&run.out_dir).await {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(run = remote_id, "a local Studio run's folder stayed: {e}"),
        }
    }
}

/// One run's task: the engine job, with what it tells kept in `state`, counted as an engine
/// process (`process`) until the job has ended.
async fn run(
    engine: Engine,
    job: SdJob,
    cancel: CancellationToken,
    state: Arc<Mutex<RunState>>,
    process: EngineProcess,
) {
    let on_event = |event: SdRunEvent| lock(&state).heard(event);
    let ended = run_job(&engine, &job, RunLimits::default(), cancel, &on_event).await;
    drop(process);
    lock(&state).ended = Some(ended);
}

#[async_trait]
impl MediaProvider for LocalMediaProvider {
    fn company(&self) -> Company {
        Company::Local
    }

    fn host(&self) -> String {
        String::new()
    }

    /// The installed models as [`MediaModel`]s, all in `images` (a video model is one of
    /// kind video). Never the network.
    async fn fetch_catalog(&self) -> Result<RawCatalog, MediaError> {
        let models: Vec<MediaModel> = self
            .installed()
            .await
            .iter()
            .map(|(row, entry)| media_model(row, entry))
            .collect();
        let images = serde_json::to_value(&models)
            .map_err(|e| MediaError::Local(LocalIo::Io(e.to_string())))?;
        Ok(RawCatalog {
            images,
            videos: None,
        })
    }

    async fn fetch_image_price(&self, _model: &str) -> Result<Value, MediaError> {
        Ok(Value::Null)
    }

    fn preview(&self, req: &MediaRequest) -> RequestPreview {
        self.preview_local(req, &LocalJobOptions::default())
    }

    async fn start(&self, req: &MediaRequest, scratch: &Path) -> Result<Started, MediaError> {
        let folder = scratch.join(uuid::Uuid::new_v4().to_string());
        self.start_local(req, &folder, &LocalJobOptions::default())
            .await
    }

    /// Waiting for the engine's first progress, at work (how far along overall), done and
    /// free, or failed and not charged. A run stopped or released is gone: the manager's own
    /// stop path words a stopped run.
    async fn check(&self, remote_id: &str) -> Result<JobState, MediaError> {
        let runs = lock(&self.runs);
        let Some(run) = runs.get(remote_id) else {
            return Ok(JobState::Gone);
        };
        let state = lock(&run.state);
        Ok(match &state.ended {
            Some(Ok(output)) => JobState::Done(JobDone {
                outputs: output.files.len(),
                signed: Vec::new(),
                cost: Some(FREE),
            }),
            Some(Err(SdFailure::Cancelled)) => JobState::Gone,
            Some(Err(f)) => JobState::Failed(failure(
                failure_kind(f),
                errors::engine_failure_details(f, run.kind),
            )),
            None => match &state.progress {
                Some(progress) => JobState::Running {
                    progress: Some(overall(progress)),
                },
                None => JobState::Pending,
            },
        })
    }

    /// Moves result `index` of a done run out of its folder to `dest`.
    async fn download(
        &self,
        remote_id: &str,
        _job: &JobDone,
        index: usize,
        dest: &Path,
    ) -> Result<Fetched, MediaError> {
        let found = {
            let runs = lock(&self.runs);
            runs.get(remote_id).and_then(|run| {
                let state = lock(&run.state);
                match &state.ended {
                    Some(Ok(output)) => output
                        .files
                        .get(index)
                        .map(|file| (file.clone(), run.results)),
                    _ => None,
                }
            })
        };
        let (file, results) =
            found.ok_or_else(|| refused(FailureKind::NotFound, errors::LOCAL_NO_SUCH_RESULT))?;
        if let Some(folder) = dest.parent() {
            tokio::fs::create_dir_all(folder)
                .await
                .map_err(|e| local_io(&e))?;
        }
        tokio::fs::rename(&file, dest)
            .await
            .map_err(|e| local_io(&e))?;
        let size_bytes = tokio::fs::metadata(dest)
            .await
            .map_err(|e| local_io(&e))?
            .len();
        Ok(Fetched::Saved(Saved {
            path: dest.to_path_buf(),
            mime: results.mime.to_owned(),
            size_bytes,
            width: Some(results.width),
            height: Some(results.height),
            duration_ms: results.duration_ms,
        }))
    }

    /// Kills the engine's whole tree, waits for it and removes the run's folder; the run is
    /// then gone.
    async fn cancel(&self, remote_id: &str) -> Result<(), MediaError> {
        self.forget(remote_id).await;
        Ok(())
    }
}

#[async_trait]
impl LocalProvider for LocalMediaProvider {
    async fn start_local(
        &self,
        req: &MediaRequest,
        scratch: &Path,
        opts: &LocalJobOptions,
    ) -> Result<Started, MediaError> {
        let run_id = scratch
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| args_refused(&ArgsError::BadOutputStem))?;
        let engine = self
            .engine()
            .ok_or_else(|| refused(FailureKind::EngineMissing, errors::LOCAL_ENGINE_NOT_FOUND))?;
        if req.prompt.chars().count() > PROMPT_MAX_CHARS {
            return Err(refused(
                FailureKind::BadRequest,
                errors::LOCAL_PROMPT_TOO_LONG,
            ));
        }
        let (model, entry) = self
            .entry_of(&req.model)
            .await
            .ok_or_else(|| refused(FailureKind::NotFound, errors::LOCAL_NOT_INSTALLED))?;
        if media_kind(entry.kind) != req.kind {
            return Err(refused(FailureKind::BadRequest, errors::LOCAL_WRONG_KIND));
        }
        check_files(&entry, &model).await?;
        let backend = match matched_device(opts) {
            Some(description) => self.backend_for(&engine, description).await,
            None => None,
        };
        let job = self.job_for(&entry, &model, req, opts, backend, scratch, run_id)?;
        build_args(&job).map_err(|e| args_refused(&e))?;
        let remote_id = format!("{REMOTE_PREFIX}{run_id}");
        self.spawn_run(engine, job, &remote_id, req.kind)?;
        Ok(Started::Job { remote_id })
    }

    async fn list_devices_for(&self, opts: &LocalJobOptions) {
        if matched_device(opts).is_none() {
            return;
        }
        let Some(engine) = self.engine() else {
            return;
        };
        let listing = tokio::time::timeout(PREVIEW_DEVICES_WAIT, self.devices_of(&engine));
        if listing.await.is_err() {
            tracing::info!(
                seconds = PREVIEW_DEVICES_WAIT.as_secs(),
                "the Studio engine is still listing its devices; the command leaves the card to it"
            );
        }
    }

    fn preview_local(&self, req: &MediaRequest, opts: &LocalJobOptions) -> RequestPreview {
        let body = match self.command(req, opts) {
            Ok((argv, negative)) => json!({
                "argv": argv,
                "prompt": req.prompt,
                "negativePrompt": negative,
            }),
            Err(e) => json!({
                "argv": [],
                "prompt": req.prompt,
                "negativePrompt": Value::Null,
                "error": e.to_string(),
            }),
        };
        RequestPreview {
            method: "run".to_owned(),
            url: ENGINE_NAME.to_owned(),
            host: String::new(),
            body,
        }
    }

    async fn release(&self, remote_id: &str) {
        self.forget(remote_id).await;
    }

    fn status(&self, remote_id: &str) -> Option<LocalRunStatus> {
        let runs = lock(&self.runs);
        let run = runs.get(remote_id)?;
        let state = lock(&run.state);
        let (outputs, failure) = match &state.ended {
            Some(Ok(output)) => (output.files.len(), None),
            Some(Err(f)) => (0, Some(f.clone())),
            None => (0, None),
        };
        Some(LocalRunStatus {
            progress: state.progress.clone(),
            stalled: state.stalled,
            retrying: state.retrying,
            outputs,
            failure,
        })
    }

    async fn entry_of(&self, model_id: &str) -> Option<(store::GenModel, GenEntry)> {
        self.installed()
            .await
            .into_iter()
            .find(|(row, _)| row.id == model_id)
    }

    fn engine_build(&self) -> Option<u32> {
        self.engine().map(|engine| engine.build)
    }
}

/// What a run's engine job makes: its size (frames 1 for a picture), its steps and how many
/// pictures (1 for a clip). Its fit and its time are judged at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunShape {
    /// Width and height, and a clip's frames.
    pub size: GenSize,
    /// Steps of each picture.
    pub steps: u32,
    /// The batch: How many for pictures, 1 for a clip.
    pub pictures: u32,
}

/// The shape of a validated request's engine job, as [`LocalMediaProvider::job_for`] builds
/// it: Shape (or an exact size) → width and height, Length → frames by the family's rule
/// ([`frames_for`]), How many → the pictures, Steps; a setting left out is the entry's
/// default. A value the job cannot take is `BadRequest`, never charged.
pub fn run_shape(entry: &GenEntry, req: &MediaRequest) -> Result<RunShape, MediaError> {
    let (width, height) = size_of(entry, req)?;
    let (pictures, frames) = if entry.kind == GenKind::Video {
        let frames = match whole_param(req, key::LENGTH)? {
            Some(seconds) => frames_for(entry, seconds),
            None => default_frames(entry),
        };
        (1, frames)
    } else {
        (whole_param(req, key::COUNT)?.unwrap_or(1), 1)
    };
    Ok(RunShape {
        size: GenSize {
            width,
            height,
            frames,
        },
        steps: whole_param(req, key::STEPS)?.unwrap_or(entry.defaults.steps),
        pictures,
    })
}

/// The frames of a clip whose Length is left out: the entry's default, else 2 s by its
/// family's rule.
pub fn default_frames(entry: &GenEntry) -> u32 {
    entry
        .defaults
        .frames
        .unwrap_or_else(|| frames_for(entry, FIRST_SECONDS))
}

/// Frames for a clip of `seconds`: seconds × the entry's frames a second, made one more than a
/// multiple of 4 (or 8) as its family's rule asks (2 s at 24 fps: 49), and never more than
/// the entry's most (Wan 2.2: 121).
pub fn frames_for(entry: &GenEntry, seconds: u32) -> u32 {
    let step = match entry.limits.frame_rule {
        Some(FrameRule::FourNPlusOne) => 4,
        Some(FrameRule::EightNPlusOne) => 8,
        None => 1,
    };
    let on_rule = |frames: u64| {
        if step == 1 {
            frames.max(1)
        } else {
            frames / step * step + 1
        }
    };
    let frames = on_rule(u64::from(seconds) * u64::from(fps_of(entry)));
    // The largest count on the rule within the entry's most.
    let most = entry.limits.max_frames.map_or(u64::MAX, |most| {
        let most = u64::from(most).max(1);
        if step == 1 { most } else { on_rule(most - 1) }
    });
    u32::try_from(frames.min(most)).unwrap_or(u32::MAX)
}

/// The entry's frames a second; the engine's own when it states none.
fn fps_of(entry: &GenEntry) -> u32 {
    entry
        .defaults
        .fps
        .filter(|&fps| fps > 0)
        .unwrap_or(ENGINE_FPS)
}

/// The longest clip offered, in whole seconds: the entry's most frames at its frames a second
/// (at least 1; the starting length when it states no most).
fn longest_seconds(entry: &GenEntry) -> u32 {
    entry
        .limits
        .max_frames
        .map_or(FIRST_SECONDS, |frames| frames / fps_of(entry))
        .max(1)
}

/// The longest side `entry` is made at here: its own longest, within the engine's.
pub fn longest_side(entry: &GenEntry) -> u32 {
    size_rules(entry).max_side
}

/// The rules of an exact size for `entry`: its multiple, its longest side (within the
/// engine's), sides at most four times one another, and at least 128 × 128 pixels.
fn size_rules(entry: &GenEntry) -> SizeRules {
    let max_side = match entry.limits.max_side {
        0 => ENGINE_MAX_SIDE,
        side => side.min(ENGINE_MAX_SIDE),
    };
    SizeRules {
        multiple_of: entry.limits.multiple_of,
        min_ratio: 1.0 / MAX_SIDE_RATIO,
        max_ratio: MAX_SIDE_RATIO,
        max_side,
        min_pixels: MIN_PIXELS,
        max_pixels: u64::from(max_side) * u64::from(max_side),
    }
}

/// The width and height `req` asks of `entry`: an exact size when one is set; else the entry's
/// default size when neither Shape nor Resolution is set; else the shape (the default size's
/// own when not set) at a shorter side of the resolution's lines for a clip (the default's
/// shorter side when not set), or for a picture keeping about the default size's pixels.
/// The webview works out a clip's size the same way (`localClipSize`); the test
/// `size_of_matches_the_webviews_local_clip_size` holds the two to the same cases.
fn size_of(entry: &GenEntry, req: &MediaRequest) -> Result<(u32, u32), MediaError> {
    if let Some(size) = text_param(req, key::SIZE)? {
        return parse_size(size).ok_or_else(|| bad_value(key::SIZE));
    }
    let (width, height) = (entry.defaults.width, entry.defaults.height);
    let shape = word_param(req, key::SHAPE, |word| {
        SHAPES.into_iter().find(|&shape| shape_name(shape) == word)
    })?;
    let video = entry.kind == GenKind::Video;
    let lines = if video {
        word_param(req, key::RESOLUTION, |word| {
            RESOLUTIONS
                .iter()
                .find(|(name, _)| *name == word)
                .map(|&(_, lines)| lines)
        })?
    } else {
        None
    };
    if shape.is_none() && lines.is_none() {
        return Ok((width, height));
    }
    let shape = shape.unwrap_or_else(|| default_shape(entry));
    let shorter = match lines {
        Some(lines) => lines,
        None if video => width.min(height),
        None => {
            let (lo, hi) = (shape.0.min(shape.1), shape.0.max(shape.1));
            let pixels = f64::from(width) * f64::from(height);
            // A side of a few thousand pixels: `as` cannot lose anything here.
            (pixels * f64::from(lo) / f64::from(hi)).sqrt() as u32
        }
    };
    // The longer side kept within the entry's longest, each side on its multiple (480p at
    // 16:9 on a multiple of 32 is 832 × 480, 720p 1280 × 704): the fit's own helper.
    let max_side = Some(size_rules(entry).max_side);
    at_shape(shorter, shape, entry.limits.multiple_of, max_side).ok_or_else(|| bad_value(key::SIZE))
}

/// The offered shape nearest the entry's default size (`gen_fit::shape_of`).
fn default_shape(entry: &GenEntry) -> (u32, u32) {
    shape_of(GenSize {
        width: entry.defaults.width,
        height: entry.defaults.height,
        frames: 1,
    })
}

/// A shape's name, as the cloud models write it (`16:9`).
fn shape_name((w, h): (u32, u32)) -> String {
    format!("{w}:{h}")
}

/// A `WxH` size of two whole numbers above 0.
fn parse_size(size: &str) -> Option<(u32, u32)> {
    let side = |text: &str| {
        (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse::<u32>().ok())
            .flatten()
            .filter(|&n| n > 0)
    };
    let (width, height) = size.split_once('x')?;
    Some((side(width)?, side(height)?))
}

/// The engine role of a part: one to one.
fn part_role(role: GenRole) -> PartRole {
    match role {
        GenRole::Diffusion => PartRole::Diffusion,
        GenRole::HighNoiseDiffusion => PartRole::HighNoiseDiffusion,
        GenRole::UncondDiffusion => PartRole::UncondDiffusion,
        GenRole::Llm => PartRole::Llm,
        GenRole::LlmVision => PartRole::LlmVision,
        GenRole::T5xxl => PartRole::T5xxl,
        GenRole::ClipL => PartRole::ClipL,
        GenRole::ClipG => PartRole::ClipG,
        GenRole::ClipVision => PartRole::ClipVision,
        GenRole::Tokenizer => PartRole::Tokenizer,
        GenRole::Vae => PartRole::Vae,
        GenRole::Tae => PartRole::Tae,
        GenRole::AudioVae => PartRole::AudioVae,
        GenRole::AudioEncoder => PartRole::AudioEncoder,
        GenRole::EmbeddingsConnectors => PartRole::EmbeddingsConnectors,
        GenRole::Model => PartRole::Single,
    }
}

/// The variables the entry's quirks add, from the compiled-in list only:
/// `GGML_METAL_TENSOR_DISABLE=1` on macOS for `metal-tensor-off`.
fn quirk_env(entry: &GenEntry) -> Vec<(String, String)> {
    entry
        .quirks
        .iter()
        .filter_map(|quirk| match quirk {
            Quirk::MetalTensorOff if cfg!(target_os = "macos") => {
                Some(("GGML_METAL_TENSOR_DISABLE".to_owned(), "1".to_owned()))
            }
            Quirk::MetalTensorOff | Quirk::VideoWebpOnMac => None,
        })
        .collect()
}

/// The parts of an installed model, as its row keeps them.
fn installed_parts(model: &store::GenModel) -> Result<Vec<InstalledPart>, MediaError> {
    Vec::<InstalledPart>::deserialize(&model.parts).map_err(|e| {
        refused(
            FailureKind::ModelFileBad,
            format!("{} ({e})", errors::LOCAL_PART_MISSING),
        )
    })
}

/// Every part file is there at the size it was downloaded at, and the model file's own header
/// is a model the entry runs (`downloads::check_main_file`, read off the async runtime).
/// Otherwise `ModelFileBad`: download it again.
async fn check_files(entry: &GenEntry, model: &store::GenModel) -> Result<(), MediaError> {
    let parts = installed_parts(model)?;
    for part in &parts {
        match tokio::fs::metadata(&part.file).await {
            Ok(meta) if meta.is_file() && meta.len() == part.size => {}
            Ok(_) => {
                return Err(refused(
                    FailureKind::ModelFileBad,
                    errors::LOCAL_PART_CHANGED,
                ));
            }
            Err(_) => {
                return Err(refused(
                    FailureKind::ModelFileBad,
                    errors::LOCAL_PART_MISSING,
                ));
            }
        }
    }
    let diffusion = parts
        .iter()
        .find(|p| matches!(p.role, GenRole::Diffusion | GenRole::Model))
        .ok_or_else(|| refused(FailureKind::ModelFileBad, errors::LOCAL_PART_MISSING))?;
    let (entry, path) = (entry.clone(), diffusion.file.clone());
    let checked =
        tokio::task::spawn_blocking(move || crate::downloads::check_main_file(&entry, &path)).await;
    let details = match checked {
        Ok(Ok(())) => return Ok(()),
        Ok(Err(e)) => e.details,
        Err(e) => e.to_string(),
    };
    Err(refused(
        FailureKind::ModelFileBad,
        format!("{}: {details}.", errors::LOCAL_NOT_ITS_MODEL),
    ))
}

/// The installed model `row` with its `entry`, as Studio lists it: free, with local controls.
pub(crate) fn media_model(row: &store::GenModel, entry: &GenEntry) -> MediaModel {
    let params = LocalMediaProvider::params_for(entry);
    MediaModel {
        id: row.id.clone(),
        display_name: entry.name.clone(),
        family: row.family.clone(),
        kind: media_kind(entry.kind),
        accepts_images: params
            .iter()
            .any(|p| matches!(p.kind, ParamKind::Images { max, .. } if max > 0)),
        params,
        price: Price::Free,
        notes: Vec::new(),
        usable: None,
        created: Some(row.added_at / 1000),
        companies: Vec::new(),
    }
}

/// What a model of `kind` makes, as a media request names it.
pub(crate) fn media_kind(kind: GenKind) -> MediaKind {
    match kind {
        GenKind::Video => MediaKind::Video,
        GenKind::Image | GenKind::ImageEdit => MediaKind::Image,
    }
}

/// The kind of failure an engine failure is. A stop is never reported as a failure (`check`
/// says a stopped run is gone); it is named here only so every failure has a kind.
fn failure_kind(f: &SdFailure) -> FailureKind {
    match f {
        SdFailure::OutOfMemory => FailureKind::OutOfMemory,
        SdFailure::DeviceLost => FailureKind::DeviceLost,
        SdFailure::BadModel => FailureKind::ModelFileBad,
        SdFailure::EngineMissing | SdFailure::EngineBlocked(_) | SdFailure::MissingLibrary(_) => {
            FailureKind::EngineMissing
        }
        SdFailure::Crashed { .. } | SdFailure::Io(_) | SdFailure::Cancelled => {
            FailureKind::EngineFailed
        }
    }
}

/// llama.cpp's device description without the memory figures its own listing ends a row
/// with (`AMD Radeon RX 7600 (RADV NAVI33) (8176 MiB, 7900 MiB free)`), which the engine's
/// listing does not print.
fn bare_description(description: &str) -> &str {
    let text = description.trim();
    let Some(inside) = text.strip_suffix(')') else {
        return text;
    };
    let Some(open) = inside.rfind('(') else {
        return text;
    };
    let memory = inside[open + 1..]
        .split_once(',')
        .is_some_and(|(total, free)| {
            total.trim().ends_with("MiB") && free.trim().ends_with("MiB free")
        });
    if memory {
        text[..open].trim_end()
    } else {
        text
    }
}

/// The `--backend` word for llama.cpp's device described as `description` among the engine's
/// `devices` (`devices::match_llama_device`, on the bare description).
fn backend_from(devices: &[SdDevice], description: &str) -> Option<String> {
    match_llama_device(devices, bare_description(description))
}

/// llama.cpp's device a run with `opts` is matched to among the engine's devices: none for a
/// run on the processor (`cpu`) or without a description (the engine's own choice).
fn matched_device(opts: &LocalJobOptions) -> Option<&str> {
    opts.device_description
        .as_deref()
        .filter(|_| !opts.processor_only)
}

/// A field with no help, default, starting value, price effect or requirement.
fn field(
    key: &str,
    canonical: Option<Canonical>,
    label: &str,
    layer: Layer,
    kind: ParamKind,
) -> ParamSpec {
    ParamSpec {
        key: key.to_owned(),
        canonical,
        label: label.to_owned(),
        help: None,
        layer,
        kind,
        company_default: None,
        initial: None,
        affects_price: false,
        requires: None,
    }
}

/// A list of words, each with its name on screen.
fn words<'a>(choices: impl Iterator<Item = (&'a str, &'a str)>) -> ParamKind {
    ParamKind::Enum {
        values: choices
            .map(|(value, label)| Choice {
                value: value.into(),
                label: label.to_owned(),
            })
            .collect(),
    }
}

/// Whole numbers from `min` to `max`.
fn whole_range(min: u32, max: u32) -> ParamKind {
    ParamKind::Range {
        min: f64::from(min),
        max: f64::from(max),
        step: Some(1.0),
        unit: None,
    }
}

/// A guidance scale as the catalog writes it (`3.5`), not as its `f32` widens (`6.1` would
/// read 6.099999904632568).
fn decimal(x: f32) -> f64 {
    x.to_string().parse().unwrap_or(f64::from(x))
}

fn sampler_label(sampler: Sampler) -> &'static str {
    match sampler {
        Sampler::Euler => "Euler",
        Sampler::EulerA => "Euler a",
        Sampler::Heun => "Heun",
        Sampler::Dpm2 => "DPM2",
        Sampler::DpmPp2sA => "DPM++ 2S a",
        Sampler::DpmPp2m => "DPM++ 2M",
        Sampler::DpmPp2mV2 => "DPM++ 2M v2",
        Sampler::Ipndm => "iPNDM",
        Sampler::IpndmV => "iPNDM v",
        Sampler::Lcm => "LCM",
        Sampler::DdimTrailing => "DDIM trailing",
        Sampler::Tcd => "TCD",
        Sampler::ResMultistep => "RES multistep",
        Sampler::Res2s => "RES 2S",
        Sampler::ErSde => "ER-SDE",
        Sampler::EulerCfgPp => "Euler CFG++",
        Sampler::EulerACfgPp => "Euler a CFG++",
        Sampler::EulerGe => "Euler GE",
        Sampler::DpmPp2mSde => "DPM++ 2M SDE",
        Sampler::DpmPp2mSdeBt => "DPM++ 2M SDE BT",
        Sampler::Lms => "LMS",
    }
}

fn scheduler_label(scheduler: Scheduler) -> &'static str {
    match scheduler {
        Scheduler::Discrete => "Discrete",
        Scheduler::Karras => "Karras",
        Scheduler::Exponential => "Exponential",
        Scheduler::Ays => "AYS",
        Scheduler::Gits => "GITS",
        Scheduler::SgmUniform => "SGM uniform",
        Scheduler::Simple => "Simple",
        Scheduler::Smoothstep => "Smoothstep",
        Scheduler::KlOptimal => "KL optimal",
        Scheduler::Lcm => "LCM",
        Scheduler::BongTangent => "Bong tangent",
        Scheduler::Ltx2 => "LTX-2",
        Scheduler::LogitNormal => "Logit normal",
        Scheduler::Flux2 => "FLUX.2",
        Scheduler::Flux => "FLUX",
        Scheduler::Beta => "Beta",
        Scheduler::LladaImage => "LLaDA-Image",
    }
}

/// The whole number `req` sets for `key`; refused when it is not one a `u32` holds.
fn whole_param(req: &MediaRequest, key: &str) -> Result<Option<u32>, MediaError> {
    req.params
        .get(key)
        .map(|value| {
            let whole = value.as_u64().or_else(|| {
                value
                    .as_f64()
                    .filter(|x| x.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(x))
                    // A whole number within u32: `as` is exact.
                    .map(|x| x as u64)
            });
            whole
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| bad_value(key))
        })
        .transpose()
}

/// The finite number `req` sets for `key`.
fn number_param(req: &MediaRequest, key: &str) -> Result<Option<f64>, MediaError> {
    req.params
        .get(key)
        .map(|value| {
            value
                .as_f64()
                .filter(|x| x.is_finite())
                .ok_or_else(|| bad_value(key))
        })
        .transpose()
}

/// The text `req` sets for `key`.
fn text_param<'a>(req: &'a MediaRequest, key: &str) -> Result<Option<&'a str>, MediaError> {
    req.params
        .get(key)
        .map(|value| value.as_str().ok_or_else(|| bad_value(key)))
        .transpose()
}

/// The word `req` sets for `key`, read by `read`; refused when `read` does not know it.
fn word_param<T>(
    req: &MediaRequest,
    key: &str,
    read: impl Fn(&str) -> Option<T>,
) -> Result<Option<T>, MediaError> {
    text_param(req, key)?
        .map(|word| read(word).ok_or_else(|| bad_value(key)))
        .transpose()
}

/// A local failure: never charged, with `message` in its details.
fn failure(kind: FailureKind, message: impl Into<String>) -> MediaFailure {
    MediaFailure {
        kind,
        stage: None,
        charged: Some(false),
        company_code: None,
        request_id: None,
        status: None,
        message: message.into(),
        retry_after: None,
    }
}

/// A run refused before the engine starts.
fn refused(kind: FailureKind, message: impl Into<String>) -> MediaError {
    MediaError::Answered(failure(kind, message))
}

fn bad_value(key: &str) -> MediaError {
    refused(FailureKind::BadRequest, errors::local_bad_value(key))
}

fn args_refused(e: &ArgsError) -> MediaError {
    refused(FailureKind::BadRequest, errors::local_args_refused(e))
}

/// A failed write of a result: a full disk by name, anything else with its text.
fn local_io(e: &io::Error) -> MediaError {
    MediaError::Local(if e.kind() == io::ErrorKind::StorageFull {
        LocalIo::DiskFull
    } else {
        LocalIo::Io(e.to_string())
    })
}

// Nothing panics while these are held and none can be left half written, so a poisoned
// lock is used as it is.

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use agent::media::{
        Company, Cost, FailureKind, Fetched, JobDone, JobState, Layer, MediaError, MediaKind,
        MediaModel, MediaProvider, MediaRequest, ParamKind, ParamSpec, Price, RefImage, RefRole,
        SEED_MAX, Started, parse_catalog, validate,
    };
    use async_trait::async_trait;
    use models::catalog::Catalog;
    use models::gen_catalog::{GenCatalog, GenEntry, GenRole, Quirk, gen_catalog};
    use models::gguf::testing::diffusion_gguf;
    use runtime::sd::args::{OutputFormat, PartRole, build_args};
    use runtime::sd::devices::parse_list_devices;
    use runtime::sd::engine::{Engine, EngineSource, program_file_name};
    use runtime::sd::failure::SdFailure;
    use runtime::sd::run::LIST_DEVICES_TIMEOUT;
    use runtime::sd::testing::{FAKE_MODE_ENV, FAKE_RECORD_ENV, fake_main};
    use serde_json::{Map, Value, json};

    use super::*;
    use crate::studio::local_models::entry_from_installed;
    use crate::studio::test_support::Scratch;

    const KLEIN: &str = "flux2-klein-4b-q4";
    const ZIMAGE: &str = "z-image-turbo-q4";
    const WAN: &str = "wan22-ti2v-5b-fast-q6";

    /// This test binary as the Studio engine: started with [`FAKE_MODE_ENV`] set (by
    /// [`fake_engine`], whose leading arguments run only this test), it is `sd-cli` for the
    /// arguments after `--`. libtest has already written `test … ... ` without a line end, so
    /// the line is ended first: the engine's first line (a device of `--list-devices`, the
    /// fake's own grandchild saying it is ready) is then a line of its own. A plain test run
    /// does nothing.
    #[test]
    fn fake_sd_process() {
        if std::env::var_os(FAKE_MODE_ENV).is_none() {
            return;
        }
        println!();
        let args: Vec<String> = std::env::args()
            .skip_while(|arg| arg != "--")
            .skip(1)
            .collect();
        fake_main(&args);
    }

    /// This test binary as the engine, in `mode`, recording each start into `rec`.
    pub(crate) fn fake_engine(mode: &str, rec: &Path) -> Engine {
        let program = std::env::current_exe().unwrap();
        Engine {
            dir: program.parent().unwrap().to_path_buf(),
            program,
            build: runtime::sd::lock::embedded().sd_cpp.build,
            source: EngineSource::Dev,
            leading_args: [
                "studio::local::tests::fake_sd_process",
                "--exact",
                "--nocapture",
                "--test-threads=1",
                "--",
            ]
            .map(OsString::from)
            .into(),
            extra_env: vec![
                (FAKE_MODE_ENV.into(), mode.into()),
                (FAKE_RECORD_ENV.into(), rec.to_str().unwrap().into()),
            ],
            vulkan_fallback: None,
        }
    }

    /// The bundled catalog's picture and video models.
    pub(crate) fn catalog() -> GenCatalog {
        gen_catalog(
            &Catalog::bundled(),
            runtime::sd::lock::embedded().sd_cpp.build,
        )
    }

    pub(crate) struct Fixed(pub(crate) GenCatalog);

    #[async_trait]
    impl GenCatalogSource for Fixed {
        async fn gen_catalog(&self) -> GenCatalog {
            self.0.clone()
        }
    }

    /// A value's serde word (`GenKind::Image` → `image`).
    fn word<T: serde::Serialize>(value: &T) -> String {
        serde_json::to_value(value)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Writes the parts of catalog entry `id` under `models` (its diffusion file the tiny model
    /// of the recorded tensor table `table`, every other part a few bytes) and returns its
    /// installed row, each part's `file` and `size` those of the file written.
    pub(crate) fn install(models: &Path, id: &str, table: &str) -> store::NewGenModel {
        let entry = catalog().entries.remove(id).unwrap();
        let mut total = 0;
        let parts = entry
            .parts
            .iter()
            .map(|p| {
                let file = models::download::destination(models, &p.repo, &p.path).unwrap();
                let bytes = match p.role {
                    GenRole::Diffusion | GenRole::Model => diffusion_gguf(table),
                    _ => vec![7; 300],
                };
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(&file, &bytes).unwrap();
                total += bytes.len() as u64;
                json!({
                    "role": p.role,
                    "repo": p.repo,
                    "revision": p.revision,
                    "path": p.path,
                    "file": file.to_str().unwrap(),
                    "size": bytes.len(),
                    "sha256": p.sha256,
                    "license": p.license,
                })
            })
            .collect();
        store::NewGenModel {
            id: id.into(),
            catalog_id: Some(id.into()),
            name: entry.name.clone(),
            kind: word(&entry.kind),
            family: word(&entry.family),
            recipe_id: Some(entry.recipe.clone()),
            parts: Value::Array(parts),
            size_bytes: total,
            source: store::GenSource::Catalog,
        }
    }

    /// The row the store keeps for `new`.
    fn row_of(new: store::NewGenModel) -> store::GenModel {
        store::GenModel {
            id: new.id,
            catalog_id: new.catalog_id,
            name: new.name,
            kind: new.kind,
            family: new.family,
            recipe_id: new.recipe_id,
            parts: new.parts,
            size_bytes: new.size_bytes,
            source: new.source,
            added_at: 0,
        }
    }

    /// The recorded table of each installed model's diffusion file.
    const TABLES: [(&str, &str); 3] = [
        (KLEIN, "klein4b-tensors.txt"),
        (ZIMAGE, "zimage-tensors.txt"),
        (WAN, "wan22-ti2v-5b-tensors.txt"),
    ];

    /// FLUX.2 klein, Z-Image Turbo and Wan 2.2 installed under `<dir>/models`, a gallery's
    /// `.partial` folder, the fake engine's record folder, and the provider over them.
    struct World {
        dir: Scratch,
        store: store::Store,
        provider: LocalMediaProvider,
    }

    impl World {
        fn models(&self) -> PathBuf {
            self.dir.path().join("models")
        }
        fn partial(&self) -> PathBuf {
            self.dir.path().join("gallery").join(".partial")
        }
        fn rec(&self) -> PathBuf {
            self.dir.path().join("record")
        }
        /// The run folder of `remote_id` when it was started with `partial()` as its scratch.
        fn run_dir(&self, remote_id: &str) -> PathBuf {
            self.partial()
                .join(remote_id.strip_prefix("local:").unwrap())
        }
        /// Part `role`'s file of installed model `id`.
        async fn part_file(&self, id: &str, role: &str) -> PathBuf {
            let row = self.store.get_gen_model(id).await.unwrap().unwrap();
            let part = row
                .parts
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["role"] == role)
                .unwrap();
            PathBuf::from(part["file"].as_str().unwrap())
        }
    }

    /// The three models installed, and a provider that finds an engine only through
    /// `discover`.
    async fn world_with(discover: DiscoverEngine) -> World {
        let dir = Scratch::new();
        let store = store::Store::open_in_memory().unwrap();
        let provider = LocalMediaProvider::new(store.clone(), Arc::new(Fixed(catalog())), discover);
        let world = World {
            dir,
            store,
            provider,
        };
        std::fs::create_dir_all(world.partial()).unwrap();
        std::fs::create_dir_all(world.rec()).unwrap();
        for (id, table) in TABLES {
            world
                .store
                .insert_gen_model(install(&world.models(), id, table))
                .await
                .unwrap();
        }
        world
    }

    /// The three models installed, with the fake engine in `mode` set.
    async fn world(mode: &str) -> World {
        let world = world_with(Arc::new(|| None)).await;
        world
            .provider
            .set_engine(Some(fake_engine(mode, &world.rec())));
        world
    }

    /// A provider over an empty store, with installed model `id`'s entry and row, for the
    /// engine job alone: its files are written under `dir`.
    fn offline(dir: &Scratch, id: &str) -> (LocalMediaProvider, GenEntry, store::GenModel) {
        let table = TABLES.iter().find(|(m, _)| *m == id).unwrap().1;
        let row = row_of(install(&dir.path().join("models"), id, table));
        let entry = entry_from_installed(&row, &catalog()).unwrap();
        let provider = LocalMediaProvider::new(
            store::Store::open_in_memory().unwrap(),
            Arc::new(Fixed(catalog())),
            Arc::new(|| None),
        );
        (provider, entry, row)
    }

    fn request(model: &str, kind: MediaKind, prompt: &str, params: Value) -> MediaRequest {
        let Value::Object(params) = params else {
            panic!("params are an object")
        };
        MediaRequest {
            kind,
            model: model.into(),
            prompt: prompt.into(),
            params,
            extra: Map::new(),
            refs: Vec::new(),
            routing: None,
            provider_options: BTreeMap::new(),
        }
    }

    /// A FLUX.2 klein picture with no settings.
    fn req(prompt: &str) -> MediaRequest {
        request(KLEIN, MediaKind::Image, prompt, json!({}))
    }

    fn picture(params: Value) -> MediaRequest {
        request(KLEIN, MediaKind::Image, "x", params)
    }

    fn clip(params: Value) -> MediaRequest {
        request(WAN, MediaKind::Video, "x", params)
    }

    fn added(role: RefRole, path: &Path) -> RefImage {
        RefImage {
            role,
            mime: "image/png".into(),
            path: path.to_path_buf(),
            sha256: "0".repeat(64),
            size_bytes: 1,
        }
    }

    fn keys(m: &MediaModel, layer: Layer) -> Vec<&str> {
        m.params
            .iter()
            .filter(|p| p.layer == layer)
            .map(|p| p.key.as_str())
            .collect()
    }

    fn param<'a>(m: &'a MediaModel, key: &str) -> &'a ParamSpec {
        m.params.iter().find(|p| p.key == key).unwrap()
    }

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|arg| arg.into_string().unwrap())
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

    /// The arguments start `n` of the fake engine recorded, if it started.
    fn argv(rec: &Path, n: usize) -> Option<Vec<String>> {
        let text = std::fs::read(rec.join(format!("argv-{n}.json"))).ok()?;
        Some(serde_json::from_slice(&text).unwrap())
    }

    fn no_argv_recorded(rec: &Path) -> bool {
        argv(rec, 0).is_none()
    }

    /// Checks `remote_id` until it is done or failed.
    async fn ended(p: &LocalMediaProvider, remote_id: &str) -> JobState {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            match p.check(remote_id).await.unwrap() {
                JobState::Pending | JobState::Running { .. } => {}
                state => return state,
            }
            assert!(Instant::now() < deadline, "the run did not end");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn poll_until_done(p: &LocalMediaProvider, remote_id: &str) -> JobDone {
        match ended(p, remote_id).await {
            JobState::Done(done) => done,
            other => panic!("the run did not succeed: {other:?}"),
        }
    }

    async fn started(world: &World, req: &MediaRequest) -> String {
        match world.provider.start(req, &world.partial()).await.unwrap() {
            Started::Job { remote_id } => remote_id,
            other => panic!("not a job: {other:?}"),
        }
    }

    fn refusal(result: Result<Started, MediaError>) -> agent::media::MediaFailure {
        match result {
            Err(MediaError::Answered(f)) => f,
            other => panic!("not refused: {other:?}"),
        }
    }

    #[tokio::test]
    async fn installed_models_become_media_models_with_local_controls() {
        let w = world("ok").await;
        assert_eq!(w.provider.company(), Company::Local);
        assert_eq!(w.provider.host(), "");
        assert_eq!(
            w.provider.fetch_image_price(KLEIN).await.unwrap(),
            Value::Null
        );
        let raw = w.provider.fetch_catalog().await.unwrap();
        assert_eq!(raw.videos, None);
        let m = parse_catalog(Company::Local, &raw, &BTreeMap::new());
        assert_eq!(m.len(), 3);
        let klein = m.iter().find(|m| m.id == "flux2-klein-4b-q4").unwrap();
        assert_eq!(klein.price, Price::Free);
        assert_eq!((klein.kind, klein.accepts_images), (MediaKind::Image, true));
        assert_eq!(
            keys(klein, Layer::Simple),
            ["aspect_ratio", "n", "input_references"]
        );
        // Its guidance is 1.0: no guidance and no negative prompt.
        assert_eq!(
            keys(klein, Layer::Advanced),
            [
                "seed",
                "steps",
                "sampler",
                "scheduler",
                "size",
                "memory_saving",
                "run_on"
            ]
        );
        assert!(
            klein
                .params
                .iter()
                .all(|p| !matches!(p.kind, ParamKind::Extra { .. }))
        );
        assert!(klein.params.iter().all(|p| !p.affects_price));
        assert_eq!(param(klein, "steps").initial, Some(json!(4)));
        assert_eq!(
            param(klein, "sampler").company_default,
            Some(json!("euler"))
        );
        assert_eq!(param(klein, "seed").kind, ParamKind::Seed { max: SEED_MAX });
        let wan = m.iter().find(|m| m.kind == MediaKind::Video).unwrap();
        assert_eq!(
            keys(wan, Layer::Simple),
            ["aspect_ratio", "resolution", "duration", "first_frame"]
        );
        assert_eq!(
            (
                param(wan, "resolution").initial.clone(),
                param(wan, "duration").initial.clone()
            ),
            (Some(json!("480p")), Some(json!(2)))
        );
        // 1 to 5 s: 121 frames at 24 fps hold five whole seconds.
        let ParamKind::Enum { values } = &param(wan, "duration").kind else {
            panic!("the length is a list")
        };
        assert_eq!(
            values.iter().map(|c| c.value.clone()).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5].map(Value::from)
        );
        // A clip runs on the graphics card only: no Run on (ruling L14-video-processor).
        let advanced = keys(wan, Layer::Advanced);
        assert!(advanced.contains(&"memory_saving") && !advanced.contains(&"run_on"));
        let zimage = m.iter().find(|m| m.id == ZIMAGE).unwrap();
        assert_eq!(keys(zimage, Layer::Simple), ["aspect_ratio", "n"]);
        assert!(!zimage.accepts_images);
    }

    /// A model whose default guidance is above 1 offers its guidance (from 1 to 20, by
    /// halves) and a negative prompt, a text field as long as the prompt may be.
    #[test]
    fn guidance_and_a_negative_prompt_come_with_a_default_guidance_above_one() {
        let mut entry = catalog().entries.remove(KLEIN).unwrap();
        entry.defaults.cfg = 3.5;
        let params = LocalMediaProvider::params_for(&entry);
        let cfg = params.iter().find(|p| p.key == "cfg_scale").unwrap();
        assert_eq!(
            cfg.kind,
            ParamKind::Range {
                min: 1.0,
                max: 20.0,
                step: Some(0.5),
                unit: None
            }
        );
        assert_eq!(cfg.company_default, Some(json!(3.5)));
        let negative = params.iter().find(|p| p.key == "negative_prompt").unwrap();
        assert_eq!(negative.kind, ParamKind::Text { max_chars: 4_000 });
        assert_eq!(negative.layer, Layer::Advanced);
    }

    /// The fields the lane resolves itself read as the controls offer them: Memory saving
    /// (Automatic when not set) and Run on (the graphics card when not set).
    #[test]
    fn the_lanes_own_fields_read_as_offered() {
        let params = LocalMediaProvider::params_for(&catalog().entries[KLEIN]);
        let offered = |key: &str| match &params.iter().find(|p| p.key == key).unwrap().kind {
            ParamKind::Enum { values } => {
                values.iter().map(|c| c.value.clone()).collect::<Vec<_>>()
            }
            other => panic!("{key} is {other:?}"),
        };
        assert_eq!(
            offered("memory_saving"),
            ["automatic", "off", "on"].map(Value::from)
        );
        // the pinned engine's full lists (design 7.6), each word with a plain label
        let (samplers, schedulers) = (offered("sampler"), offered("scheduler"));
        assert_eq!((samplers.len(), schedulers.len()), (21, 17));
        assert_eq!(samplers[0], "euler");
        assert!(schedulers.contains(&json!("ltx2")));
        assert!(
            params
                .iter()
                .filter(|p| p.key == "sampler" || p.key == "scheduler")
                .all(|p| matches!(&p.kind, ParamKind::Enum { values }
                    if values.iter().all(|c| c.value != c.label)))
        );
        assert_eq!(
            offered("run_on"),
            ["graphicsCard", "processor"].map(Value::from)
        );
        assert_eq!(MemoryChoice::of(&req("x").params), MemoryChoice::Automatic);
        for (word, choice) in [
            ("automatic", MemoryChoice::Automatic),
            ("off", MemoryChoice::Off),
            ("on", MemoryChoice::On),
        ] {
            assert_eq!(
                MemoryChoice::of(&picture(json!({"memory_saving": word})).params),
                choice
            );
        }
        assert!(!runs_on_processor(&req("x")));
        assert!(!runs_on_processor(&picture(
            json!({"run_on": "graphicsCard"})
        )));
        assert!(runs_on_processor(&picture(json!({"run_on": "processor"}))));
        // A clip is never made on the processor: the field is not offered, and a processor
        // sent anyway is ignored (ruling L14-video-processor).
        let wan = LocalMediaProvider::params_for(&catalog().entries[WAN]);
        assert!(wan.iter().all(|p| p.key != "run_on"));
        assert!(!runs_on_processor(&clip(json!({"run_on": "processor"}))));
    }

    /// What the controls offer is what the engine job reads: a request using each of them
    /// passes the check and becomes a job.
    #[test]
    fn a_request_the_controls_allow_becomes_a_job() {
        let dir = Scratch::new();
        let (provider, klein, row) = offline(&dir, KLEIN);
        let model = MediaModel {
            id: KLEIN.into(),
            display_name: klein.name.clone(),
            family: String::new(),
            kind: MediaKind::Image,
            accepts_images: true,
            params: LocalMediaProvider::params_for(&klein),
            price: Price::Free,
            notes: Vec::new(),
            usable: None,
            created: None,
            companies: Vec::new(),
        };
        let mut r = picture(json!({
            "aspect_ratio": "16:9",
            "n": 4,
            "seed": SEED_MAX,
            "steps": 50,
            "sampler": "dpm++2m",
            "scheduler": "karras",
            "memory_saving": "on",
            "run_on": "processor",
        }));
        r.refs = vec![added(RefRole::Reference, &dir.path().join("a.png"))];
        assert_eq!(validate(&model, &r), Ok(()));
        let out = dir.path().join(".partial").join("r1");
        let job = provider
            .job_for(
                &klein,
                &row,
                &r,
                &LocalJobOptions::default(),
                None,
                &out,
                "r1",
            )
            .unwrap();
        assert_eq!((job.width, job.height), (1360, 768));
        assert_eq!((job.batch, job.steps, job.seed), (4, 50, SEED_MAX));
        assert_eq!(job.ref_images, [dir.path().join("a.png")]);
        let a = strings(build_args(&job).unwrap());
        assert_eq!(
            (after(&a, "--sampling-method"), after(&a, "--scheduler")),
            ("dpm++2m", "karras")
        );
        // The memory and processor fields are the lane's to resolve: the job follows `opts`.
        assert!(!job.vae_tiling && !job.offload_to_cpu && !job.processor_only);
        // An exact size wins over the shape.
        let r = picture(json!({"aspect_ratio": "16:9", "size": "768x512"}));
        assert_eq!(validate(&model, &r), Ok(()));
        let job = provider
            .job_for(
                &klein,
                &row,
                &r,
                &LocalJobOptions::default(),
                None,
                &out,
                "r1",
            )
            .unwrap();
        assert_eq!((job.width, job.height), (768, 512));
    }

    /// A picture with nothing set is the entry's defaults: its parts each with their one flag,
    /// in order, its size, steps, guidance and sampler, one picture, a PNG named for the run.
    #[test]
    fn a_picture_with_nothing_set_is_the_entrys_defaults() {
        let dir = Scratch::new();
        let (provider, klein, row) = offline(&dir, KLEIN);
        let out = dir.path().join(".partial").join("r1");
        let job = provider
            .job_for(
                &klein,
                &row,
                &req("x"),
                &LocalJobOptions::default(),
                None,
                &out,
                "r1",
            )
            .unwrap();
        let files: Vec<PathBuf> = row
            .parts
            .as_array()
            .unwrap()
            .iter()
            .map(|p| PathBuf::from(p["file"].as_str().unwrap()))
            .collect();
        assert_eq!(
            job.parts,
            [PartRole::Diffusion, PartRole::Llm, PartRole::Vae]
                .into_iter()
                .zip(files)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            (job.width, job.height, job.steps, job.cfg),
            (1024, 1024, 4, 1.0)
        );
        assert_eq!(job.sampler, Some(runtime::sd::args::Sampler::Euler));
        assert_eq!((job.batch, job.frames, job.fps), (1, None, None));
        assert!(job.seed <= SEED_MAX);
        assert_eq!(
            (job.out_dir.as_path(), job.out_stem.as_str()),
            (out.as_path(), "r1")
        );
        assert_eq!(job.format, OutputFormat::Png);
        assert_eq!((job.backend, job.threads), (None, None));
        // The other shapes keep about the default's pixels, on the entry's multiple of 16.
        for (shape, size) in [
            ("1:1", (1024, 1024)),
            ("4:3", (1168, 880)),
            ("9:16", (768, 1360)),
        ] {
            let job = provider
                .job_for(
                    &klein,
                    &row,
                    &picture(json!({"aspect_ratio": shape})),
                    &LocalJobOptions::default(),
                    None,
                    &out,
                    "r1",
                )
                .unwrap();
            assert_eq!((job.width, job.height), size, "{shape}");
        }
    }

    #[tokio::test]
    async fn the_preview_is_the_exact_command() {
        let w = world("ok").await;
        // The manager reads the list before it asks for the command.
        w.provider.fetch_catalog().await.unwrap();
        let p = w.provider.preview(&req("a lighthouse"));
        assert_eq!(
            (p.host.as_str(), p.body["prompt"].as_str()),
            ("", Some("a lighthouse"))
        );
        assert_eq!((p.method.as_str(), p.url.as_str()), ("run", "sd-cli"));
        assert_eq!(p.body["negativePrompt"], Value::Null);
        let argv = p.body["argv"].as_array().unwrap();
        assert!(
            argv.iter().any(|a| a == "--prompt-file")
                && !argv
                    .iter()
                    .any(|a| a.as_str().unwrap().contains("lighthouse"))
        );
        // The model's files are shown by their paths.
        let diffusion = w.part_file(KLEIN, "diffusion").await;
        let argv: Vec<String> = argv
            .iter()
            .map(|a| a.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            after(&argv, "--diffusion-model"),
            diffusion.to_str().unwrap()
        );
        // The lane's options show as they would run.
        let opts = LocalJobOptions {
            processor_only: true,
            threads: Some(5),
            ..Default::default()
        };
        let p = w.provider.preview_local(&req("a lighthouse"), &opts);
        let argv: Vec<String> = p.body["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            (after(&argv, "--backend"), after(&argv, "--threads")),
            ("cpu", "5")
        );
    }

    /// A klein picture installed with 0.1.x, as 0.1.2's `job_for` and `build_args` gave it:
    /// recorded from the unchanged 0.1.2 code before Task S2 (preflight P4, Review Focus 5).
    /// The part files are fixed (`/m/<file>`) and the run's folder is `/g/.partial/r1`, so the
    /// vector is the same on every computer; a Windows separator reads as `/`.
    const KLEIN_0_1_2_ARGS: &[&str] = &[
        "--diffusion-model",
        "/m/flux-2-klein-4b-Q4_0.gguf",
        "--llm",
        "/m/Qwen3-4B-Q4_K_M.gguf",
        "--vae",
        "/m/flux2-vae.safetensors",
        "--prompt-file",
        "/g/.partial/r1/prompt.txt",
        "-W",
        "1024",
        "-H",
        "1024",
        "--steps",
        "4",
        "--cfg-scale",
        "1",
        "--sampling-method",
        "euler",
        "--seed",
        "7",
        "--batch-count",
        "1",
        "--backend",
        "Vulkan0",
        "-o",
        "/g/.partial/r1/r1.png",
        "--disable-image-metadata",
        "--log-level",
        "info",
    ];

    #[test]
    fn an_01x_klein_picture_keeps_its_0_1_2_arguments() {
        // the klein catalog entry, 1024×1024, 4 steps, seed 7; a green fit, so 0.1.2's options
        // have memory_saving: false; Vulkan0
        let dir = Scratch::new();
        let (provider, klein, mut row) = offline(&dir, KLEIN);
        for part in row.parts.as_array_mut().unwrap() {
            let name = part["path"].as_str().unwrap().rsplit('/').next().unwrap();
            part["file"] = json!(format!("/m/{name}"));
        }
        let opts = LocalJobOptions {
            processor_only: false,
            memory_saving: false,
            threads: None,
            device_description: None,
        };
        let job = provider
            .job_for(
                &klein,
                &row,
                &picture(json!({"seed": 7})),
                &opts,
                Some("Vulkan0".into()),
                Path::new("/g/.partial/r1"),
                "r1",
            )
            .unwrap();
        let args: Vec<String> = strings(build_args(&job).unwrap())
            .into_iter()
            .map(|arg| arg.replace('\\', "/"))
            .collect();
        assert_eq!(args, KLEIN_0_1_2_ARGS);
    }

    #[test]
    fn the_job_follows_the_lanes_options() {
        let dir = Scratch::new();
        let (provider, klein, row) = offline(&dir, KLEIN);
        let out = dir.path().join(".partial").join("r1");
        let opts = LocalJobOptions {
            processor_only: true,
            threads: Some(3),
            ..Default::default()
        };
        let a = strings(
            build_args(
                &provider
                    .job_for(
                        &klein,
                        &row,
                        &req("x"),
                        &opts,
                        Some("vulkan0".into()),
                        &out,
                        "r1",
                    )
                    .unwrap(),
            )
            .unwrap(),
        );
        assert_eq!(
            (after(&a, "--backend"), after(&a, "--threads")),
            ("cpu", "3")
        );
        let opts = LocalJobOptions {
            memory_saving: true,
            ..Default::default()
        };
        let a = strings(
            build_args(
                &provider
                    .job_for(
                        &klein,
                        &row,
                        &req("x"),
                        &opts,
                        Some("vulkan0".into()),
                        &out,
                        "r1",
                    )
                    .unwrap(),
            )
            .unwrap(),
        );
        assert_eq!(after(&a, "--backend"), "vulkan0");
        assert!(a.contains(&"--offload-to-cpu".into()));
        // Threads go only with a run on the processor.
        let opts = LocalJobOptions {
            threads: Some(3),
            ..Default::default()
        };
        let job = provider
            .job_for(&klein, &row, &req("x"), &opts, None, &out, "r1")
            .unwrap();
        assert_eq!(job.threads, None);
    }

    /// The engine's numbers and texts at their edges: the largest seed and a negative prompt of
    /// 4,000 characters go, one more of either is refused before the engine starts.
    #[test]
    fn numbers_and_texts_at_their_edges() {
        let dir = Scratch::new();
        let (provider, klein, row) = offline(&dir, KLEIN);
        let out = dir.path().join(".partial").join("r1");
        let job = |params: Value| {
            provider.job_for(
                &klein,
                &row,
                &picture(params),
                &LocalJobOptions::default(),
                None,
                &out,
                "r1",
            )
        };
        assert_eq!(job(json!({"seed": SEED_MAX})).unwrap().seed, SEED_MAX);
        let bad = |result: Result<SdJob, MediaError>| match result {
            Err(MediaError::Answered(f)) => (f.kind, f.charged),
            other => panic!("not refused: {other:?}"),
        };
        assert_eq!(
            bad(job(json!({"seed": SEED_MAX + 1}))),
            (FailureKind::BadRequest, Some(false))
        );
        let longest = "n".repeat(4_000);
        assert_eq!(
            job(json!({"negative_prompt": longest})).unwrap().negative,
            Some(longest)
        );
        assert_eq!(
            bad(job(json!({"negative_prompt": "n".repeat(4_001)}))),
            (FailureKind::BadRequest, Some(false))
        );
        // A blank negative prompt is none.
        assert_eq!(
            job(json!({"negative_prompt": "  "})).unwrap().negative,
            None
        );
        // A count or steps that are not whole numbers, and a word off the list, are refused.
        assert_eq!(bad(job(json!({"n": 1.5}))).0, FailureKind::BadRequest);
        assert_eq!(bad(job(json!({"steps": -1}))).0, FailureKind::BadRequest);
        assert_eq!(
            bad(job(json!({"sampler": "ddim"}))).0,
            FailureKind::BadRequest
        );
        assert_eq!(bad(job(json!({"size": "big"}))).0, FailureKind::BadRequest);
    }

    #[tokio::test]
    async fn a_prompt_too_long_or_a_size_the_engine_refuses_never_starts_it() {
        let w = world("ok").await;
        let f = refusal(
            w.provider
                .start(&req(&"a".repeat(4_001)), &w.partial())
                .await,
        );
        assert_eq!((f.kind, f.charged), (FailureKind::BadRequest, Some(false)));
        // An exact size beyond the engine's 2048 is refused with the arguments' own sentence.
        let f = refusal(
            w.provider
                .start(&picture(json!({"size": "2064x1024"})), &w.partial())
                .await,
        );
        assert_eq!(f.kind, FailureKind::BadRequest);
        assert!(
            f.message.contains("multiple of 16 from 64 to 2048"),
            "{}",
            f.message
        );
        assert!(no_argv_recorded(&w.rec()));
        assert_eq!(std::fs::read_dir(w.partial()).unwrap().count(), 0);
    }

    /// A Studio folder whose path holds % followed by d (`50%discount`) is refused before the
    /// engine starts, and the card names the folder in plain words, never the arguments' own
    /// sentence about an output path (finding local-percent-gallery-wording).
    #[tokio::test]
    async fn a_studio_folder_with_percent_d_in_its_path_is_named_in_plain_words() {
        let w = world("ok").await;
        let partial = w.dir.path().join("50%discount").join(".partial");
        std::fs::create_dir_all(&partial).unwrap();
        let f = refusal(w.provider.start(&req("x"), &partial).await);
        assert_eq!(
            (f.kind, f.message.as_str()),
            (FailureKind::BadRequest, errors::LOCAL_FOLDER_NUMBERED)
        );
        assert!(no_argv_recorded(&w.rec()));
        let e = errors::from_local_failure(&f, MediaKind::Image);
        assert_eq!(
            (e.message.as_str(), e.next_step.as_str()),
            (
                "Studio can't save local results in a folder whose name has % followed by d.",
                "Choose another Studio folder in Settings → Studio."
            )
        );
    }

    #[tokio::test]
    async fn start_local_matches_the_device_by_description() {
        let w = world("ok").await;
        let opts = LocalJobOptions {
            device_description: Some("AMD Radeon RX 7600 (RADV NAVI33)".into()),
            ..Default::default()
        };
        let scratch = w.partial().join("r1");
        let Started::Job { remote_id } = w
            .provider
            .start_local(&req("x"), &scratch, &opts)
            .await
            .unwrap()
        else {
            panic!("not a job")
        };
        assert_eq!(remote_id, "local:r1");
        poll_until_done(&w.provider, &remote_id).await;
        let a = argv(&w.rec(), 0).unwrap();
        assert_eq!(after(&a, "--backend"), "vulkan0");
        assert_eq!(after(&a, "-o"), scratch.join("r1.png").to_str().unwrap());
        // A card the engine does not list leaves the engine its own choice.
        let opts = LocalJobOptions {
            device_description: Some("NVIDIA GeForce RTX 4070".into()),
            ..Default::default()
        };
        let remote_id = match w
            .provider
            .start_local(&req("x"), &w.partial().join("r2"), &opts)
            .await
            .unwrap()
        {
            Started::Job { remote_id } => remote_id,
            other => panic!("not a job: {other:?}"),
        };
        poll_until_done(&w.provider, &remote_id).await;
        assert!(!argv(&w.rec(), 1).unwrap().contains(&"--backend".to_owned()));
    }

    /// A run on llama.cpp's graphics card, the Vulkan fixture's `vulkan0`.
    fn on_the_rx_7600() -> LocalJobOptions {
        LocalJobOptions {
            device_description: Some("AMD Radeon RX 7600 (RADV NAVI33)".into()),
            ..Default::default()
        }
    }

    /// The `--backend` an exact command passes, if any.
    fn backend(p: &RequestPreview) -> Option<String> {
        let argv: Vec<&str> = p.body["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        assert!(!argv.is_empty(), "{}", p.body);
        argv.iter()
            .position(|a| *a == "--backend")
            .map(|at| argv[at + 1].to_owned())
    }

    /// The process id a fake engine writes to `path`, once it is there (within 10 s, real
    /// time).
    fn wait_for_pid(path: &Path) -> Option<u32> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(text) = std::fs::read_to_string(path)
                && let Ok(pid) = text.trim().parse()
            {
                return Some(pid);
            }
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// "See the exact command" before any run names the graphics card the run would: the
    /// manager has the engine list its devices first (ruling L14-preview-backend).
    #[tokio::test]
    async fn a_first_preview_names_the_device_the_run_would() {
        let w = world("ok").await;
        // The manager reads the list before it asks for the command.
        w.provider.fetch_catalog().await.unwrap();
        let opts = on_the_rx_7600();
        // Before the engine listed its devices, the command could not name the card.
        assert_eq!(backend(&w.provider.preview_local(&req("x"), &opts)), None);
        // A run on the processor, or one without llama.cpp's device, needs no listing.
        let processor = LocalJobOptions {
            processor_only: true,
            ..opts.clone()
        };
        w.provider.list_devices_for(&processor).await;
        w.provider
            .list_devices_for(&LocalJobOptions::default())
            .await;
        assert!(w.provider.devices.lock().await.is_none());
        w.provider.list_devices_for(&opts).await;
        let shown = backend(&w.provider.preview_local(&req("x"), &opts));
        assert_eq!(shown.as_deref(), Some("vulkan0"));
        // The run passes the same.
        let Started::Job { remote_id } = w
            .provider
            .start_local(&req("x"), &w.partial().join("r1"), &opts)
            .await
            .unwrap()
        else {
            panic!("not a job")
        };
        poll_until_done(&w.provider, &remote_id).await;
        let run = argv(&w.rec(), 0).unwrap();
        assert_eq!(Some(after(&run, "--backend")), shown.as_deref());
    }

    /// An engine that cannot list its devices is asked once: the failure is kept for that
    /// engine, so later commands and the run never start it again for the list, and a
    /// command leaves `--backend` out, as the run does (it leaves the card to the engine).
    #[tokio::test]
    async fn a_device_listing_that_fails_is_kept_and_not_asked_again() {
        let w = world("devices-fail").await;
        w.provider.fetch_catalog().await.unwrap();
        let opts = on_the_rx_7600();
        let listings = || {
            std::fs::read_to_string(w.rec().join("devices-fail.txt"))
                .map_or(0, |text| text.lines().count())
        };
        w.provider.list_devices_for(&opts).await;
        assert_eq!(listings(), 1);
        let engine = w.provider.engine().unwrap();
        assert!(matches!(
            w.provider.devices.lock().await.as_ref(),
            Some((by, None)) if *by == engine
        ));
        assert_eq!(backend(&w.provider.preview_local(&req("x"), &opts)), None);
        // Each command after it finds the failure kept.
        w.provider.list_devices_for(&opts).await;
        w.provider.list_devices_for(&opts).await;
        assert_eq!(backend(&w.provider.preview_local(&req("x"), &opts)), None);
        assert_eq!(listings(), 1);
        // So does the run, which passes no `--backend`, as the command showed.
        let Started::Job { remote_id } = w
            .provider
            .start_local(&req("x"), &w.partial().join("r1"), &opts)
            .await
            .unwrap()
        else {
            panic!("not a job")
        };
        poll_until_done(&w.provider, &remote_id).await;
        assert!(!argv(&w.rec(), 0).unwrap().contains(&"--backend".to_owned()));
        assert_eq!(listings(), 1);
        // Another engine (one a download installed) is asked again.
        w.provider.set_engine(Some(fake_engine("ok", &w.rec())));
        w.provider.list_devices_for(&opts).await;
        let shown = backend(&w.provider.preview_local(&req("x"), &opts));
        assert_eq!(shown.as_deref(), Some("vulkan0"));
    }

    /// An engine that never lists its devices holds "See the exact command" up 5 s at most:
    /// the command then leaves `--backend` out. The one listing goes on for the run (a
    /// second command starts no other) until its own 30 s limit stops the engine, and that
    /// failure is kept: later commands come at once, and the run passes no `--backend`.
    #[tokio::test]
    async fn a_device_listing_that_hangs_holds_the_command_up_5_s_at_most() {
        let w = world("devices-hang").await;
        w.provider.fetch_catalog().await.unwrap();
        let opts = on_the_rx_7600();
        let pid_file = w.rec().join("devices-hang.txt");
        let command = || async {
            w.provider.list_devices_for(&opts).await;
            backend(&w.provider.preview_local(&req("x"), &opts))
        };
        // A paused clock that the test moves on. Tokio moves a paused clock on by itself
        // only while no blocking task runs, so a blocking task waiting for the engine's
        // process id holds it still until the engine has started (as in the runtime's
        // tests/sd_cli.rs); then it moves just past the command's wait.
        tokio::time::pause();
        let clock = tokio::time::Instant::now();
        let (shown, pid) = tokio::join!(command(), async {
            let file = pid_file.clone();
            let pid = tokio::task::spawn_blocking(move || wait_for_pid(&file))
                .await
                .unwrap();
            tokio::time::advance(PREVIEW_DEVICES_WAIT + Duration::from_millis(1)).await;
            pid
        });
        assert_eq!(shown, None);
        let waited = clock.elapsed();
        assert!(
            waited > PREVIEW_DEVICES_WAIT && waited < LIST_DEVICES_TIMEOUT,
            "{waited:?}"
        );
        // The listing goes on for the run.
        let pid = pid.expect("the engine's process id");
        assert!(runtime::process_alive(pid), "{pid} was stopped");
        // A second command waits for the same listing, 5 s at most again.
        std::fs::remove_file(&pid_file).unwrap();
        let (shown, ()) = tokio::join!(
            command(),
            tokio::time::advance(PREVIEW_DEVICES_WAIT + Duration::from_millis(1))
        );
        assert_eq!(shown, None);
        assert!(runtime::process_alive(pid), "{pid} was stopped");
        // At its own limit the listing stops the engine, and its failure is kept for it.
        tokio::time::advance(LIST_DEVICES_TIMEOUT).await;
        let engine = w.provider.engine().unwrap();
        assert!(matches!(
            w.provider.devices.lock().await.as_ref(),
            Some((by, None)) if *by == engine
        ));
        assert!(!runtime::process_alive(pid), "{pid} still runs");
        // A command from now on comes at once, and starts no engine.
        let asked = tokio::time::Instant::now();
        assert_eq!(command().await, None);
        assert!(
            asked.elapsed() < PREVIEW_DEVICES_WAIT,
            "{:?}",
            asked.elapsed()
        );
        // The run, on the real clock, asks nothing and passes no `--backend` either.
        tokio::time::resume();
        let Started::Job { remote_id } = w
            .provider
            .start_local(&req("x"), &w.partial().join("r1"), &opts)
            .await
            .unwrap()
        else {
            panic!("not a job")
        };
        poll_until_done(&w.provider, &remote_id).await;
        assert!(!argv(&w.rec(), 0).unwrap().contains(&"--backend".to_owned()));
        assert!(!pid_file.exists(), "the engine was asked again");
    }

    /// Finding local-engine-folder-deleted-while-running: a run's engine process is counted
    /// until it has ended, and while one runs the engine's folder cannot be claimed (for a
    /// pack install or Remove); while the folder is claimed, no run starts.
    #[tokio::test]
    async fn a_running_engine_holds_its_folder_and_a_claimed_folder_starts_none() {
        let w = world("hang").await;
        let engine = w.provider.engine_use();
        assert!(!engine.in_use());
        let remote_id = started(&w, &req("x")).await;
        assert!(engine.in_use());
        assert!(engine.replace().is_none(), "the folder is in use");
        w.provider.cancel(&remote_id).await.unwrap();
        assert!(!engine.in_use());

        let folder = engine.replace().expect("no engine process runs");
        let f = refusal(w.provider.start(&req("x"), &w.partial()).await);
        assert_eq!(
            (f.kind, f.message.as_str(), f.charged),
            (
                FailureKind::EngineMissing,
                errors::LOCAL_ENGINE_BEING_REPLACED,
                Some(false)
            )
        );
        assert!(!engine.in_use());
        drop(folder);
        let remote_id = started(&w, &req("x")).await;
        assert!(engine.in_use());
        w.provider.cancel(&remote_id).await.unwrap();
        assert!(!engine.in_use());
    }

    /// The one-off device listing a first estimate or command starts runs the engine for up to
    /// 30 s: it holds the engine's folder until it has ended, after the command came back.
    #[tokio::test]
    async fn a_device_listing_holds_the_engine_folder_until_it_ends() {
        let w = world("devices-hang").await;
        w.provider.fetch_catalog().await.unwrap();
        let engine = w.provider.engine_use();
        let opts = on_the_rx_7600();
        let pid_file = w.rec().join("devices-hang.txt");
        // A paused clock that the test moves on, as in the listing tests above.
        tokio::time::pause();
        let ((), pid) = tokio::join!(w.provider.list_devices_for(&opts), async {
            let file = pid_file.clone();
            let pid = tokio::task::spawn_blocking(move || wait_for_pid(&file))
                .await
                .unwrap();
            tokio::time::advance(PREVIEW_DEVICES_WAIT + Duration::from_millis(1)).await;
            pid
        });
        let pid = pid.expect("the engine's process id");
        assert!(runtime::process_alive(pid), "{pid} was stopped");
        assert!(engine.in_use(), "the listing goes on");
        assert!(engine.replace().is_none());
        // At its own limit the listing stops the engine and keeps its failure.
        tokio::time::advance(LIST_DEVICES_TIMEOUT).await;
        drop(w.provider.devices.lock().await);
        assert!(!engine.in_use());
        assert!(engine.replace().is_some());
    }

    /// llama.cpp's own device listing ends a row with the card's memory, which the engine's
    /// does not print: the card is matched without it.
    #[test]
    fn a_device_description_with_llamas_memory_figures_still_matches() {
        let devices = parse_list_devices(include_str!(
            "../../../crates/runtime/tests/fixtures/sd_list_devices_vulkan.txt"
        ));
        let row = "AMD Radeon RX 7600 (RADV NAVI33) (8176 MiB, 7900 MiB free)";
        assert_eq!(backend_from(&devices, row).as_deref(), Some("vulkan0"));
        // As llama's parser reads its listing, the figures are already gone.
        let llama = runtime::parse_list_devices(&format!("Available devices:\n  Vulkan0: {row}\n"));
        assert_eq!(
            backend_from(&devices, &llama[0].description).as_deref(),
            Some("vulkan0")
        );
        assert_eq!(
            bare_description("AMD Radeon RX 7600 (RADV NAVI33)"),
            "AMD Radeon RX 7600 (RADV NAVI33)"
        );
        assert_eq!(
            backend_from(
                &devices,
                "NVIDIA GeForce RTX 4070 (12282 MiB, 11975 MiB free)"
            ),
            None
        );
    }

    #[tokio::test]
    async fn a_picture_run_ends_done_free_with_its_file() {
        let w = world("ok").await;
        let remote_id = started(&w, &req("x")).await;
        let done = poll_until_done(&w.provider, &remote_id).await;
        assert_eq!(
            done.cost,
            Some(Cost {
                usd: 0.0,
                reported: true
            })
        );
        assert_eq!((done.outputs, done.signed.len()), (1, 0));
        let status = w.provider.status(&remote_id).unwrap();
        assert_eq!(
            (
                status.outputs,
                status.failure,
                status.retrying,
                status.stalled
            ),
            (1, None, false, false)
        );
        let dest = w.partial().join("o1");
        let fetched = w
            .provider
            .download(&remote_id, &done, 0, &dest)
            .await
            .unwrap();
        assert!(matches!(&fetched, Fetched::Saved(s) if s.mime == "image/png"));
        let Fetched::Saved(saved) = fetched else {
            unreachable!()
        };
        assert_eq!(
            (saved.path.as_path(), saved.width, saved.height),
            (dest.as_path(), Some(1024), Some(1024))
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            runtime::sd::testing::tiny_png()
        );
        assert_eq!(saved.size_bytes, std::fs::metadata(&dest).unwrap().len());
        // A result it does not have is refused.
        assert!(
            w.provider
                .download(&remote_id, &done, 1, &w.partial().join("o2"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_clip_run_ends_with_a_webm() {
        let w = world("ok").await;
        let remote_id = started(&w, &clip(json!({"duration": 2}))).await;
        let done = poll_until_done(&w.provider, &remote_id).await;
        let dest = w.partial().join("o1");
        let Fetched::Saved(saved) = w
            .provider
            .download(&remote_id, &done, 0, &dest)
            .await
            .unwrap()
        else {
            panic!("not saved")
        };
        assert_eq!(saved.mime, "video/webm");
        assert_eq!((saved.width, saved.height), (Some(832), Some(480)));
        // 49 frames at 24 fps.
        assert_eq!(saved.duration_ms, Some(2_041));
        let a = argv(&w.rec(), 0).unwrap();
        assert_eq!(
            (after(&a, "--video-frames"), after(&a, "--fps")),
            ("49", "24")
        );
    }

    #[tokio::test]
    async fn cancel_kills_the_run_and_removes_its_partial_folder() {
        let w = world("hang").await;
        let remote_id = started(&w, &req("x")).await;
        let run_dir = w.run_dir(&remote_id);
        let deadline = Instant::now() + Duration::from_secs(60);
        while w.provider.status(&remote_id).unwrap().progress.is_none() {
            assert!(Instant::now() < deadline, "the engine never drew");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(matches!(
            w.provider.check(&remote_id).await.unwrap(),
            JobState::Running { progress: Some(_) }
        ));
        assert!(run_dir.join("prompt.txt").is_file());
        let at = Instant::now();
        w.provider.cancel(&remote_id).await.unwrap();
        assert!(at.elapsed() < Duration::from_secs(1), "{:?}", at.elapsed());
        assert!(!run_dir.exists());
        assert_eq!(w.provider.status(&remote_id), None);
        // A run that was stopped is gone, never failed; stopping it again is no error.
        assert_eq!(w.provider.check(&remote_id).await.unwrap(), JobState::Gone);
        w.provider.cancel(&remote_id).await.unwrap();
    }

    /// Each of the 16 part roles is passed with the flag of the recipe slot it fills.
    #[test]
    fn every_part_role_takes_its_slots_flag() {
        for slot in models::sd::recipes::SlotId::RUNNABLE {
            let role = GenRole::from_slot(slot).unwrap();
            assert_eq!(
                Some(runtime::sd::args::flag_for(part_role(role))),
                slot.flag(),
                "{role:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_file_of_another_family_is_refused_before_the_engine_starts() {
        let w = world("ok").await;
        // klein's diffusion file replaced by a 9B klein's table (no metadata keys, its size
        // recorded): the name says 4B, the tensors do not.
        w.store
            .insert_gen_model(install(&w.models(), KLEIN, "klein9b-tensors.txt"))
            .await
            .unwrap();
        let rec = w.rec();
        assert!(
            matches!(provider_start(&w).await, Err(MediaError::Answered(f)) if f.kind == FailureKind::ModelFileBad)
        );
        assert!(no_argv_recorded(&rec));
    }

    async fn provider_start(w: &World) -> Result<Started, MediaError> {
        w.provider.start(&req("x"), &w.partial()).await
    }

    #[tokio::test]
    async fn release_removes_the_run_folder_and_its_prompt_file() {
        let w = world("ok").await;
        let remote_id = started(&w, &req("x")).await;
        let done = poll_until_done(&w.provider, &remote_id).await;
        let run_dir = w.run_dir(&remote_id);
        assert!(run_dir.join("prompt.txt").is_file());
        let dest = w.partial().join("o1");
        w.provider
            .download(&remote_id, &done, 0, &dest)
            .await
            .unwrap();
        w.provider.release(&remote_id).await;
        assert!(!run_dir.exists());
        assert!(dest.is_file());
        assert_eq!(w.provider.status(&remote_id), None);
        assert_eq!(w.provider.check(&remote_id).await.unwrap(), JobState::Gone);
    }

    #[tokio::test]
    async fn a_missing_part_says_download_it_again() {
        let w = world("ok").await;
        std::fs::remove_file(w.part_file(KLEIN, "vae").await).unwrap();
        let f = refusal(provider_start(&w).await);
        assert_eq!(
            (f.kind, f.charged),
            (FailureKind::ModelFileBad, Some(false))
        );
        // A part of another size than it was downloaded at, too.
        let w = world("ok").await;
        std::fs::write(w.part_file(KLEIN, "llm").await, b"short").unwrap();
        let f = refusal(provider_start(&w).await);
        assert_eq!(f.kind, FailureKind::ModelFileBad);
        assert!(no_argv_recorded(&w.rec()));
    }

    #[tokio::test]
    async fn no_engine_says_engine_missing() {
        let w = world("ok").await;
        w.provider.set_engine(None);
        let f = refusal(provider_start(&w).await);
        assert_eq!(
            (f.kind, f.charged),
            (FailureKind::EngineMissing, Some(false))
        );
        assert_eq!(w.provider.engine_build(), None);
    }

    /// The engine is looked for again while none is known, or the one known is gone: one
    /// that a download installs after start-up runs the next job, without a restart.
    #[tokio::test]
    async fn an_engine_installed_later_is_found_without_a_restart() {
        let found: Arc<Mutex<Option<Engine>>> = Arc::new(Mutex::new(None));
        let discover = {
            let found = Arc::clone(&found);
            Arc::new(move || found.lock().unwrap().clone())
        };
        let w = world_with(discover).await;
        let f = refusal(provider_start(&w).await);
        assert_eq!(f.kind, FailureKind::EngineMissing);
        let engine = fake_engine("ok", &w.rec());
        *found.lock().unwrap() = Some(engine.clone());
        let remote_id = started(&w, &req("x")).await;
        poll_until_done(&w.provider, &remote_id).await;
        assert_eq!(w.provider.engine_build(), Some(engine.build));
        // An engine whose program went (a pack removed) is looked for again.
        w.provider.set_engine(Some(Engine {
            program: w.dir.path().join("gone").join(program_file_name()),
            ..engine.clone()
        }));
        assert_eq!(w.provider.engine(), Some(engine));
    }

    #[tokio::test]
    async fn out_of_memory_twice_is_a_named_failure_not_charged() {
        let w = world("oom").await;
        let remote_id = started(&w, &req("x")).await;
        let JobState::Failed(f) = ended(&w.provider, &remote_id).await else {
            panic!("the run did not fail")
        };
        assert_eq!((f.kind, f.charged), (FailureKind::OutOfMemory, Some(false)));
        let status = w.provider.status(&remote_id).unwrap();
        assert!(status.retrying);
        assert_eq!(status.failure, Some(SdFailure::OutOfMemory));
        // The second start saved memory.
        assert!(
            argv(&w.rec(), 1)
                .unwrap()
                .contains(&"--vae-tiling".to_owned())
        );
        w.provider.release(&remote_id).await;
        assert!(!w.run_dir(&remote_id).exists());
    }

    #[test]
    fn the_metal_quirk_sets_its_variable_on_macos_only() {
        let dir = Scratch::new();
        let (provider, zimage, row) = offline(&dir, ZIMAGE);
        let out = dir.path().join(".partial").join("r1");
        let r = request(ZIMAGE, MediaKind::Image, "x", json!({}));
        let job = provider
            .job_for(
                &zimage,
                &row,
                &r,
                &LocalJobOptions::default(),
                None,
                &out,
                "r1",
            )
            .unwrap();
        let metal = ("GGML_METAL_TENSOR_DISABLE".to_owned(), "1".to_owned());
        assert_eq!(job.env.contains(&metal), cfg!(target_os = "macos"));
        // FLUX.2 klein has no quirk.
        let (provider, klein, row) = offline(&dir, KLEIN);
        let job = provider
            .job_for(
                &klein,
                &row,
                &req("x"),
                &LocalJobOptions::default(),
                None,
                &out,
                "r1",
            )
            .unwrap();
        assert!(job.env.is_empty());
        // A clip is WebM, or an animated WebP on macOS for an entry with that quirk.
        let (provider, mut wan, row) = offline(&dir, WAN);
        let job = |wan: &GenEntry| {
            provider
                .job_for(
                    wan,
                    &row,
                    &clip(json!({})),
                    &LocalJobOptions::default(),
                    None,
                    &out,
                    "r1",
                )
                .unwrap()
                .format
        };
        assert_eq!(job(&wan), OutputFormat::Webm);
        wan.quirks.push(Quirk::VideoWebpOnMac);
        let expected = if cfg!(target_os = "macos") {
            OutputFormat::Webp
        } else {
            OutputFormat::Webm
        };
        assert_eq!(job(&wan), expected);
    }

    #[test]
    fn a_clip_length_becomes_frames_by_the_familys_rule() {
        let dir = Scratch::new();
        let (provider, wan, row) = offline(&dir, WAN);
        let out = dir.path().join(".partial").join("r1");
        let job = |params: Value| {
            provider
                .job_for(
                    &wan,
                    &row,
                    &clip(params),
                    &LocalJobOptions::default(),
                    None,
                    &out,
                    "r1",
                )
                .unwrap()
        };
        let two = job(json!({"duration": 2}));
        assert_eq!((two.frames, two.fps), (Some(49), Some(24)));
        assert_eq!(job(json!({"duration": 5})).frames, Some(121));
        assert_eq!(frames_for(&wan, 1), 25);
        // Never beyond the entry's 121 frames.
        assert_eq!(frames_for(&wan, 60), 121);
        // Nothing set: the entry's own 49 frames at 832×480 (480p at 16:9).
        let plain = job(json!({}));
        assert_eq!(
            (plain.frames, plain.width, plain.height),
            (Some(49), 832, 480)
        );
        for (params, size) in [
            (json!({"resolution": "720p"}), (1280, 704)),
            (
                json!({"resolution": "480p", "aspect_ratio": "9:16"}),
                (480, 832),
            ),
            (json!({"aspect_ratio": "1:1"}), (480, 480)),
        ] {
            let job = job(params.clone());
            assert_eq!((job.width, job.height), size, "{params}");
        }
        // A start frame is the clip's first picture.
        let mut r = clip(json!({}));
        r.refs = vec![added(RefRole::StartFrame, &dir.path().join("start.png"))];
        let job = provider
            .job_for(
                &wan,
                &row,
                &r,
                &LocalJobOptions::default(),
                None,
                &out,
                "r1",
            )
            .unwrap();
        assert_eq!(job.init_image, Some(dir.path().join("start.png")));
        assert!(strings(build_args(&job).unwrap()).contains(&"vid_gen".to_owned()));
    }

    /// The webview works out a local clip's size itself, to ask each Length and Resolution
    /// chip's time (`localClipSize` in src/components/studio/SettingsPanel.tsx). These are the
    /// five cases of its test (`describe("localClipSize")` in
    /// src/components/studio/SettingsPanel.test.tsx), on a multiple of 32 and a longest side
    /// of 1280: a change to either rule shows in both tests.
    #[test]
    fn size_of_matches_the_webviews_local_clip_size() {
        let mut wan = catalog().entries.remove(WAN).unwrap();
        wan.limits.multiple_of = 32;
        wan.limits.max_side = 1280;
        for (params, size) in [
            // 480p at the default shape, 16:9.
            (json!({"resolution": "480p"}), (832, 480)),
            (
                json!({"resolution": "720p", "aspect_ratio": "16:9"}),
                (1280, 704),
            ),
            (
                json!({"resolution": "720p", "aspect_ratio": "9:16"}),
                (704, 1280),
            ),
            (
                json!({"resolution": "720p", "aspect_ratio": "1:1"}),
                (704, 704),
            ),
            // An exact size as it is, whatever the shape and resolution.
            (json!({"size": "1024x576"}), (1024, 576)),
            (
                json!({"size": "1024x576", "resolution": "480p", "aspect_ratio": "9:16"}),
                (1024, 576),
            ),
        ] {
            assert_eq!(
                size_of(&wan, &clip(params.clone())).unwrap(),
                size,
                "{params}"
            );
        }
    }

    /// A shape at a resolution keeps within the entry's longest side, the shorter side keeping
    /// the shape: the size helpers are the fit's own (`gen_fit::at_shape`, ruling
    /// L14-shape-helpers), so the fit, the estimate and the job describe one size.
    #[test]
    fn a_shape_keeps_within_the_entrys_longest_side() {
        let dir = Scratch::new();
        let (provider, mut wan, row) = offline(&dir, WAN);
        wan.limits.max_side = 1024;
        let r = clip(json!({"resolution": "720p"}));
        let job = provider
            .job_for(
                &wan,
                &row,
                &r,
                &LocalJobOptions::default(),
                None,
                &dir.path().join("r1"),
                "r1",
            )
            .unwrap();
        assert_eq!((job.width, job.height), (1024, 576));
        let shape = run_shape(&wan, &r).unwrap();
        assert_eq!((shape.size.width, shape.size.height), (1024, 576));
        // The shape a model starts at is the nearest offered to its default size.
        let klein = LocalMediaProvider::params_for(&catalog().entries[KLEIN]);
        let wan = LocalMediaProvider::params_for(&wan);
        let start = |params: &[ParamSpec]| {
            params
                .iter()
                .find(|p| p.key == "aspect_ratio")
                .unwrap()
                .company_default
                .clone()
        };
        assert_eq!(
            (start(&klein), start(&wan)),
            (Some(json!("1:1")), Some(json!("16:9")))
        );
        let ParamKind::Enum { values } = &klein[0].kind else {
            panic!("the shape is a list")
        };
        assert_eq!(
            values.iter().map(|c| c.value.clone()).collect::<Vec<_>>(),
            ["1:1", "4:3", "3:4", "16:9", "9:16"].map(Value::from)
        );
    }
}
