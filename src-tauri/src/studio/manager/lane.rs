//! The local lane (Task L15): runs on this computer, one at a time for the whole computer, in
//! a queue of their own beside the cloud queues. The rules it keeps are in the manager's
//! documentation; in more detail:
//!
//! - **Starting** ([`StudioManager::start`] with `RunTarget::ThisComputer`), under the same
//!   `starting` lock as a cloud start: the same press sent twice gives the run it made; the
//!   prompt check (`studio_wont_make`), then the prompt's length (at most
//!   `runtime::sd::args::PROMPT_MAX_CHARS`, `studio_prompt_too_long_here`); the model among
//!   the installed ones (`parse_catalog` of the provider's list), its settings checked
//!   (`spec::validate`) and a seed picked when none is set; the run's fit and time
//!   ([`local_verdict`]). One that does not fit even with the chat model unloaded, and is not
//!   a picture the processor can make, is refused (`studio_does_not_fit`). A local run is free
//!   (`Price::Free`): it never asks and never meets the daily limit. Only then is it recorded
//!   (`queued`, $0, "This computer", the installed model and the engine's build), with its
//!   audit row (the prompt's length, never the prompt), and queued.
//! - **Its task**, once it has the lane's one place: the fit again, on the hardware and the
//!   chat model as they are now; the graphics card from the lease (`queued` meanwhile, with
//!   what it waits for, and under Ask until [`StudioManager::answer_unload`]); `running`, with
//!   the device it runs on, as shown, and how ([`job_options`]); one engine job, checked every
//!   [`LOCAL_CHECK_EVERY`] in memory (its progress written and sent at most every
//!   [`PROGRESS_EVERY`]); each result moved to `.partial/<output id>`, labelled as AI-made
//!   there (never with the prompt) and kept in the gallery as a cloud picture is; `done` at $0
//!   with the time it took. A failure is never charged. The card is held from the grant until
//!   the task ends, an out-of-memory retry inside the engine job included, and freed however
//!   it ends.
//! - **Stop** kills the engine's whole tree (`MediaProvider::cancel`); a run still waiting for
//!   the card never starts.
//! - **The chat model** a grant unloaded is kept while the local queue has work; when the
//!   queue empties and "Reload it after Studio" is on, it is loaded again, once.
//! - **Restarts**: a local run the app left queued or running is `interrupted`, never
//!   charged, and its `.partial/<run>` goes. At exit the engine is stopped and nothing is
//!   recorded.

use std::any::Any;
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, PoisonError};
use std::time::Instant;

use agent::media::{
    Company, Fetched, Invalid, JobDone, JobState, MediaError, MediaKind, MediaModel, MediaProvider,
    MediaRequest, Price, RequestPreview, Saved, Started, parse_catalog, validate,
};
use jiff::Timestamp;
use models::fit::Target;
use models::gen_catalog::{GenEntry, GenKind};
use models::gen_eta::{self, Eta, FitWord, MeasuredRun};
use models::gen_fit::{GenFit, GenFitResult, GenSize, gen_fit};
use runtime::sd::args::PROMPT_MAX_CHARS;
use runtime::sd::progress::{LocalProgress, LocalStage};
use serde_json::{Value, json};
use tokio::sync::Notify;

use super::{
    Ending, Inner, Job, LOCAL_CHECK_EVERY, LaneKey, PROGRESS_EVERY, Step, StudioManager, Task,
    Waited, creation_kind, job_options, pick_seed, refs_json, remove_scratch, tool_of,
};
use crate::dto::{
    EstimateDto, EtaDto, LimitDto, LocalEstimateDto, LocalRunDto, RunDto, StartRequest,
};
use crate::error::{AppError, AppResult};
use crate::gpu_lease::{Grant, LeaseWait, Refused, StudioNeed};
use crate::studio::deps::{Endpoint, LocalHardware, LocalLane, with_device_free};
use crate::studio::errors;
use crate::studio::label::{AiLabel, label_file};
use crate::studio::local::{LocalRunStatus, RunShape, run_shape, runs_on_processor};
use crate::studio::prompt_check;
use crate::studio::spend;

/// Runs this computer makes at once.
pub(super) const LOCAL_AT_ONCE: usize = 1;

/// What a run on this computer records where a cloud run records its endpoint.
const THIS_COMPUTER: &str = "This computer";

/// The device a run on the processor records, as shown.
const PROCESSOR: &str = "Processor";

/// How many of this computer's measured runs of a model an estimate reads.
const MEASURED_RUNS: u32 = 10;

/// A recorded run on this computer, for its task.
pub(super) struct LocalStep {
    /// The request as the engine job gets it (the seed added).
    request: MediaRequest,
    entry: GenEntry,
    shape: RunShape,
    /// The model part's pinned revision, for the label.
    revision: String,
}

/// What a local run's card shows beyond its row while Studio follows it, and the answer to
/// "Unload the chat model?" its task waits for.
#[derive(Default)]
pub(super) struct LocalLive {
    /// How long it takes, as last judged.
    eta: Option<Eta>,
    /// What it waits for before its grant.
    waiting_for: Option<LeaseWait>,
    /// The chat model its grant unloaded.
    unloaded_chat: Option<String>,
    /// It runs beside a loaded chat model.
    alongside: bool,
    /// It runs on the processor.
    on_processor: bool,
    /// What the engine last told.
    status: Option<LocalRunStatus>,
    /// When its engine job started.
    started: Option<Instant>,
    /// The answer given and not yet read.
    answer: Option<bool>,
    /// Woken when an answer is given.
    answered: Arc<Notify>,
}

/// A run of a model at one size on this computer: where it would run, its fit and its time
/// there, its word, and what the graphics card's lease is asked for.
#[derive(Debug, Clone)]
pub(crate) struct LocalVerdict {
    /// Its fit where it would run: on the graphics device (or on the processor of a computer
    /// that runs models there), or on the processor for a picture that does not fit the device
    /// even with the chat model unloaded (`processor_only` is then set).
    pub fit: GenFitResult,
    /// How long it takes there: from this computer's runs measured on that device, else the
    /// catalog's first guess; `None` with neither.
    pub eta: Option<Eta>,
    /// The word its fit gets (`gen_eta::fit_word`).
    pub word: FitWord,
    /// The graphics device as shown ("Processor" on a computer that runs models there): what a
    /// run the grant leaves on the card records.
    pub card: String,
    /// llama.cpp's description of the graphics device, for the engine's `--backend`.
    pub description: Option<String>,
    /// Why not, when the word is `NotHere` (`errors::not_here_reason`).
    pub reason: Option<String>,
    /// What the lease is asked for.
    pub need: StudioNeed,
}

/// A local request checked against its installed model, before it is recorded or estimated.
struct LocalPrepared {
    model: MediaModel,
    row: store::GenModel,
    entry: GenEntry,
    request: MediaRequest,
    seed: Option<u64>,
    /// What the settings break; empty when they pass. The prompt's length is checked on its
    /// own (`studio_prompt_too_long_here`).
    problems: Vec<Invalid>,
    /// The engine job's shape, or why it cannot be made.
    shape: Result<RunShape, AppError>,
    /// Its fit and time, once the shape is known.
    verdict: Option<LocalVerdict>,
}

/// How the graphics card was asked for.
enum Card {
    Granted(Grant, Box<dyn Any + Send + Sync>),
    DoesNotFit,
    /// The unload it waited for did not happen.
    NotStarted,
    Stopped,
    Shutdown,
}

/// `entry` at `size`, `steps` and `pictures` on this computer as it is now, judged where the
/// run would go: [`LocalVerdict`]. Every fit Studio shows for a local model is this one, so
/// Create and Get local models say the same. The measured times read are those of the device
/// the run would use, as a run records it ("<device> (<backend>)", or "Processor").
pub(crate) async fn local_verdict(
    entry: &GenEntry,
    size: GenSize,
    steps: u32,
    pictures: u32,
    hardware: &dyn LocalHardware,
    store: &store::Store,
) -> LocalVerdict {
    let shape = RunShape {
        size,
        steps,
        pictures,
    };
    verdict(entry, shape, false, hardware, store).await
}

/// [`local_verdict`], for a run the request puts on the processor when `by_choice` (the
/// `run_on` field).
async fn verdict(
    entry: &GenEntry,
    shape: RunShape,
    by_choice: bool,
    hardware: &dyn LocalHardware,
    store: &store::Store,
) -> LocalVerdict {
    let RunShape {
        size,
        steps,
        pictures,
    } = shape;
    let (target, backend, description) = hardware.target().await;
    let chat = hardware.chat_device_bytes();
    let on_device = gen_fit(entry, size, &target, chat);
    let processor = Target::Cpu {
        ram_free_bytes: free_memory(&target),
    };
    let on_processor = gen_fit(entry, size, &processor, None);
    let processor_ok = entry.kind != GenKind::Video && on_processor.fit != GenFit::Red;
    let shared_memory = matches!(target, Target::AppleUnified { .. });
    let card = if on_device.processor_only {
        PROCESSOR.to_owned()
    } else {
        shown(description.as_deref(), &backend)
    };
    // The processor when the request asks for it, on a computer that runs models there, and
    // for a picture the device cannot take even with the chat model unloaded.
    let not_on_device = gen_eta::fit_word(entry, &on_device, None) == FitWord::NotHere;
    let runs_on_processor =
        by_choice || on_device.processor_only || (not_on_device && processor_ok);
    let (fit, device) = match (runs_on_processor, on_device.processor_only) {
        (false, _) => (on_device, card.as_str()),
        (true, true) => (on_device, PROCESSOR),
        (true, false) => (on_processor, PROCESSOR),
    };
    // The device's class is the card's own, whatever the chat model takes of it now.
    let class = if runs_on_processor {
        gen_eta::device_class(&target, "cpu")
    } else {
        gen_eta::device_class(&chat_counted_back(&target, chat), &backend)
    };
    let measured = match store
        .local_run_times(&entry.id, device, MEASURED_RUNS)
        .await
    {
        Ok(times) => measured_runs(&times),
        Err(e) => {
            tracing::warn!(model = %entry.id, "this computer's measured times could not be read: {e}");
            Vec::new()
        }
    };
    let eta = gen_eta::estimate(entry, size, steps, pictures, &class, &measured);
    let word = gen_eta::fit_word(entry, &fit, eta.as_ref());
    let reason = fit
        .not_for
        .filter(|_| word == FitWord::NotHere)
        .map(|not_for| errors::not_here_reason(not_for, fit.processor_only));
    let need = StudioNeed {
        alongside: on_card(on_device.fit) || (by_choice && !shared_memory),
        after_unload: on_device.after_unload.is_some_and(|f| f != GenFit::Red),
        processor_ok,
        shared_memory,
    };
    LocalVerdict {
        fit,
        eta,
        word,
        card,
        description,
        reason,
        need,
    }
}

/// Whole or one part at a time on the device: nothing of it on the processor.
fn on_card(fit: GenFit) -> bool {
    matches!(fit, GenFit::Green | GenFit::OnePartAtATime)
}

/// `target` with the chat model's `chat` bytes on the device counted back as free.
fn chat_counted_back(target: &Target, chat: Option<u64>) -> Target {
    let back = chat.unwrap_or(0);
    with_device_free(target.clone(), |free| free.saturating_add(back))
}

/// The computer's free memory where the processor runs a model: 0 when the system does not
/// say.
fn free_memory(target: &Target) -> u64 {
    match target {
        Target::Gpu { ram_free_bytes, .. } | Target::AppleUnified { ram_free_bytes, .. } => {
            ram_free_bytes.unwrap_or(0)
        }
        Target::Cpu { ram_free_bytes } => *ram_free_bytes,
    }
}

/// A graphics device as shown: its name ([`device_label`]) with the engine's backend in
/// brackets ("AMD Radeon RX 7600 (Vulkan)", "Apple M5 (Metal)").
fn shown(description: Option<&str>, backend: &str) -> String {
    let label = device_label(description, backend);
    let backend = match backend {
        "cpu" => return label,
        "vulkan" => "Vulkan",
        "metal" => "Metal",
        other => other,
    };
    format!("{label} ({backend})")
}

/// Where models run, by name: the graphics device's ([`device_name`]; "Graphics card" when
/// llama.cpp gives none), or "Processor" for the backend `cpu`.
pub(crate) fn device_label(description: Option<&str>, backend: &str) -> String {
    if backend == "cpu" {
        return PROCESSOR.to_owned();
    }
    description
        .map(device_name)
        .filter(|name| !name.is_empty())
        .unwrap_or("Graphics card")
        .to_owned()
}

/// A graphics device's name from llama.cpp's description of it: the driver's code for the
/// chip that the description may end with, set apart by a space and in brackets (Mesa's
/// "AMD Radeon RX 7600 (RADV NAVI33)", Intel's "… Graphics (DG2)"), is left out. Brackets
/// inside the name ("AMD Radeon(TM) Graphics") are its own.
fn device_name(description: &str) -> &str {
    let name = description.trim();
    let Some(inner) = name.strip_suffix(')') else {
        return name;
    };
    let mut depth = 0_usize;
    for (at, c) in inner.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' if depth > 0 => depth -= 1,
            '(' => {
                let before = &name[..at];
                let kept = before.trim_end();
                return if before.len() > kept.len() && !kept.is_empty() {
                    kept
                } else {
                    name
                };
            }
            _ => {}
        }
    }
    name
}

/// This computer's measured runs, from what each recorded run was made at (its `job`).
fn measured_runs(times: &[store::LocalRunTime]) -> Vec<MeasuredRun> {
    times
        .iter()
        .filter_map(|time| {
            let job = time.params.get("job")?;
            let number = |key: &str| {
                job.get(key)
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
            };
            Some(MeasuredRun {
                size: GenSize {
                    width: number("width")?,
                    height: number("height")?,
                    frames: number("frames").unwrap_or(1),
                },
                steps: number("steps")?,
                pictures: number("pictures").unwrap_or(1),
                elapsed_ms: time.elapsed_ms,
            })
        })
        .collect()
}

/// What a run made at `shape` records of it, for this computer's measured times.
fn job_record(shape: &RunShape) -> Value {
    json!({
        "width": shape.size.width,
        "height": shape.size.height,
        "frames": shape.size.frames,
        "steps": shape.steps,
        "pictures": shape.pictures,
    })
}

/// A time as the webview shows it: rounded as people read it (`gen_eta::round_for_display`),
/// with the time left during a run.
pub(crate) fn eta_dto(eta: &Eta, left_ms: Option<u64>) -> EtaDto {
    EtaDto {
        low_ms: gen_eta::round_for_display(eta.low_ms),
        high_ms: gen_eta::round_for_display(eta.high_ms),
        measured: eta.measured,
        left_ms: left_ms.map(gen_eta::round_for_display),
    }
}

/// The time left in a run that is drawing, from its live seconds a step (the passes of a
/// model that draws in two counted as more steps); `None` before the first step ends.
fn left_of(p: &LocalProgress) -> Option<u64> {
    let sec_per_step = p.sec_per_step?;
    if p.stage != LocalStage::Drawing || p.steps == 0 {
        return None;
    }
    let passes = p.passes.max(1);
    let steps = p.steps.saturating_mul(passes);
    let step = (p.pass.clamp(1, passes) - 1)
        .saturating_mul(p.steps)
        .saturating_add(p.step.min(p.steps));
    let pictures_left = p.pictures.max(1).saturating_sub(p.picture.max(1)) + 1;
    Some(gen_eta::left_ms(
        step,
        steps,
        pictures_left,
        sec_per_step,
        0,
    ))
}

/// A fit word as the webview names it: `fits`, `slow`, `afterUnload` or `notHere`.
pub(crate) fn word_tag(word: FitWord) -> &'static str {
    match word {
        FitWord::Fits => "fits",
        FitWord::Slow => "slow",
        FitWord::AfterUnload => "afterUnload",
        FitWord::NotHere => "notHere",
    }
}

fn stage_tag(stage: LocalStage) -> &'static str {
    match stage {
        LocalStage::Loading => "loading",
        LocalStage::ReadingPrompt => "readingPrompt",
        LocalStage::Drawing => "drawing",
        LocalStage::Decoding => "decoding",
        LocalStage::Saving => "saving",
    }
}

fn wait_tag(wait: LeaseWait) -> &'static str {
    match wait {
        LeaseWait::ReplyRunning => "reply",
        LeaseWait::AnotherStudioJob => "studioJob",
        LeaseWait::NeedsAnswer => "answer",
        LeaseWait::ChatModelLoading => "chatModelLoading",
    }
}

/// Where a run on this computer goes, as the run records it.
fn this_computer() -> Endpoint {
    Endpoint {
        id: None,
        name: THIS_COMPUTER.to_owned(),
        host: String::new(),
        company: Company::Local,
    }
}

/// How a local engine job's failure is recorded: as the engine named it, never charged.
fn local_ending(e: &MediaError, cap: u64) -> Ending {
    let error = match e {
        MediaError::Answered(f) => errors::from_failure(Company::Local, f),
        MediaError::Local(io) => errors::from_local(io, cap),
        MediaError::NotSent(why) => AppError {
            details: why.to_string(),
            ..errors::not_sent(Company::Local)
        },
        MediaError::Unknown(why) => AppError {
            details: why.to_string(),
            ..errors::not_answered(Company::Local, Some(false))
        },
    };
    Ending::failed(error, Some(false))
}

/// Removes a file, if it is there.
async fn remove_file(path: &Path) {
    match tokio::fs::remove_file(path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("a local Studio result could not be removed: {e}"),
    }
}

/// The run's audit row: as a cloud run's (the model, the settings set, the prompt's length but
/// never the prompt), on this computer, with the catalog id, the revision and the engine's
/// build.
fn audit_entry(
    req: &StartRequest,
    model: &MediaModel,
    request: &MediaRequest,
    row: &store::GenModel,
    revision: &str,
    engine_build: Option<u32>,
    at: i64,
) -> store::AuditEntry {
    let what = match model.kind {
        MediaKind::Image => "a picture",
        MediaKind::Video => "a video",
    };
    let extra_fields: Vec<&String> = req.extra.keys().collect();
    store::AuditEntry {
        ts: Some(at),
        chat_id: None,
        server: "studio".to_owned(),
        tool: tool_of(model.kind).to_owned(),
        arguments: json!({
            "endpoint": THIS_COMPUTER,
            "host": "",
            "model": model.id,
            "changed": request.params,
            "extra_fields": extra_fields,
            "pictures_sent": request.refs.len(),
            "estimate_usd": 0.0,
            "confirmed": false,
            "prompt_chars": req.prompt.chars().count(),
            "runsOn": "local",
            "catalog_id": row.catalog_id,
            "revision": revision,
            "engine_build": engine_build,
        }),
        preview: format!("Made {what} on this computer with {}", model.display_name),
        risk: "normal".to_owned(),
        decision: store::AuditDecision::AllowedWithoutPrompt,
        decided_by: "user".to_owned(),
        note: None,
        parent_audit_id: None,
        result: None,
    }
}

impl StudioManager {
    /// Whether a local run is queued or running: quitting asks first, and an update waits.
    pub fn local_running(&self) -> bool {
        let lanes = self.inner.lanes();
        lanes.running.values().any(|r| r.lane == LaneKey::Local)
            || lanes
                .queues
                .get(&LaneKey::Local)
                .is_some_and(|queue| !queue.is_empty())
    }

    /// The answer to "Unload the chat model?" for local run `run_id` under
    /// `GpuPolicy::Ask`: its task asks the lease again with it. A run that is not waiting for
    /// an answer is given as it is.
    pub async fn answer_unload(&self, run_id: &str, unload: bool) -> AppResult<RunDto> {
        self.inner.give_answer(run_id, unload);
        let run = self
            .inner
            .deps
            .store
            .get_creation_run(run_id)
            .await?
            .ok_or_else(|| AppError::not_found("run"))?;
        Ok(self.inner.dto_of(&run))
    }

    /// *Quit and stop*: stops every local run, those waiting first, so none starts after the
    /// one running.
    pub async fn stop_local(&self) {
        let ids: Vec<String> = {
            let lanes = self.inner.lanes();
            let waiting = lanes
                .queues
                .get(&LaneKey::Local)
                .into_iter()
                .flatten()
                .map(|job| job.run_id.clone());
            let running = lanes
                .running
                .iter()
                .filter(|(_, r)| r.lane == LaneKey::Local)
                .map(|(id, _)| id.clone());
            waiting.chain(running).collect()
        };
        for id in ids {
            if let Err(e) = self.cancel(&id).await {
                tracing::warn!(run = %id, code = %e.code, "a local Studio run was not stopped");
            }
        }
    }

    /// Records a run on this computer and queues it (see the module documentation).
    pub(super) async fn start_local(&self, req: StartRequest) -> AppResult<RunDto> {
        let inner = &self.inner;
        let lane = inner
            .deps
            .local
            .clone()
            .ok_or_else(errors::no_local_engine)?;
        let _starting = inner.starting.lock().await;
        if let Some(run) = inner.known_press(&req.client_token).await? {
            return Ok(inner.dto_of(&run));
        }
        if prompt_check::refuses(&req.prompt) {
            return Err(errors::wont_make());
        }
        if req.prompt.chars().count() > PROMPT_MAX_CHARS {
            return Err(errors::prompt_too_long_here());
        }
        let prepared = inner.prepare_local(&req, &lane).await?;
        if !prepared.problems.is_empty() {
            return Err(errors::invalid_settings(
                &prepared.problems,
                &prepared.model,
            ));
        }
        let shape = prepared.shape.clone()?;
        let verdict = prepared
            .verdict
            .clone()
            .ok_or_else(|| AppError::internal("a local run without its fit"))?;
        if verdict.word == FitWord::NotHere {
            return Err(errors::does_not_fit(verdict.reason));
        }
        let job = inner
            .record_local(&req, prepared, shape, &verdict, &lane)
            .await?;
        inner.remember_press(&req.client_token, &job.run_id);
        inner.enqueue(job).await
    }

    /// What a run on this computer would take: a time, not a price; its fit and its word; the
    /// settings it would refuse, the prompt's length among them. Records and starts nothing.
    pub(super) async fn estimate_local(&self, req: &StartRequest) -> AppResult<EstimateDto> {
        let inner = &self.inner;
        let lane = inner
            .deps
            .local
            .clone()
            .ok_or_else(errors::no_local_engine)?;
        let prepared = inner.prepare_local(req, &lane).await?;
        let prefs = inner.prefs();
        let spend = inner.spend_today().await?;
        // Free: it never asks and always passes the limit.
        let billing = spend::billing_of(&Price::Free, Some(0.0));
        let mut invalid = errors::invalid_sentences(&prepared.problems, &prepared.model);
        if req.prompt.chars().count() > PROMPT_MAX_CHARS {
            invalid.push(errors::PROMPT_TOO_LONG_HERE.to_owned());
        }
        let verdict = prepared.verdict.as_ref();
        Ok(EstimateDto {
            usd: Some(0.0),
            reasons: spend::confirm_reasons(prepared.model.kind, billing, &prefs.confirm_rules()),
            confirm_at_usd: prefs.confirm_at_usd,
            limit: LimitDto {
                limit_usd: prefs.daily_limit_usd,
                spent_today_usd: spend.settled_usd,
                pending_usd: spend.pending_usd,
                would_pass: spend::check_limit(prefs.daily_limit_usd, &spend, billing).is_ok(),
            },
            invalid,
            eta: verdict
                .and_then(|v| v.eta.as_ref())
                .map(|eta| eta_dto(eta, None)),
            local: verdict.map(|v| LocalEstimateDto {
                fit: word_tag(v.word).to_owned(),
                unload_chat: gen_eta::running_fit(&v.fit).1,
                very_slow: gen_eta::very_slow(v.word, v.eta.as_ref()),
                over_15_min: gen_eta::over_15_min(v.word, v.eta.as_ref()),
                reason: v.reason.clone(),
            }),
        })
    }

    /// The exact command a run on this computer would use, with the options it would get (a
    /// grant beside the chat model, or after its unload when the run needs that). Starts
    /// nothing.
    pub(super) async fn preview_local(&self, req: &StartRequest) -> AppResult<RequestPreview> {
        let inner = &self.inner;
        let lane = inner
            .deps
            .local
            .clone()
            .ok_or_else(errors::no_local_engine)?;
        let prepared = inner.prepare_local(req, &lane).await?;
        if !prepared.problems.is_empty() {
            return Err(errors::invalid_settings(
                &prepared.problems,
                &prepared.model,
            ));
        }
        prepared.shape?;
        let verdict = prepared
            .verdict
            .ok_or_else(|| AppError::internal("a local run without its fit"))?;
        let grant = if verdict.word == FitWord::AfterUnload {
            Grant::AfterUnload { unloaded: None }
        } else {
            Grant::Alongside
        };
        let opts = job_options(
            &grant,
            &verdict.fit,
            &prepared.request,
            verdict.description.as_deref(),
            lane.hardware.logical_cores(),
        );
        // The engine's devices first (once per engine, as a run lists them before it starts),
        // so a first command names the graphics card the run would pass.
        lane.provider.list_devices_for(&opts).await;
        Ok(lane.provider.preview_local(&prepared.request, &opts))
    }
}

impl Inner {
    /// `req` checked against its installed model: the model from the provider's list (it
    /// reads the store, never the network), the seed added, the settings checked, the job's
    /// shape and its verdict.
    async fn prepare_local(
        &self,
        req: &StartRequest,
        lane: &LocalLane,
    ) -> AppResult<LocalPrepared> {
        let raw = lane
            .provider
            .fetch_catalog()
            .await
            .map_err(|e| local_ending(&e, self.deps.limits.picture_cap).error)?;
        let model = parse_catalog(Company::Local, &raw, &BTreeMap::new())
            .into_iter()
            .find(|m| m.id == req.model)
            .ok_or_else(errors::local_model_missing)?;
        if req.kind != model.kind {
            return Err(errors::wrong_kind(model.kind));
        }
        let (row, entry) = lane
            .provider
            .entry_of(&model.id)
            .await
            .ok_or_else(errors::local_model_missing)?;
        let mut params = req.params.clone();
        let seed = pick_seed(&model, &mut params);
        let request = MediaRequest {
            kind: model.kind,
            model: model.id.clone(),
            prompt: req.prompt.clone(),
            params,
            extra: req.extra.clone(),
            refs: self.refs_of(req)?,
            routing: None,
            provider_options: BTreeMap::new(),
        };
        let mut problems = validate(&model, &request).err().unwrap_or_default();
        // A local prompt has its own, shorter limit.
        problems.retain(|p| !matches!(p, Invalid::PromptTooLong));
        let shape = run_shape(&entry, &request)
            .map_err(|e| local_ending(&e, self.deps.limits.picture_cap).error);
        let verdict = match &shape {
            Ok(shape) => Some(
                verdict(
                    &entry,
                    *shape,
                    runs_on_processor(&request),
                    lane.hardware.as_ref(),
                    &self.deps.store,
                )
                .await,
            ),
            Err(_) => None,
        };
        Ok(LocalPrepared {
            model,
            row,
            entry,
            request,
            seed,
            problems,
            shape,
            verdict,
        })
    }

    /// Records the local run (`queued`, $0) and its audit row, and gives its job.
    async fn record_local(
        &self,
        req: &StartRequest,
        prepared: LocalPrepared,
        shape: RunShape,
        verdict: &LocalVerdict,
        lane: &LocalLane,
    ) -> AppResult<Job> {
        let LocalPrepared {
            model,
            row,
            entry,
            request,
            seed,
            ..
        } = prepared;
        let run_id = uuid::Uuid::new_v4().to_string();
        let created_at = self.deps.clock.now_ms();
        let engine_build = lane.provider.engine_build();
        let revision = entry.diffusion().revision.clone();
        let endpoint = this_computer();
        let run = store::NewCreationRun {
            id: run_id.clone(),
            kind: creation_kind(model.kind),
            runs_on: store::RunsOn::Local,
            provider_id: None,
            provider_name: endpoint.name.clone(),
            provider_host: endpoint.host.clone(),
            model: model.id.clone(),
            model_label: model.display_name.clone(),
            prompt: req.prompt.clone(),
            params: json!({
                "params": request.params,
                "extra": request.extra,
                "routing": Value::Null,
                "job": job_record(&shape),
            }),
            refs: refs_json(&request.refs),
            estimate_usd: Some(0.0),
            gen_model_id: Some(row.id.clone()),
            engine_build,
            // Written once the grant decides where it runs.
            device: None,
            created_at,
        };
        let entry_row = audit_entry(
            req,
            &model,
            &request,
            &row,
            &revision,
            engine_build,
            created_at,
        );
        let provider: Arc<dyn MediaProvider> = lane.provider.clone();
        let job = Job {
            run_id,
            created_at,
            kind: model.kind,
            lane: LaneKey::Local,
            provider,
            endpoint,
            seed,
            audit_id: None,
            step: Step::Local(Box::new(LocalStep {
                request,
                entry,
                shape,
                revision,
            })),
        };
        let job = self
            .insert_and_audit(run, entry_row, job, store::CreationPatch::default())
            .await?;
        self.with_live(&job.run_id, |live| live.eta = verdict.eta);
        Ok(job)
    }

    /// A local run's task: the run is made (see the module documentation), then its live
    /// state is forgotten and, once the local queue is empty, the chat model a grant
    /// unloaded is loaded again when the preferences say so.
    pub(super) async fn run_local(&self, t: &Task<'_>, step: &LocalStep) {
        let Some(lane) = self.deps.local.clone() else {
            // Never: a local run is recorded only with a lane.
            let ending = Ending::failed(errors::no_local_engine(), Some(false));
            self.end_run(t.job, ending, None).await;
            return;
        };
        // The hold on the card is dropped when this returns.
        self.make_local(t, step, &lane).await;
        self.forget_live(&t.job.run_id);
        self.after_local(&lane);
    }

    /// "Reload it after Studio": when no local run waits, the chat model a grant unloaded
    /// is loaded again, once (in a task of its own: the lane does not wait for it), when the
    /// preferences say so; either way it is no longer kept.
    fn after_local(&self, lane: &LocalLane) {
        if self.root.is_cancelled() {
            return;
        }
        let empty = self
            .lanes()
            .queues
            .get(&LaneKey::Local)
            .is_none_or(VecDeque::is_empty);
        if !empty {
            return;
        }
        let unloaded = self
            .local_unloaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(path) = unloaded
            && self.prefs().reload_chat_after
        {
            let lease = Arc::clone(&lane.lease);
            tauri::async_runtime::spawn(async move { lease.reload_chat(&path).await });
        }
    }

    /// Makes the run: the fit again, the card, `running`, the engine job; records how it
    /// ended (except at a Shutdown).
    async fn make_local(&self, t: &Task<'_>, step: &LocalStep, lane: &LocalLane) {
        let job = t.job;
        let row = match self.deps.store.get_creation_run(&job.run_id).await {
            Ok(Some(row)) => row,
            Ok(None) => return,
            Err(e) => {
                let ending = Ending::failed(e.into(), Some(false));
                self.end_run(job, ending, None).await;
                return;
            }
        };
        // Read again now: the chat model, or what is free, may have changed while it waited.
        let verdict = verdict(
            &step.entry,
            step.shape,
            runs_on_processor(&step.request),
            lane.hardware.as_ref(),
            &self.deps.store,
        )
        .await;
        self.with_live(&job.run_id, |live| live.eta = verdict.eta);
        if verdict.word == FitWord::NotHere {
            let ending = Ending::failed(errors::does_not_fit(verdict.reason), Some(false));
            self.end_run(job, ending, None).await;
            return;
        }
        let card = self.take_card(t, lane, &verdict.need, &row).await;
        // It no longer waits for anything, however that ended.
        self.with_live(&job.run_id, |live| live.waiting_for = None);
        let (grant, _hold) = match card {
            Card::Granted(grant, hold) => (grant, hold),
            Card::DoesNotFit => {
                let ending = Ending::failed(errors::does_not_fit(verdict.reason), Some(false));
                self.end_run(job, ending, None).await;
                return;
            }
            Card::NotStarted => {
                let ending = Ending::failed(errors::not_sent(Company::Local), Some(false));
                self.end_run(job, ending, None).await;
                return;
            }
            Card::Stopped => {
                self.end_run(job, Ending::stopped(job, Some(false)), None)
                    .await;
                return;
            }
            Card::Shutdown => return,
        };
        let chat_loaded = lane.hardware.chat_device_bytes().is_some();
        let unloaded = match &grant {
            Grant::AfterUnload { unloaded } | Grant::UseProcessor { unloaded } => unloaded.clone(),
            Grant::Alongside => None,
        };
        if let Some(path) = &unloaded {
            *self
                .local_unloaded
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(path.clone());
        }
        let opts = job_options(
            &grant,
            &verdict.fit,
            &step.request,
            verdict.description.as_deref(),
            lane.hardware.logical_cores(),
        );
        let device = if opts.processor_only {
            PROCESSOR.to_owned()
        } else {
            verdict.card.clone()
        };
        self.with_live(&job.run_id, |live| {
            live.unloaded_chat = unloaded;
            live.alongside = grant == Grant::Alongside && chat_loaded;
            live.on_processor = opts.processor_only;
        });
        let started = self.deps.clock.now_ms();
        let running = store::CreationPatch {
            status: Some(store::CreationStatus::Running),
            started_at: Some(Some(started)),
            device: Some(Some(device)),
            ..Default::default()
        };
        if let Err(e) = self
            .deps
            .store
            .update_creation_run(&job.run_id, running)
            .await
        {
            // Never start a run whose start is not recorded.
            tracing::warn!(run = %job.run_id, "a local Studio run could not be marked as running: {e}");
            let ending = Ending::failed(errors::not_sent(Company::Local), Some(false));
            self.end_run(job, ending, None).await;
            return;
        }
        self.emit(&job.run_id).await;
        if self.root.is_cancelled() {
            return;
        }
        if t.token.is_cancelled() {
            self.end_run(job, Ending::stopped(job, Some(false)), Some(started))
                .await;
            return;
        }
        let clock = Instant::now();
        self.with_live(&job.run_id, |live| live.started = Some(clock));
        let begun = self
            .until_stopped(
                t.token,
                lane.provider.start_local(&step.request, &t.scratch, &opts),
            )
            .await;
        let remote_id = match begun {
            // Dropped before it began: nothing runs.
            Waited::Shutdown => return,
            Waited::Stopped => {
                self.end_run(job, Ending::stopped(job, Some(false)), Some(started))
                    .await;
                return;
            }
            Waited::Got(Ok(Started::Job { remote_id })) => remote_id,
            Waited::Got(Ok(Started::Done { outputs, .. })) => {
                let outputs = outputs
                    .into_iter()
                    .map(|saved| (uuid::Uuid::new_v4().to_string(), saved))
                    .collect();
                self.keep_local(t, step, outputs, started, clock).await;
                return;
            }
            Waited::Got(Err(e)) => {
                let ending = local_ending(&e, self.cap_of(job.kind));
                self.end_run(job, ending, Some(started)).await;
                return;
            }
        };
        let with_id = store::CreationPatch {
            remote_id: Some(Some(remote_id.clone())),
            ..Default::default()
        };
        if let Err(e) = self
            .deps
            .store
            .update_creation_run(&job.run_id, with_id)
            .await
        {
            tracing::warn!(run = %job.run_id, "a local Studio run's id was not recorded: {e}");
        }
        self.watch_local(t, step, lane, &remote_id, started, clock)
            .await;
    }

    /// The graphics card for run `row` (its task `t`): asked of the lease with the policy as
    /// it is now, `queued` with what it waits for meanwhile, and under Ask again with the
    /// person's answer.
    async fn take_card(
        &self,
        t: &Task<'_>,
        lane: &LocalLane,
        need: &StudioNeed,
        row: &store::CreationRun,
    ) -> Card {
        let run_id = &t.job.run_id;
        let on_wait = |wait: LeaseWait| {
            self.with_live(run_id, |live| live.waiting_for = Some(wait));
            self.deps.sink.progress(&self.dto_of(row));
        };
        let mut answered = None;
        loop {
            let policy = self.prefs().gpu_policy;
            let asked = tokio::select! {
                biased;
                asked = lane.lease.acquire(need, policy, answered, t.token, &on_wait) => asked,
                () = self.root.cancelled() => return Card::Shutdown,
            };
            match asked {
                Ok((grant, hold)) => return Card::Granted(grant, hold),
                Err(Refused::DoesNotFit) => return Card::DoesNotFit,
                Err(Refused::Cancelled) if self.root.is_cancelled() => return Card::Shutdown,
                Err(Refused::Cancelled) if t.token.is_cancelled() => return Card::Stopped,
                Err(Refused::Cancelled) => return Card::NotStarted,
                Err(Refused::NeedsAnswer) => match self.answer_for(run_id, t.token).await {
                    Waited::Got(answer) => answered = Some(answer),
                    Waited::Stopped => return Card::Stopped,
                    Waited::Shutdown => return Card::Shutdown,
                },
            }
        }
    }

    /// Waits for the answer to "Unload the chat model?" for run `run_id`.
    async fn answer_for(
        &self,
        run_id: &str,
        token: &tokio_util::sync::CancellationToken,
    ) -> Waited<bool> {
        loop {
            let (answer, answered) = self.with_live(run_id, |live| {
                (live.answer.take(), Arc::clone(&live.answered))
            });
            if let Some(answer) = answer {
                return Waited::Got(answer);
            }
            tokio::select! {
                biased;
                () = token.cancelled() => return Waited::Stopped,
                () = self.root.cancelled() => return Waited::Shutdown,
                () = answered.notified() => {}
            }
        }
    }

    /// Gives run `run_id`'s task the answer to "Unload the chat model?", when it was asked
    /// (the question is shown only then).
    fn give_answer(&self, run_id: &str, unload: bool) {
        let mut live = self
            .local_live
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(live) = live
            .get_mut(run_id)
            .filter(|live| live.waiting_for == Some(LeaseWait::NeedsAnswer))
        {
            live.answer = Some(unload);
            live.answered.notify_one();
        }
    }

    /// Checks engine job `remote_id` every [`LOCAL_CHECK_EVERY`] until it is done (then its
    /// results are kept), fails or is gone, or the run is stopped (the engine is killed) or
    /// the app closes (the engine is killed, and nothing is recorded).
    async fn watch_local(
        &self,
        t: &Task<'_>,
        step: &LocalStep,
        lane: &LocalLane,
        remote_id: &str,
        started: i64,
        clock: Instant,
    ) {
        let job = t.job;
        let provider = &lane.provider;
        let cap = self.cap_of(job.kind);
        // Its `running` event was just sent.
        let mut last_event = Instant::now();
        loop {
            match self
                .until_stopped(t.token, tokio::time::sleep(LOCAL_CHECK_EVERY))
                .await
            {
                Waited::Got(()) => {}
                Waited::Stopped => {
                    stop_engine(provider.as_ref(), remote_id).await;
                    self.end_run(job, Ending::stopped(job, Some(false)), Some(started))
                        .await;
                    return;
                }
                Waited::Shutdown => {
                    stop_engine(provider.as_ref(), remote_id).await;
                    return;
                }
            }
            let state = provider.check(remote_id).await;
            let status = provider.status(remote_id);
            let progress = match state {
                Ok(JobState::Pending) => None,
                Ok(JobState::Running { progress }) => progress.map(f64::from),
                Ok(JobState::Done(done)) => {
                    self.with_live(&job.run_id, |live| live.status = status);
                    let outputs = self
                        .move_results(t, provider.as_ref(), remote_id, &done)
                        .await;
                    provider.release(remote_id).await;
                    match outputs {
                        Ok(outputs) => self.keep_local(t, step, outputs, started, clock).await,
                        Err(e) => {
                            self.end_run(job, local_ending(&e, cap), Some(started))
                                .await;
                        }
                    }
                    return;
                }
                Ok(JobState::Failed(f)) => {
                    provider.release(remote_id).await;
                    let ending =
                        Ending::failed(errors::from_failure(Company::Local, &f), Some(false));
                    self.end_run(job, ending, Some(started)).await;
                    return;
                }
                Ok(JobState::Gone) => {
                    provider.release(remote_id).await;
                    let ending = Ending::failed(errors::job_lost(Company::Local), Some(false));
                    self.end_run(job, ending, Some(started)).await;
                    return;
                }
                Err(e) => {
                    provider.release(remote_id).await;
                    self.end_run(job, local_ending(&e, cap), Some(started))
                        .await;
                    return;
                }
            };
            // Still going: its progress, written and sent at most every PROGRESS_EVERY.
            let changed = self.with_live(&job.run_id, |live| {
                let changed = live.status != status;
                live.status = status;
                changed
            });
            if changed && last_event.elapsed() >= PROGRESS_EVERY {
                let patch = store::CreationPatch {
                    progress: Some(progress),
                    ..Default::default()
                };
                if let Err(e) = self
                    .deps
                    .store
                    .update_creation_run(&job.run_id, patch)
                    .await
                {
                    tracing::debug!(run = %job.run_id, "a local Studio run's progress was not written: {e}");
                }
                self.emit(&job.run_id).await;
                last_event = Instant::now();
            }
        }
    }

    /// Moves each result of done engine job `remote_id` out of the run's folder to
    /// `.partial/<output id>`. On a failure, the results moved so far go.
    async fn move_results(
        &self,
        t: &Task<'_>,
        provider: &dyn crate::studio::local::LocalProvider,
        remote_id: &str,
        done: &JobDone,
    ) -> Result<Vec<(String, Saved)>, MediaError> {
        let partial = t.gallery.partial_dir();
        let mut moved: Vec<(String, Saved)> = Vec::new();
        for index in 0..done.outputs {
            let output_id = uuid::Uuid::new_v4().to_string();
            let dest = partial.join(&output_id);
            let failed = match provider.download(remote_id, done, index, &dest).await {
                Ok(Fetched::Saved(saved)) => {
                    moved.push((output_id, saved));
                    continue;
                }
                Ok(Fetched::Elsewhere(_)) => MediaError::Local(agent::media::LocalIo::Io(
                    "the engine's result is not on this computer".to_owned(),
                )),
                Err(e) => e,
            };
            for (_, saved) in &moved {
                remove_file(&saved.path).await;
            }
            remove_file(&dest).await;
            return Err(failed);
        }
        Ok(moved)
    }

    /// Keeps the results of the run: each labelled as AI-made where it waits (so no local
    /// file reaches the gallery without its label), then moved into the gallery with its
    /// thumbnail and row as a cloud picture is; then the time it took, and `done` at $0. A
    /// failure removes what is left and ends the run failed, not charged.
    async fn keep_local(
        &self,
        t: &Task<'_>,
        step: &LocalStep,
        outputs: Vec<(String, Saved)>,
        started: i64,
        clock: Instant,
    ) {
        let job = t.job;
        let cap = self.cap_of(job.kind);
        for (at, (output_id, saved)) in outputs.iter().enumerate() {
            let idx = u32::try_from(at).unwrap_or(u32::MAX);
            let kept = match self.label(step, output_id, saved).await {
                Ok(()) => {
                    self.keep_one(job, &t.gallery, output_id.clone(), idx, saved, cap)
                        .await
                }
                Err(e) => Err(e),
            };
            if let Err(error) = kept {
                for (_, left) in &outputs[at..] {
                    remove_file(&left.path).await;
                }
                self.end_run(job, Ending::failed(error, Some(false)), Some(started))
                    .await;
                return;
            }
        }
        let elapsed = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
        let took = store::CreationPatch {
            elapsed_ms: Some(Some(elapsed)),
            ..Default::default()
        };
        if let Err(e) = self.deps.store.update_creation_run(&job.run_id, took).await {
            tracing::warn!(run = %job.run_id, "a local Studio run's time was not recorded: {e}");
        }
        self.finish(job, &t.gallery, Some(0.0), Some(started)).await;
    }

    /// Writes the AI label into result `saved` (output `output_id`): the model, its revision,
    /// the output's id and the time now, never the prompt.
    async fn label(&self, step: &LocalStep, output_id: &str, saved: &Saved) -> AppResult<()> {
        let created = Timestamp::from_millisecond(self.deps.clock.now_ms())
            .map(|at| at.to_string())
            .unwrap_or_default();
        let label = AiLabel {
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            model: step.entry.name.clone(),
            revision: step.revision.clone(),
            id: output_id.to_owned(),
            created,
        };
        let (path, mime) = (saved.path.clone(), saved.mime.clone());
        tokio::task::spawn_blocking(move || label_file(&path, &mime, &label))
            .await
            .map_err(AppError::internal)?
            .map_err(|e| errors::from_gallery(&e))
    }

    /// Records a local run the app left queued or running as `interrupted` (a local job
    /// cannot outlive the app), not charged, and removes its `.partial/<run>`.
    pub(super) async fn interrupt_local(&self, run: &store::CreationRun) {
        let ending = Ending {
            status: store::CreationStatus::Interrupted,
            ..Ending::failed(errors::interrupted(Company::Local), Some(false))
        };
        self.record_end(
            &run.id,
            Company::Local,
            run.audit_id,
            ending,
            run.started_at,
        )
        .await;
        remove_scratch(&self.gallery(), &run.id).await;
    }

    /// Changes run `run_id`'s live state (made when it is missing) and gives what `change`
    /// returns.
    fn with_live<T>(&self, run_id: &str, change: impl FnOnce(&mut LocalLive) -> T) -> T {
        let mut live = self
            .local_live
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        change(live.entry(run_id.to_owned()).or_default())
    }

    /// Forgets run `run_id`'s live state, once Studio no longer follows it.
    pub(super) fn forget_live(&self, run_id: &str) {
        self.local_live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(run_id);
    }

    /// A local run's card and time: from its row (the device, the engine's build, the time
    /// it took, its label) and, while Studio follows it, its live state.
    pub(super) fn fill_local(&self, run: &store::CreationRun, dto: &mut RunDto) {
        if run.runs_on != store::RunsOn::Local {
            return;
        }
        let live = self
            .local_live
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let live = live.get(&run.id);
        let status = live.and_then(|l| l.status.as_ref());
        let progress = status.and_then(|s| s.progress.as_ref());
        let elapsed = live
            .and_then(|l| l.started)
            .map(|at| u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX));
        dto.local = Some(LocalRunDto {
            stage: progress.map(|p| stage_tag(p.stage).to_owned()),
            step: progress.map(|p| p.step),
            steps: progress.map(|p| p.steps),
            picture: progress.map(|p| p.picture),
            pictures: progress.map(|p| p.pictures),
            pass: progress.map(|p| p.pass),
            passes: progress.map(|p| p.passes),
            waiting_for: live
                .and_then(|l| l.waiting_for)
                .map(|wait| wait_tag(wait).to_owned()),
            unloaded_chat: live.and_then(|l| l.unloaded_chat.clone()),
            retrying: status.is_some_and(|s| s.retrying),
            stalled: status.is_some_and(|s| s.stalled),
            device: run.device.clone(),
            engine_build: run.engine_build,
            elapsed_ms: run.elapsed_ms.or(elapsed),
            labelled: run.status == store::CreationStatus::Done && !run.outputs.is_empty(),
            alongside: live.is_some_and(|l| l.alongside),
            on_processor: live.is_some_and(|l| l.on_processor),
        });
        dto.eta = live
            .and_then(|l| l.eta.as_ref())
            .map(|eta| eta_dto(eta, progress.and_then(left_of)));
    }
}

/// Kills engine job `remote_id`'s whole tree and removes its folder.
async fn stop_engine(provider: &dyn crate::studio::local::LocalProvider, remote_id: &str) {
    if let Err(e) = provider.cancel(remote_id).await {
        tracing::warn!("a local Studio run's engine was not stopped: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ruling L15-x-B9-device: a device is shown by its name with the backend in brackets,
    /// never with the driver's code for its chip.
    #[test]
    fn a_device_is_shown_by_its_name_and_backend() {
        let vulkan = |description: &str| shown(Some(description), "vulkan");
        assert_eq!(
            vulkan("AMD Radeon RX 7600 (RADV NAVI33)"),
            "AMD Radeon RX 7600 (Vulkan)"
        );
        assert_eq!(
            vulkan("Intel(R) Arc(tm) A770 Graphics (DG2)"),
            "Intel(R) Arc(tm) A770 Graphics (Vulkan)"
        );
        assert_eq!(
            vulkan("llvmpipe (LLVM 17.0.6, 256 bits)"),
            "llvmpipe (Vulkan)"
        );
        assert_eq!(vulkan("Card (rev (a1))"), "Card (Vulkan)");
        // Brackets that are part of the name stay.
        assert_eq!(
            vulkan("NVIDIA GeForce RTX 4070"),
            "NVIDIA GeForce RTX 4070 (Vulkan)"
        );
        assert_eq!(
            vulkan("AMD Radeon(TM) Graphics"),
            "AMD Radeon(TM) Graphics (Vulkan)"
        );
        assert_eq!(vulkan("AMD Radeon(TM)"), "AMD Radeon(TM) (Vulkan)");
        // A description that is nothing but brackets is all there is to name it by.
        assert_eq!(vulkan("(RADV NAVI33)"), "(RADV NAVI33) (Vulkan)");
        assert_eq!(shown(Some("Apple M5"), "metal"), "Apple M5 (Metal)");
        assert_eq!(vulkan("  "), "Graphics card (Vulkan)");
        assert_eq!(shown(None, "vulkan"), "Graphics card (Vulkan)");
        assert_eq!(
            shown(Some("AMD Radeon RX 7600 (RADV NAVI33)"), "cpu"),
            PROCESSOR
        );
    }
}
