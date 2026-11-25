//! Will a picture or video model fit this computer (studio-local-design.md L5, "Fit")? The
//! model's parts (D: the diffusion part, or parts used one after the other; T: the text
//! encoders; V: the decoder) and the working memory a run needs besides, from the entry's
//! measured peaks or its family's fallback coefficients, compared with the [`Room`] the
//! target gives: now, and with the chat model unloaded.
//!
//! - **Green:** D + T + V + max(act_dit, act_vae) + [`SPARE_BYTES`] fits on the device
//!   (`room.whole`). A measured peak replaces D + T + V + max(act_dit, act_vae).
//! - **One part at a time** (memory saving): max(T, D + act_dit, V + act_vae) + the spare
//!   fits on the device, with the decoder tiled, and every part's weights fit the
//!   computer's memory less [`RAM_RESERVE_BYTES`](crate::fit::RAM_RESERVE_BYTES), where
//!   they wait their turn.
//! - **Partly on the processor:** D + act_dit fits `room.partial`.
//! - **Red** otherwise.
//!
//! On the processor ([`Target::Cpu`]) every part shares the one memory, so a run is Green
//! or Red: nothing takes turns there. On Apple silicon the working set is a share of the
//! computer's memory, so a verdict other than Red also needs every part and the working
//! memory (the Green need without the spare) to fit the computer's memory less the reserve.

use serde::Serialize;

use crate::fit::{RAM_RESERVE_BYTES, Room, SPARE_BYTES, Target};
use crate::gen_catalog::{GenEntry, GenKind, GenRole, MemoryPoint};

/// A picture's preset sides (its shorter side), for [`GenFitResult::suggestion`].
pub const PICTURE_SIDES: [u32; 3] = [512, 768, 1024];
/// A clip's preset lengths in frames (each one 4n + 1 and 8n + 1).
pub const VIDEO_FRAMES: [u32; 5] = [17, 33, 49, 81, 121];
/// A clip's preset shorter sides: 480p and 720p.
const VIDEO_SIDES: [u32; 2] = [480, 720];
/// The shapes Studio offers, width to height, in the order they are offered; a suggestion
/// keeps the requested one.
pub const SHAPES: [(u32, u32); 5] = [(1, 1), (4, 3), (3, 4), (16, 9), (9, 16)];
/// About how much of a Mac's memory macOS lets Metal use (its working set), as a fraction:
/// two thirds. A Mac's memory is told from its working set by it
/// ([`device_class`](crate::gen_eta::device_class)), and a working set a run needs is turned
/// into the Mac's memory by it ([`NotFor::MacMemory`]).
pub(crate) const MAC_WORKING_SET_SHARE: (u64, u64) = (2, 3);

/// The size of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenSize {
    pub width: u32,
    pub height: u32,
    /// 1 for a picture.
    pub frames: u32,
}

impl GenSize {
    /// Pixels times frames: what the decoder turns out, and how presets are ranked.
    pub(crate) fn pixel_frames(self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * u64::from(self.frames.max(1))
    }
}

/// How a run would use this computer's memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GenFit {
    /// Every part and the run's working memory on the device at once.
    Green,
    /// One part at a time on the device while the others wait in the computer's memory,
    /// the decoder tiled (memory saving).
    OnePartAtATime,
    /// Only with part of the diffusion model on the processor: slow.
    PartlyOnProcessor,
    /// Does not fit.
    Red,
}

impl GenFit {
    /// Whole or one part at a time on the device: nothing runs on the processor.
    pub(crate) fn on_device(self) -> bool {
        matches!(self, Self::Green | Self::OnePartAtATime)
    }
}

/// Why a model is not offered here, judged with the chat model's memory counted back, so the
/// reason never blames the chat model's share. Each `needs` figure has one unit, is enough to
/// run the job, and is always more than this computer has in that unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotFor {
    /// The free memory the system would have to report where this computer runs models (the
    /// figure [`GenFitResult::available_bytes`] is) to run it. On a graphics card, the card's
    /// free memory for one part at a time: the largest part with its working memory, and the
    /// spare. On a computer that runs models on the processor, where nothing takes turns, the
    /// computer's free memory for every part and the working memory at once, with the spare
    /// and the reserve it keeps back for other programs.
    GraphicsMemory { needs: u64 },
    /// The memory a Mac would have to have to run it one part at a time: the larger of the
    /// Mac whose working set (two thirds of its memory, [`MAC_WORKING_SET_SHARE`]) holds the
    /// largest part with its working memory and the spare, and the Mac whose memory holds
    /// every part and the working memory besides the reserve (its free memory counted as all
    /// of it).
    MacMemory { needs: u64 },
    /// Video needs a graphics card or Apple silicon; this computer runs models on the
    /// processor.
    VideoNeedsGpu,
    /// The catalog keeps this video model off Apple silicon (`runs_on_apple`).
    VideoNotOnThisMac,
}

/// The fit of one model at one size on this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenFitResult {
    /// With the memory free now (the chat model loaded, if one is).
    pub fit: GenFit,
    /// With the chat model unloaded: only when one is loaded and `fit` is not Green.
    pub after_unload: Option<GenFit>,
    /// Everything on the device at once, without the spare: D + T + V + the larger working
    /// memory, or the measured peak.
    pub needed_bytes: u64,
    /// The free memory there as the system reports it: the graphics card's, the Metal
    /// working set, or the computer's before the reserve is kept back.
    pub available_bytes: u64,
    /// `fit` runs with memory saving ([`needs_memory_saving`]).
    pub memory_saving: bool,
    /// The computer runs models on the processor ([`Target::Cpu`]).
    pub processor_only: bool,
    /// Why not, when the run is red even with the chat model unloaded.
    pub not_for: Option<NotFor>,
    /// When the requested size does not run on the device (whole or one part at a time),
    /// the largest preset by pixel-frames that does: the preset sides at the requested shape
    /// and, for a clip, the preset lengths, none above the requested size; judged in the
    /// room after an unload when a chat model is loaded.
    pub suggestion: Option<GenSize>,
}

/// One part at a time and partly on the processor both run with memory saving
/// (`--vae-tiling --offload-to-cpu`).
pub fn needs_memory_saving(fit: GenFit) -> bool {
    matches!(fit, GenFit::OnePartAtATime | GenFit::PartlyOnProcessor)
}

/// The run's latent tokens: ⌈W/d⌉·⌈H/d⌉ with d the family's `token_div`, times a clip's
/// latent frames, (F − 1)/`frame_div` + 1.
///
/// The catalog does not refuse a 0 in either, so neither is divided by: a `token_div` of 0
/// counts as 1 (every pixel a token, so a broken entry is judged far too big rather than too
/// small) and a `frame_div` of 0 as unset, which is the same as 1 (every frame its own
/// latent frame).
pub fn tokens(entry: &GenEntry, size: GenSize) -> u64 {
    let f = &entry.fallback;
    let d = f.token_div.max(1);
    let frames = size.frames.max(1);
    let latent_frames = match f.frame_div {
        Some(div) if div > 0 => (frames - 1) / div + 1,
        _ => frames,
    };
    u64::from(size.width.div_ceil(d))
        * u64::from(size.height.div_ceil(d))
        * u64::from(latent_frames)
}

/// The diffusion model's working memory: `act_base_bytes + act_bytes_per_token·tokens`.
fn act_dit(entry: &GenEntry, size: GenSize) -> u64 {
    let f = &entry.fallback;
    f.act_base_bytes
        .saturating_add(f.act_bytes_per_token.saturating_mul(tokens(entry, size)))
}

/// The decoder's working memory: `vae_bytes_per_pixel_frame`·W·H·C, with C the frames it
/// decodes at once (`vae_chunk_frames`, at most the clip's; all of them when unset, or 0,
/// which the catalog does not refuse: the most memory, never a guess below it); at most
/// `vae_tile_cap_bytes` with memory saving, which tiles the decoder.
pub fn act_vae(entry: &GenEntry, size: GenSize, memory_saving: bool) -> u64 {
    let f = &entry.fallback;
    let frames = size.frames.max(1);
    let chunk = f
        .vae_chunk_frames
        .filter(|&c| c > 0)
        .map_or(frames, |c| c.min(frames));
    let pixel_frames = u64::from(size.width) * u64::from(size.height) * u64::from(chunk);
    let bytes = f.vae_bytes_per_pixel_frame.saturating_mul(pixel_frames);
    if memory_saving {
        bytes.min(f.vae_tile_cap_bytes)
    } else {
        bytes
    }
}

/// The parts' sizes, in bytes.
struct Weights {
    /// Every diffusion part (a second one, used after the first, included).
    diffusion: u64,
    /// The largest diffusion part: they take turns.
    largest_diffusion: u64,
    /// The text encoders, which work together.
    encoders: u64,
    /// The decoder.
    decoder: u64,
}

impl Weights {
    fn of(entry: &GenEntry) -> Self {
        let mut w = Weights {
            diffusion: 0,
            largest_diffusion: 0,
            encoders: 0,
            decoder: 0,
        };
        for part in &entry.parts {
            match part.role {
                GenRole::Diffusion
                | GenRole::HighNoiseDiffusion
                | GenRole::UncondDiffusion
                | GenRole::Model => {
                    w.diffusion = w.diffusion.saturating_add(part.size);
                    w.largest_diffusion = w.largest_diffusion.max(part.size);
                }
                GenRole::Llm
                | GenRole::LlmVision
                | GenRole::T5xxl
                | GenRole::ClipL
                | GenRole::ClipG
                | GenRole::ClipVision
                | GenRole::Tokenizer
                | GenRole::AudioEncoder
                | GenRole::EmbeddingsConnectors => {
                    w.encoders = w.encoders.saturating_add(part.size);
                }
                GenRole::Vae | GenRole::Tae | GenRole::AudioVae => {
                    w.decoder = w.decoder.saturating_add(part.size);
                }
            }
        }
        w
    }

    fn total(&self) -> u64 {
        self.diffusion
            .saturating_add(self.encoders)
            .saturating_add(self.decoder)
    }
}

/// Everything on the device at once by the family's fallback, without the spare: D + T + V +
/// max(act_dit, act_vae).
fn fallback_whole(entry: &GenEntry, size: GenSize) -> u64 {
    Weights::of(entry)
        .total()
        .saturating_add(act_dit(entry, size).max(act_vae(entry, size, false)))
}

/// How the target's memory is shared between the device and the computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pool {
    /// A graphics card with memory of its own.
    Card,
    /// The processor: every part and the working memory in the computer's memory at once.
    Processor,
    /// Apple silicon: the Metal working set is a share of the computer's memory.
    Unified,
}

/// Where a run is judged.
#[derive(Debug, Clone, Copy)]
struct Place {
    room: Room,
    /// The computer's memory less the reserve; `None` when unknown: not checked.
    computer: Option<u64>,
    pool: Pool,
}

impl Place {
    fn of(target: &Target) -> Self {
        Place {
            room: target.room(),
            computer: target.computer(),
            pool: match target {
                Target::Gpu { .. } => Pool::Card,
                Target::Cpu { .. } => Pool::Processor,
                Target::AppleUnified { .. } => Pool::Unified,
            },
        }
    }

    /// With the chat model's `bytes` given back: on the device and, on Apple silicon, where
    /// the device's memory is the computer's, in the computer's memory too.
    fn unloaded(self, bytes: u64) -> Self {
        Place {
            room: Room {
                whole: self.room.whole.saturating_add(bytes),
                partial: self.room.partial.saturating_add(bytes),
                ..self.room
            },
            computer: match self.pool {
                Pool::Unified => self.computer.map(|c| c.saturating_add(bytes)),
                Pool::Card | Pool::Processor => self.computer,
            },
            ..self
        }
    }
}

/// The engine's backend on `target`, as the catalog's memory points name their device:
/// `vulkan` on a graphics card (the Studio engine's only graphics backend there in 0.1.0),
/// `metal` on Apple silicon, `cpu` on the processor.
fn backend(target: &Target) -> &'static str {
    match target {
        Target::Gpu { .. } => "vulkan",
        Target::AppleUnified { .. } => "metal",
        Target::Cpu { .. } => "cpu",
    }
}

/// What a run at one size needs for each way of running it, without the spare.
struct Need {
    /// Everything on the device at once (Green): every part's weights and the working
    /// memory.
    whole: u64,
    /// The largest of the parts on the device one at a time with its working memory, the
    /// decoder tiled.
    one_part: u64,
    /// Every part's weights: one part at a time, they all wait in the computer's memory
    /// (`--offload-to-cpu` keeps the engine's copy of each there).
    weights: u64,
    /// The diffusion part with its working memory: what a run partly on the processor
    /// needs, between the device and the computer's memory.
    diffusion: u64,
}

impl Need {
    /// At `size`, with the memory points measured on `backend` when the entry has any.
    fn of(entry: &GenEntry, size: GenSize, backend: &str) -> Self {
        let w = Weights::of(entry);
        let act_dit = act_dit(entry, size);
        let whole =
            measured_peak(entry, size, backend).unwrap_or_else(|| fallback_whole(entry, size));
        let diffusion = w.largest_diffusion.saturating_add(act_dit);
        let decoder = w.decoder.saturating_add(act_vae(entry, size, true));
        Need {
            whole,
            one_part: w.encoders.max(diffusion).max(decoder),
            weights: w.total(),
            diffusion,
        }
    }

    /// One part at a time with the spare: what a graphics card or a Mac's working set needs
    /// for it.
    fn device_need(&self) -> u64 {
        self.one_part.saturating_add(SPARE_BYTES)
    }

    /// Why a run that is red at `place` does not fit there ([`NotFor`]): on a graphics card,
    /// the card's free memory one part at a time needs; on the processor, the computer's free
    /// memory the Green need (the only way a run goes there) takes with the reserve; on Apple
    /// silicon, the Mac's memory one part at a time needs, the larger of the working set's
    /// need grown to a whole Mac and every part with the working memory and the reserve.
    fn not_for(&self, place: Place) -> NotFor {
        match place.pool {
            Pool::Card => NotFor::GraphicsMemory {
                needs: self.device_need(),
            },
            Pool::Processor => NotFor::GraphicsMemory {
                needs: self
                    .whole
                    .saturating_add(SPARE_BYTES)
                    .saturating_add(RAM_RESERVE_BYTES),
            },
            Pool::Unified => NotFor::MacMemory {
                needs: mac_with_working_set(self.device_need())
                    .max(self.whole.saturating_add(RAM_RESERVE_BYTES)),
            },
        }
    }

    /// The fit at `place`.
    fn fit(&self, place: Place) -> GenFit {
        let Place {
            room,
            computer,
            pool,
        } = place;
        let fit = if self.whole.saturating_add(SPARE_BYTES) <= room.whole {
            GenFit::Green
        } else if pool == Pool::Processor {
            // Every part already shares the one memory: nothing can wait its turn elsewhere.
            GenFit::Red
        } else if self.device_need() <= room.whole && computer.is_none_or(|c| self.weights <= c) {
            GenFit::OnePartAtATime
        } else if self.diffusion <= room.partial {
            GenFit::PartlyOnProcessor
        } else {
            GenFit::Red
        };
        // The working set is a share of the computer's memory, which holds every part and the
        // working memory however the run goes.
        let short_on_a_mac = pool == Pool::Unified && computer.is_some_and(|c| self.whole > c);
        if short_on_a_mac { GenFit::Red } else { fit }
    }
}

/// The measured peak at `size` from the entry's points of its newest engine build (several
/// builds' points are never mixed), and of those only the ones measured on `backend` when
/// there are any (all of the build's otherwise): linear in tokens between the two nearest;
/// below the smallest, that one alone; above the largest, its peak plus what the fallback
/// grows from its size to `size` (never less than its peak). `None` with no point.
fn measured_peak(entry: &GenEntry, size: GenSize, backend: &str) -> Option<u64> {
    let build = entry.memory.iter().map(|m| m.build).max()?;
    let of_build: Vec<&MemoryPoint> = entry.memory.iter().filter(|m| m.build == build).collect();
    let here = of_build
        .iter()
        .any(|m| m.device.eq_ignore_ascii_case(backend));
    let mut points: Vec<(u64, u64, GenSize)> = of_build
        .into_iter()
        .filter(|m| !here || m.device.eq_ignore_ascii_case(backend))
        .map(|m| {
            let at = point_size(m);
            (tokens(entry, at), m.peak_bytes, at)
        })
        .collect();
    // By tokens; of points with the same tokens, the highest peak.
    points.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    points.dedup_by_key(|p| p.0);
    let at = tokens(entry, size);
    let below = points.iter().rev().find(|p| p.0 <= at);
    let above = points.iter().find(|p| p.0 >= at);
    match (below, above) {
        (Some(&(t0, p0, _)), Some(&(t1, p1, _))) if t1 > t0 => {
            let part =
                |rise: u64| (u128::from(rise) * u128::from(at - t0) / u128::from(t1 - t0)) as u64;
            Some(if p1 >= p0 {
                p0 + part(p1 - p0)
            } else {
                p0 - part(p0 - p1)
            })
        }
        (Some(&(_, peak, _)), Some(_)) | (None, Some(&(_, peak, _))) => Some(peak),
        (Some(&(_, peak, largest)), None) => {
            let growth = fallback_whole(entry, size).saturating_sub(fallback_whole(entry, largest));
            Some(peak.saturating_add(growth))
        }
        (None, None) => None,
    }
}

fn point_size(m: &MemoryPoint) -> GenSize {
    GenSize {
        width: m.width,
        height: m.height,
        frames: m.frames.max(1),
    }
}

/// The memory of a Mac whose working set ([`MAC_WORKING_SET_SHARE`] of it) is at least
/// `working_set` bytes: rounded up, so the share of it never falls a byte short.
fn mac_with_working_set(working_set: u64) -> u64 {
    let (share, of) = MAC_WORKING_SET_SHARE;
    let mac = (u128::from(working_set) * u128::from(of)).div_ceil(u128::from(share));
    u64::try_from(mac).unwrap_or(u64::MAX)
}

/// Whether `entry` at `size` fits `target`. `target` as probed now (with the chat model
/// loaded, if one is); `chat_device_bytes`: what unloading it would give back.
///
/// On [`Target::Cpu`] the run is judged against the computer's memory, Green or Red, and a
/// video is red ([`NotFor::VideoNeedsGpu`]); on [`Target::AppleUnified`] a video model the
/// catalog keeps off Apple silicon is red ([`NotFor::VideoNotOnThisMac`]), and any other run
/// is red when the computer's memory less the reserve cannot hold all of it.
pub fn gen_fit(
    entry: &GenEntry,
    size: GenSize,
    target: &Target,
    chat_device_bytes: Option<u64>,
) -> GenFitResult {
    let video = entry.kind == GenKind::Video;
    let blocked = match target {
        Target::Cpu { .. } if video => Some(NotFor::VideoNeedsGpu),
        Target::AppleUnified { .. } if video && !entry.runs_on_apple => {
            Some(NotFor::VideoNotOnThisMac)
        }
        _ => None,
    };
    let backend = backend(target);
    let need = Need::of(entry, size, backend);
    let judge = |place: Place| match blocked {
        Some(_) => GenFit::Red,
        None => need.fit(place),
    };
    let place = Place::of(target);
    let fit = judge(place);
    let unloaded = chat_device_bytes.map(|bytes| place.unloaded(bytes));
    let after_unload = unloaded.filter(|_| fit != GenFit::Green).map(judge);
    // With a chat model loaded, the reason and the suggestion are for the room after the
    // unload: a run that is red now is not Green, so `after_unload` is that room's verdict.
    let judged = unloaded.unwrap_or(place);
    let not_for = (after_unload.unwrap_or(fit) == GenFit::Red)
        .then(|| blocked.unwrap_or_else(|| need.not_for(judged)));
    let suggestion = match blocked {
        Some(_) => None,
        None => suggest(entry, size, backend, judged),
    };
    GenFitResult {
        fit,
        after_unload,
        needed_bytes: need.whole,
        available_bytes: target.free_bytes(),
        memory_saving: needs_memory_saving(fit),
        processor_only: matches!(target, Target::Cpu { .. }),
        not_for,
        suggestion,
    }
}

/// [`GenFitResult::suggestion`]: `None` when `size` itself runs on the device at `place`, or
/// no preset does.
fn suggest(entry: &GenEntry, size: GenSize, backend: &str, place: Place) -> Option<GenSize> {
    let on_device = |s: GenSize| Need::of(entry, s, backend).fit(place).on_device();
    if on_device(size) {
        return None;
    }
    let (sides, lengths, longest): (&[u32], &[u32], u32) = if entry.kind == GenKind::Video {
        let most = entry.limits.max_frames.unwrap_or(u32::MAX);
        (&VIDEO_SIDES, &VIDEO_FRAMES, size.frames.min(most))
    } else {
        (&PICTURE_SIDES, &[1], 1)
    };
    let shape = shape_of(size);
    let max_side = entry.limits.max_side;
    sides
        .iter()
        .filter_map(|&side| at_shape(side, shape, entry.limits.multiple_of, None))
        .filter(|&(w, h)| w <= size.width && h <= size.height)
        .filter(|&(w, h)| max_side == 0 || w.max(h) <= max_side)
        .flat_map(|(width, height)| {
            lengths
                .iter()
                .filter(move |&&frames| frames <= longest)
                .map(move |&frames| GenSize {
                    width,
                    height,
                    frames,
                })
        })
        .filter(|&s| on_device(s))
        .max_by_key(|s| s.pixel_frames())
}

/// The offered shape ([`SHAPES`]) nearest the size's own (a size is rounded to the model's
/// multiple, so 1280×704 is 16:9). Studio's local runs read a request's shape by it too, so
/// the fit, the estimate and the job describe one size.
pub fn shape_of(size: GenSize) -> (u32, u32) {
    let log_ratio = |w: u32, h: u32| (f64::from(w.max(1)) / f64::from(h.max(1))).ln();
    let own = log_ratio(size.width, size.height);
    let distance = |&(w, h): &(u32, u32)| (log_ratio(w, h) - own).abs();
    SHAPES
        .into_iter()
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))
        .unwrap_or((1, 1))
}

/// The size with `side` as its shorter side at `shape`, each side rounded down to a
/// multiple of `multiple_of` (480p at 16:9 is 832×480, 720p 1280×704; a `multiple_of` of 0,
/// which the catalog does not refuse, counts as 1: any side); `None` when that leaves
/// nothing. With a `max_side`, a longer side beyond it is cut to it and the shorter side
/// follows at the shape before both are rounded (1024 at 16:9 within 1536 is 1536×864): how
/// Studio's local runs size a request, whose job keeps within the model's longest side. The
/// suggestion passes none: it leaves out a preset beyond the longest side instead.
pub fn at_shape(
    side: u32,
    (w, h): (u32, u32),
    multiple_of: u32,
    max_side: Option<u32>,
) -> Option<(u32, u32)> {
    let m = u64::from(multiple_of.max(1));
    let down = |x: u64| u32::try_from(x / m * m).ok();
    let (lo, hi) = (u64::from(w.min(h).max(1)), u64::from(w.max(h)));
    let (mut short, mut long) = (u64::from(side), u64::from(side) * hi / lo);
    if let Some(max) = max_side.map(u64::from).filter(|&max| long > max) {
        long = max;
        short = max * lo / hi;
    }
    let (short, long) = (down(short)?, down(long)?);
    if short == 0 {
        return None;
    }
    Some(if w >= h { (long, short) } else { (short, long) })
}

#[cfg(test)]
pub(crate) mod tests {
    //! Entries built here with the pinned files' sizes (Task L7), which round to the figures
    //! in the comments (GB = 1e9), and the families' fallbacks written as literals (never read
    //! from the bundled catalog), so each expectation follows from the formulas and a re-tuned
    //! catalog never moves it.

    use std::collections::BTreeMap;

    use super::*;
    use crate::gen_catalog::{
        FitFallback, FrameRule, GateMode, GenDefaults, GenFamily, GenGate, GenLimits, GenPart,
    };

    pub(crate) const GB: u64 = 1_000_000_000;

    /// FLUX.2 klein 4B's and Z-Image Turbo's fallback.
    fn picture_fallback() -> FitFallback {
        FitFallback {
            act_base_bytes: 500_000_000,
            act_bytes_per_token: 100_000,
            vae_bytes_per_pixel_frame: 300,
            vae_tile_cap_bytes: 500_000_000,
            token_div: 16,
            frame_div: None,
            vae_chunk_frames: None,
        }
    }

    fn wan22_fallback() -> FitFallback {
        FitFallback {
            act_base_bytes: 1_000_000_000,
            act_bytes_per_token: 250_000,
            vae_bytes_per_pixel_frame: 400,
            vae_tile_cap_bytes: 2_500_000_000,
            token_div: 32,
            frame_div: Some(4),
            vae_chunk_frames: Some(4),
        }
    }

    fn wan21_fallback() -> FitFallback {
        FitFallback {
            act_base_bytes: 500_000_000,
            act_bytes_per_token: 100_000,
            vae_bytes_per_pixel_frame: 300,
            vae_tile_cap_bytes: 1_500_000_000,
            token_div: 16,
            frame_div: Some(4),
            vae_chunk_frames: Some(4),
        }
    }

    fn picture_limits() -> GenLimits {
        GenLimits {
            multiple_of: 16,
            max_side: 2048,
            frame_rule: None,
            max_frames: None,
            video: None,
        }
    }

    fn video_limits(multiple_of: u32, max_frames: u32) -> GenLimits {
        GenLimits {
            multiple_of,
            max_side: 1280,
            frame_rule: Some(FrameRule::FourNPlusOne),
            max_frames: Some(max_frames),
            video: None,
        }
    }

    /// An entry with these parts, made at `default` by default.
    fn entry(
        id: &str,
        kind: GenKind,
        family: GenFamily,
        parts: [(GenRole, u64); 3],
        fallback: FitFallback,
        limits: GenLimits,
        default: GenSize,
    ) -> GenEntry {
        GenEntry {
            id: id.into(),
            name: id.into(),
            summary: String::new(),
            publisher: String::new(),
            kind,
            family: Some(family),
            recipe: family.recipe_id().into(),
            version: Some(family.version()),
            untested: false,
            inputs: Vec::new(),
            input_counts: Vec::new(),
            parts: parts
                .into_iter()
                .map(|(role, size)| GenPart {
                    role,
                    repo: "test/model".into(),
                    revision: "0".repeat(40),
                    path: format!("{role:?}.gguf"),
                    size,
                    sha256: "ab".repeat(32),
                    license: "apache-2.0".into(),
                    local: None,
                    folder: None,
                })
                .collect(),
            licenses: BTreeMap::new(),
            gate: GenGate {
                mode: GateMode::None,
                collects: Vec::new(),
            },
            defaults: GenDefaults {
                steps: 4,
                cfg: 1.0,
                flow_shift: None,
                sampler: None,
                scheduler: None,
                width: default.width,
                height: default.height,
                frames: (kind == GenKind::Video).then_some(default.frames),
                fps: (kind == GenKind::Video).then_some(24),
                guidance: None,
                img_cfg: None,
                timestep_shift: None,
                sigmas: Vec::new(),
                high_noise: None,
                moe_boundary: None,
                strength: None,
            },
            limits,
            fallback,
            memory: Vec::new(),
            speed: Vec::new(),
            quirks: Vec::new(),
            min_build: 1106,
            runs_on_apple: true,
            tested: None,
        }
    }

    /// FLUX.2 klein 4B: D 2.46, T 2.50, V 0.34.
    pub(crate) fn klein() -> GenEntry {
        entry(
            "flux2-klein-4b-q4",
            GenKind::Image,
            GenFamily::Flux2Klein4b,
            [
                (GenRole::Diffusion, 2_460_378_560),
                (GenRole::Llm, 2_497_281_312),
                (GenRole::Vae, 336_211_292),
            ],
            picture_fallback(),
            picture_limits(),
            sq(1024),
        )
    }

    /// Z-Image Turbo: D 3.86, T 2.50, V 0.34.
    pub(crate) fn zimage() -> GenEntry {
        entry(
            "z-image-turbo-q4",
            GenKind::Image,
            GenFamily::ZImageTurbo,
            [
                (GenRole::Diffusion, 3_864_250_304),
                (GenRole::Llm, 2_497_281_120),
                (GenRole::Vae, 335_304_388),
            ],
            picture_fallback(),
            picture_limits(),
            sq(1024),
        )
    }

    /// Wan 2.2 TI2V-5B fast: D 4.21, T 4.15, V 1.41.
    pub(crate) fn wan22() -> GenEntry {
        entry(
            "wan22-ti2v-5b-fast-q6",
            GenKind::Video,
            GenFamily::Wan22Ti2v5bFast,
            [
                (GenRole::Diffusion, 4_210_247_200),
                (GenRole::T5xxl, 4_145_878_880),
                (GenRole::Vae, 1_409_400_960),
            ],
            wan22_fallback(),
            video_limits(32, 121),
            clip(832, 480, 49),
        )
    }

    /// Wan 2.1 T2V 1.3B: D 2.84 (a size stated for the test: Task L17 pins its file), T 4.15,
    /// V 0.25.
    fn wan21() -> GenEntry {
        entry(
            "wan21-t2v-1.3b",
            GenKind::Video,
            GenFamily::Wan21T2v1_3b,
            [
                (GenRole::Diffusion, 2_840_000_000),
                (GenRole::T5xxl, 4_145_878_880),
                (GenRole::Vae, 253_815_318),
            ],
            wan21_fallback(),
            video_limits(16, 81),
            clip(832, 480, 33),
        )
    }

    pub(crate) fn gpu(free_gb: u64, ram_gb: u64) -> Target {
        Target::Gpu {
            name: "GPU".into(),
            free_bytes: free_gb * GB,
            ram_free_bytes: Some(ram_gb * GB),
        }
    }

    pub(crate) fn apple(ws_gb: u64, ram_gb: u64) -> Target {
        Target::AppleUnified {
            working_set_bytes: ws_gb * GB,
            ram_free_bytes: Some(ram_gb * GB),
        }
    }

    pub(crate) fn cpu(ram_gb: u64) -> Target {
        Target::Cpu {
            ram_free_bytes: ram_gb * GB,
        }
    }

    pub(crate) fn sq(side: u32) -> GenSize {
        GenSize {
            width: side,
            height: side,
            frames: 1,
        }
    }

    pub(crate) fn clip(width: u32, height: u32, frames: u32) -> GenSize {
        GenSize {
            width,
            height,
            frames,
        }
    }

    /// 0.2.0: a model the catalog does not list, from its recipe with these parts.
    fn generic(recipe_id: &str, parts: &[(GenRole, u64)]) -> GenEntry {
        crate::gen_catalog::generic_entry(crate::gen_catalog::GenericSpec {
            id: recipe_id.into(),
            name: recipe_id.into(),
            publisher: String::new(),
            recipe: crate::sd::recipes::recipe(recipe_id).unwrap(),
            version: None,
            parts: parts
                .iter()
                .map(|&(role, size)| GenPart {
                    role,
                    repo: "test/model".into(),
                    revision: "0".repeat(40),
                    path: format!("{role:?}.gguf"),
                    size,
                    sha256: "ab".repeat(32),
                    license: "apache-2.0".into(),
                    local: None,
                    folder: None,
                })
                .collect(),
            licenses: BTreeMap::new(),
            gate: GenGate {
                mode: GateMode::None,
                collects: Vec::new(),
            },
            untested: false,
        })
    }

    /// A generic SDXL checkpoint goes through the same formulas with its kind's fallback: 32
    /// pixels a token (sizes are multiples of 64), so 1024 × 1024 is 1024 tokens.
    #[test]
    fn a_generic_picture_model_is_judged_by_its_kinds_fallback() {
        let sdxl = generic("sdxl-checkpoint", &[(GenRole::Model, 6_938_078_334)]);
        // act_dit 0.5 + 0.0001·1024 = 0.60; act_vae 0.0000003·1024² = 0.31
        assert_eq!(tokens(&sdxl, sq(1024)), 1024);
        // 6.94 + 0.60 + 1 spare = 8.54 <= 9
        assert_eq!(
            gen_fit(&sdxl, sq(1024), &gpu(9, 32), None).fit,
            GenFit::Green
        );
        // 8.54 > 8; one part 6.94 + 0.60 + 1 = 8.54 > 8
        assert_ne!(
            gen_fit(&sdxl, sq(1024), &gpu(8, 32), None).fit,
            GenFit::Green
        );
    }

    /// A generic Wan 2.2 A14B's two diffusion parts take turns, like 0.1.x's: one part at a
    /// time needs the larger one only.
    #[test]
    fn a_generic_two_model_video_runs_its_diffusion_parts_in_turns() {
        let wan = generic(
            "wan22-t2v",
            &[
                (GenRole::HighNoiseDiffusion, 9_651_800_000),
                (GenRole::Diffusion, 9_651_800_000),
                (GenRole::T5xxl, 4_145_878_880),
                (GenRole::Vae, 253_815_318),
            ],
        );
        let size = clip(1280, 720, 33);
        // 80 × 45 tokens a latent frame, 9 latent frames.
        assert_eq!(tokens(&wan, size), 32_400);
        // green 23.70 + 9.10 + 1 > 24; one part 9.65 + 9.10 + 1 = 19.75 <= 24, and the
        // weights (23.70) wait in 64 − 2 GB
        let r = gen_fit(&wan, size, &gpu(24, 64), None);
        assert_eq!((r.fit, r.memory_saving), (GenFit::OnePartAtATime, true));
    }

    #[test]
    fn klein_at_1024_is_green_on_an_8_gb_card() {
        // 5.30 + 0.91 + 1 spare = 7.21 <= 8
        assert_eq!(
            gen_fit(&klein(), sq(1024), &gpu(8, 32), None).fit,
            GenFit::Green
        );
    }

    #[test]
    fn on_a_6_gb_card_it_runs_one_part_at_a_time_with_memory_saving() {
        // green 8.61 > 6; one part 3.86 + 0.91 + 1 = 5.77 <= 6
        let r = gen_fit(&zimage(), sq(1024), &gpu(6, 32), None);
        assert_eq!((r.fit, r.memory_saving), (GenFit::OnePartAtATime, true));
        assert!(needs_memory_saving(GenFit::PartlyOnProcessor));
        assert!(!needs_memory_saving(GenFit::Green) && !needs_memory_saving(GenFit::Red));
    }

    #[test]
    fn too_little_everywhere_is_red_with_a_reason() {
        // partial room 2 + (4 − 2) = 4 < D + act_dit 4.77
        let r = gen_fit(&zimage(), sq(1024), &gpu(2, 4), None);
        assert_eq!(r.fit, GenFit::Red);
        assert!(
            matches!(r.not_for, Some(NotFor::GraphicsMemory { needs }) if needs >= 5_770_000_000)
        );
    }

    #[test]
    fn the_chat_models_memory_counts_after_an_unload() {
        // 3 GB free: partly on the processor; 8 GB after the unload: one part at a time
        let r = gen_fit(&zimage(), sq(1024), &gpu(3, 32), Some(5 * GB));
        assert_eq!(
            (r.fit, r.after_unload),
            (GenFit::PartlyOnProcessor, Some(GenFit::OnePartAtATime))
        );
        // Green beside the chat model: nothing to unload, so no second verdict.
        let r = gen_fit(&klein(), sq(1024), &gpu(8, 32), Some(5 * GB));
        assert_eq!((r.fit, r.after_unload), (GenFit::Green, None));
    }

    #[test]
    fn the_reason_never_blames_the_chat_models_share() {
        // 1 GB free: red now; 4 GB and 5 GB with part on the processor after a 3 GB unload.
        let r = gen_fit(&zimage(), sq(1024), &gpu(1, 3), Some(3 * GB));
        assert_eq!(
            (r.fit, r.after_unload, r.not_for),
            (GenFit::Red, Some(GenFit::PartlyOnProcessor), None)
        );
        // Red even after a 1 GB unload: the need is the job's own, the same as with no chat.
        let r = gen_fit(&zimage(), sq(1024), &gpu(1, 3), Some(GB));
        assert_eq!(
            (r.after_unload, r.not_for),
            (
                Some(GenFit::Red),
                Some(NotFor::GraphicsMemory {
                    needs: 5_773_850_304
                })
            )
        );
    }

    #[test]
    fn a_video_decoder_works_one_chunk_at_a_time() {
        // 400 B × 832 × 480 × 4 frames, not × 49
        assert_eq!(
            act_vae(&wan22(), clip(832, 480, 49), false),
            400 * 832 * 480 * 4
        );
    }

    #[test]
    fn the_working_memory_follows_the_formulas() {
        // At 1024²: 4096 tokens, act_vae 0.31 GB (the whole picture at once).
        assert_eq!(tokens(&klein(), sq(1024)), 4096);
        assert_eq!(act_vae(&klein(), sq(1024), false), 314_572_800);
        // Memory saving tiles the decoder: at most the tile cap.
        assert_eq!(act_vae(&klein(), sq(2048), false), 300 * 2048 * 2048);
        assert_eq!(act_vae(&klein(), sq(2048), true), 500_000_000);
        // A clip shorter than a chunk decodes all its frames at once.
        assert_eq!(act_vae(&wan22(), clip(832, 480, 1), false), 400 * 832 * 480);
        // The need with everything on the card: D + T + V + the larger working memory.
        assert_eq!(
            gen_fit(&klein(), sq(1024), &gpu(8, 32), None).needed_bytes,
            5_293_871_164 + 909_600_000
        );
    }

    #[test]
    fn video_on_the_processor_is_not_offered_and_pictures_say_processor() {
        assert!(matches!(
            gen_fit(&wan22(), clip(832, 480, 49), &cpu(32), None).not_for,
            Some(NotFor::VideoNeedsGpu)
        ));
        assert!(gen_fit(&klein(), sq(512), &cpu(32), None).processor_only);
        // A picture too big for the computer's memory: the free memory the system would have
        // to report, every part and the working memory with the spare and the 2 GB reserve
        // (7.61 + 1 + 2 = 10.61 > 4).
        let r = gen_fit(&zimage(), sq(1024), &cpu(4), None);
        assert_eq!(
            (r.fit, r.not_for),
            (
                GenFit::Red,
                Some(NotFor::GraphicsMemory {
                    needs: 10_606_435_812
                })
            )
        );
    }

    #[test]
    fn on_the_processor_every_part_shares_one_memory() {
        // 6 GB less the 2 GB reserve: 5.29 + 0.91 + 1 = 7.20 > 4. Red, never one part at a
        // time (4.37) or partly on the processor (3.37), as nothing can wait elsewhere.
        let r = gen_fit(&klein(), sq(1024), &cpu(6), None);
        assert_eq!(
            (r.fit, r.memory_saving, r.processor_only),
            (GenFit::Red, false, true)
        );
        // The reason gives the free memory the only way it runs there needs, 7.20 with the
        // 2 GB reserve: 9.20, more than the 6 GB free; not one part at a time.
        assert_eq!(
            r.not_for,
            Some(NotFor::GraphicsMemory {
                needs: 9_203_471_164
            })
        );
        // 512² with 30 GB: 5.29 + 0.60 + 1 = 6.90. Green, and the word is still "Slow here".
        let r = gen_fit(&klein(), sq(512), &cpu(32), None);
        assert_eq!((r.fit, r.memory_saving), (GenFit::Green, false));
        assert_eq!(
            crate::gen_eta::fit_word(&klein(), &r, None),
            crate::gen_eta::FitWord::Slow
        );
        // 7 GB left: 1024² (7.20) is red and 768² (7.02) too; 512² (6.90) is suggested. The
        // reason's 9.20 is more than the 9 GB the system reports free, never the 4.37 one
        // part at a time would need on a graphics card.
        let r = gen_fit(&klein(), sq(1024), &cpu(9), None);
        assert_eq!(
            (r.fit, r.suggestion, r.not_for),
            (
                GenFit::Red,
                Some(sq(512)),
                Some(NotFor::GraphicsMemory {
                    needs: 9_203_471_164
                })
            )
        );
        assert!(9_203_471_164 > r.available_bytes);
        // The chat model's memory given back counts there too: 4 + 4 = 8 >= 7.20.
        let r = gen_fit(&klein(), sq(1024), &cpu(6), Some(4 * GB));
        assert_eq!(
            (r.fit, r.after_unload, r.not_for),
            (GenFit::Red, Some(GenFit::Green), None)
        );
    }

    #[test]
    fn a_mac_must_hold_every_part_in_its_memory_too() {
        // The 8 GB working set holds klein whole (7.20), but 6 GB free less the 2 GB reserve
        // cannot hold its parts and working memory (6.20): red.
        let r = gen_fit(&klein(), sq(1024), &apple(8, 6), None);
        assert_eq!(r.fit, GenFit::Red);
        // The Mac it needs: the larger of 4.37 ÷ 2/3 = 6.55 (a working set that holds one
        // part at a time) and 6.20 + the 2 GB reserve = 8.20, more than its 6 GB free.
        assert_eq!(
            r.not_for,
            Some(NotFor::MacMemory {
                needs: 8_203_471_164
            })
        );
        // Red after a 1 GB unload too (4 + 1 < 6.20): the job's own figure, more than 5 + 1.
        let r = gen_fit(&klein(), sq(1024), &apple(8, 5), Some(GB));
        assert_eq!(
            (r.after_unload, r.not_for),
            (
                Some(GenFit::Red),
                Some(NotFor::MacMemory {
                    needs: 8_203_471_164
                })
            )
        );
        // 9 GB free (7 after the reserve) holds it.
        assert_eq!(
            gen_fit(&klein(), sq(1024), &apple(8, 9), None).fit,
            GenFit::Green
        );
        // On a Mac the chat model's memory comes back to the computer as well: 4 + 3 >= 6.20.
        let r = gen_fit(&klein(), sq(1024), &apple(8, 6), Some(3 * GB));
        assert_eq!((r.after_unload, r.not_for), (Some(GenFit::Green), None));
        // One part at a time on a 6 GB working set (5.77), the weights in 7 GB (6.70), but not
        // every part with the working memory (7.61): red, needing a Mac of 7.61 + 2 = 9.61
        // (more than 5.77 ÷ 2/3 = 8.66); with 8 GB left, one part at a time.
        let r = gen_fit(&zimage(), sq(1024), &apple(6, 9), None);
        assert_eq!(
            (r.fit, r.not_for),
            (
                GenFit::Red,
                Some(NotFor::MacMemory {
                    needs: 9_606_435_812
                })
            )
        );
        assert_eq!(
            gen_fit(&zimage(), sq(1024), &apple(6, 10), None).fit,
            GenFit::OnePartAtATime
        );
    }

    /// A Mac with `memory`, two thirds of it its working set and all of it free.
    fn mac(memory: u64) -> Target {
        Target::AppleUnified {
            working_set_bytes: memory * 2 / 3,
            ram_free_bytes: Some(memory),
        }
    }

    #[test]
    fn a_reasons_figure_is_more_than_the_memory_that_falls_short() {
        // Wherever a run is red with a figure, the figure is more than this computer has in
        // the room judged (the chat model's share counted back), in the figure's own unit:
        // the free memory the system reports where models run (the card's, or the computer's
        // before the reserve on the processor), or the Mac's memory (the Mac its working set
        // tells, or else its free memory). And it is enough: a computer that has it runs the
        // job on the device.
        let runs = [
            (klein(), sq(512)),
            (klein(), sq(1024)),
            (klein(), sq(2048)),
            (zimage(), sq(1024)),
            (zimage(), clip(2048, 1152, 1)),
            (wan22(), clip(832, 480, 49)),
            (wan22(), clip(1280, 704, 121)),
        ];
        let mut computers = vec![];
        for ram_gb in [1, 2, 4, 6, 8, 10, 12, 16, 24] {
            computers.push((cpu(ram_gb), ram_gb));
            for device_gb in 1..=14 {
                computers.push((gpu(device_gb, ram_gb), ram_gb));
                computers.push((apple(device_gb, ram_gb), ram_gb));
            }
        }
        let (mut figures, mut mac_computer_short) = (0, 0);
        for ((entry, size), (target, ram_gb), chat_gb) in runs
            .iter()
            .flat_map(|run| computers.iter().map(move |c| (run, c)))
            .flat_map(|(run, c)| [None, Some(1), Some(3)].map(|chat| (run, c, chat)))
        {
            let chat = chat_gb.map(|gb| gb * GB);
            let back = chat.unwrap_or(0);
            let case = format!("{} {size:?} on {target:?} with {chat_gb:?} GB", entry.id);
            let runs_on = |there: Target| gen_fit(entry, *size, &there, None).fit.on_device();
            match gen_fit(entry, *size, target, chat).not_for {
                Some(NotFor::GraphicsMemory { needs }) => {
                    let free = target.free_bytes() + back;
                    assert!(needs > free, "{case}: needs {needs}, free {free}");
                    let enough = match target {
                        Target::Cpu { .. } => Target::Cpu {
                            ram_free_bytes: needs,
                        },
                        _ => Target::Gpu {
                            name: "GPU".into(),
                            free_bytes: needs,
                            ram_free_bytes: Some(1_000 * GB),
                        },
                    };
                    assert!(runs_on(enough), "{case}: {needs} free is not enough");
                }
                Some(NotFor::MacMemory { needs }) => {
                    let working_set_tells = (target.room().whole + back) * 3 / 2;
                    if needs <= working_set_tells {
                        let free = ram_gb * GB + back;
                        assert!(needs > free, "{case}: needs {needs}, free {free}");
                        mac_computer_short += 1;
                    }
                    assert!(
                        runs_on(mac(needs)),
                        "{case}: a Mac of {needs} is not enough"
                    );
                }
                Some(NotFor::VideoNeedsGpu | NotFor::VideoNotOnThisMac) | None => continue,
            }
            figures += 1;
        }
        assert!(figures > 100, "only {figures} red runs with a figure");
        assert!(
            mac_computer_short > 10,
            "only {mac_computer_short} on a Mac's free memory"
        );
    }

    #[test]
    fn a_card_whose_computer_did_not_say_is_judged_on_the_card() {
        let card = |free_gb: u64| Target::Gpu {
            name: "GPU".into(),
            free_bytes: free_gb * GB,
            ram_free_bytes: None,
        };
        // One part at a time (5.77 <= 6): the weights waiting in the computer's memory are
        // not checked, as unknown is not 0.
        assert_eq!(
            gen_fit(&zimage(), sq(1024), &card(6), None).fit,
            GenFit::OnePartAtATime
        );
        // Partly on the processor: the computer's side is unbounded.
        assert_eq!(
            gen_fit(&zimage(), sq(1024), &card(3), None).fit,
            GenFit::PartlyOnProcessor
        );
    }

    #[test]
    fn zeros_in_the_catalog_are_never_divided_by() {
        let mut e = wan22();
        e.fallback.token_div = 0;
        e.fallback.frame_div = Some(0);
        e.fallback.vae_chunk_frames = Some(0);
        // A token_div of 0 counts as 1 and a frame_div of 0 as unset: every pixel of every
        // frame a token, so the entry is judged far too big, not too small.
        assert_eq!(tokens(&e, clip(832, 480, 49)), 832 * 480 * 49);
        // A vae_chunk_frames of 0 as unset: every frame at once.
        assert_eq!(act_vae(&e, clip(832, 480, 49), false), 400 * 832 * 480 * 49);
        let r = gen_fit(&e, clip(832, 480, 49), &gpu(24, 64), None);
        assert_eq!((r.fit, r.suggestion), (GenFit::Red, None));
        assert!(r.not_for.is_some());
        // A multiple_of of 0 counts as 1: the 16:9 suggestion keeps 1365×768, unrounded.
        let mut e = klein();
        e.limits.multiple_of = 0;
        let card = Target::Gpu {
            name: "GPU".into(),
            free_bytes: 4_500_000_000,
            ram_free_bytes: Some(32 * GB),
        };
        let r = gen_fit(&e, clip(2048, 1152, 1), &card, None);
        assert_eq!(r.suggestion, Some(clip(1365, 768, 1)));
        assert_eq!(
            gen_fit(&e, sq(2048), &gpu(5, 32), None).suggestion,
            Some(sq(1024))
        );
    }

    #[test]
    fn on_a_mac_the_reason_names_the_mac_and_a_video_model_may_be_kept_off_it() {
        let mut wan = wan22();
        wan.runs_on_apple = false;
        let r = gen_fit(&wan, clip(832, 480, 49), &apple(16, 24), None);
        assert_eq!(
            (r.fit, r.not_for, r.suggestion),
            (GenFit::Red, Some(NotFor::VideoNotOnThisMac), None)
        );
        // 2.84 + 4.15 + 0.25 + 1.90 + 1 = 10.14 <= 16
        let r = gen_fit(&wan21(), clip(832, 480, 33), &apple(16, 24), None);
        assert_eq!((r.fit, r.not_for), (GenFit::Green, None));
        // Both short: the Mac's memory one part at a time needs, the larger of 5.77 ÷ 2/3 =
        // 8.66 and 7.61 + the 2 GB reserve = 9.61.
        let r = gen_fit(&zimage(), sq(1024), &apple(2, 4), None);
        assert_eq!(
            r.not_for,
            Some(NotFor::MacMemory {
                needs: 9_606_435_812
            })
        );
    }

    #[test]
    fn a_macs_figure_is_the_memory_of_a_mac_that_runs_it() {
        // 1280² × 121 frames: one part at a time needs 4.21 + 13.40 + 1 = 18.61 of working
        // set, so a Mac of 18.61 ÷ 2/3 = 27.92, more than every part with the working memory
        // and the reserve (23.17 + 2 = 25.17).
        let big = clip(1280, 1280, 121);
        let r = gen_fit(&wan22(), big, &apple(16, 32), None);
        assert_eq!(
            (r.fit, r.not_for),
            (
                GenFit::Red,
                Some(NotFor::MacMemory {
                    needs: 27_915_370_800
                })
            )
        );
        assert_eq!(
            gen_fit(&wan22(), big, &mac(27_915_370_800), None).fit,
            GenFit::OnePartAtATime
        );
        // An odd working-set need (18_610_247_201) rounds the Mac up, so that two thirds of it
        // still hold one part at a time; one byte less and they do not.
        let mut odd = wan22();
        odd.parts[0].size += 1;
        let needs = match gen_fit(&odd, big, &apple(16, 32), None).not_for {
            Some(NotFor::MacMemory { needs }) => needs,
            other => panic!("{other:?}"),
        };
        assert_eq!(needs, 27_915_370_802);
        assert_eq!(
            gen_fit(&odd, big, &mac(needs), None).fit,
            GenFit::OnePartAtATime
        );
        assert_ne!(
            gen_fit(&odd, big, &mac(needs - 1), None).fit,
            GenFit::OnePartAtATime
        );
    }

    #[test]
    fn a_clip_too_big_suggests_the_largest_preset_that_fits() {
        // 1280×704×121 needs 13.0 GB one part at a time; 832×480×49 needs 7.48 (one part, fits 8), 832×480×81 8.26 (no),
        // 1280×704×17 7.31 (fits, but fewer pixel-frames than 832×480×49)
        let r = gen_fit(&wan22(), clip(1280, 704, 121), &gpu(8, 16), None);
        assert_eq!(
            (r.fit, r.suggestion),
            (
                GenFit::PartlyOnProcessor,
                Some(GenSize {
                    width: 832,
                    height: 480,
                    frames: 49
                })
            )
        );
    }

    #[test]
    fn with_a_chat_model_loaded_the_suggestion_is_for_the_room_after_the_unload() {
        // 3 GB free beside a 5 GB chat model: nothing fits now, 832×480×49 after the unload.
        let big = clip(1280, 704, 121);
        assert_eq!(gen_fit(&wan22(), big, &gpu(3, 16), None).suggestion, None);
        assert_eq!(
            gen_fit(&wan22(), big, &gpu(3, 16), Some(5 * GB)).suggestion,
            Some(clip(832, 480, 49))
        );
        // A size that fits gets no suggestion.
        assert_eq!(
            gen_fit(&wan22(), clip(832, 480, 49), &gpu(8, 16), None).suggestion,
            None
        );
    }

    #[test]
    fn a_picture_too_big_suggests_the_largest_side_at_its_shape() {
        // 2048²: 2.46 + 2.14 + 1 = 5.60 > 5 with part on the processor; 1024² one part: 4.37
        let r = gen_fit(&klein(), sq(2048), &gpu(5, 32), None);
        assert_eq!(
            (r.fit, r.suggestion),
            (GenFit::PartlyOnProcessor, Some(sq(1024)))
        );
        // 16:9 at 2048×1152 needs 4.88 one part at a time; 1808×1024 4.68, 1360×768 4.37
        let card = Target::Gpu {
            name: "GPU".into(),
            free_bytes: 4_500_000_000,
            ram_free_bytes: Some(32 * GB),
        };
        let r = gen_fit(&klein(), clip(2048, 1152, 1), &card, None);
        assert_eq!(
            (r.fit, r.suggestion),
            (GenFit::PartlyOnProcessor, Some(clip(1360, 768, 1)))
        );
    }

    fn memory_point(side: u32, peak_bytes: u64, device: &str, build: u32) -> MemoryPoint {
        MemoryPoint {
            width: side,
            height: side,
            frames: 1,
            peak_bytes,
            device: device.into(),
            build,
        }
    }

    #[test]
    fn measured_points_win_over_the_fallback() {
        let point = |side, peak_bytes, build| memory_point(side, peak_bytes, "vulkan", build);
        let mut e = klein();
        e.memory = vec![
            point(1024, 5_000_000_000, 1106),
            point(768, 4_100_000_000, 1106),
            // Another engine build's point is not mixed in.
            point(896, 9_000_000_000, 1105),
        ];
        let needed = |side| gen_fit(&e, sq(side), &gpu(8, 32), None).needed_bytes;
        assert_eq!(needed(1024), 5_000_000_000);
        // 896²: 3136 tokens, 832 of the 1792 from 768² (2304) to 1024² (4096).
        assert_eq!(needed(896), 4_100_000_000 + 900_000_000 * 832 / 1792);
        assert!((4_100_000_000..5_000_000_000).contains(&needed(896)));
        // Below the smallest, that one alone.
        assert_eq!(needed(512), 4_100_000_000);
        // The measured 5.0 GB fits a 6 GB card whole, where the fallback said one part at a time.
        assert_eq!(gen_fit(&e, sq(1024), &gpu(6, 32), None).fit, GenFit::Green);
    }

    #[test]
    fn above_the_largest_point_the_need_grows_as_the_fallback_does() {
        let mut e = klein();
        e.memory = vec![memory_point(1024, 5_000_000_000, "vulkan", 1106)];
        let needed = |side| gen_fit(&e, sq(side), &gpu(8, 32), None).needed_bytes;
        // 1280²: act_dit 1.14 GB against 0.91 at 1024², 0.23 GB more.
        assert_eq!(needed(1280), 5_000_000_000 + 1_140_000_000 - 909_600_000);
        // 2048²: act_dit 2.14 GB, 1.23 GB more, so a 6 GB card no longer runs it whole
        // (6.23 + 1 > 6) but one part at a time (2.46 + 2.14 + 1 = 5.60).
        assert_eq!(needed(2048), 5_000_000_000 + 2_138_400_000 - 909_600_000);
        assert_eq!(
            gen_fit(&e, sq(2048), &gpu(6, 32), None).fit,
            GenFit::OnePartAtATime
        );
        // Never less than the point's own peak: with a decoder that dominates, 1025×1009 has
        // more tokens than 1024² (4160) but fewer pixels, so the fallback shrinks there.
        e.fallback.vae_bytes_per_pixel_frame = 10_000;
        assert!(fallback_whole(&e, clip(1025, 1009, 1)) < fallback_whole(&e, sq(1024)));
        assert_eq!(
            gen_fit(&e, clip(1025, 1009, 1), &gpu(8, 32), None).needed_bytes,
            5_000_000_000
        );
    }

    #[test]
    fn memory_points_of_this_computers_backend_are_used_when_there_are_any() {
        let mut e = klein();
        e.memory = vec![
            memory_point(1024, 5_000_000_000, "vulkan", 1106),
            memory_point(768, 4_100_000_000, "vulkan", 1106),
            memory_point(1024, 6_500_000_000, "metal", 1106),
            // Only the newest build's points are looked at, whatever their device.
            memory_point(1024, 7_000_000_000, "cpu", 1105),
        ];
        let needed = |target: &Target, side| gen_fit(&e, sq(side), target, None).needed_bytes;
        assert_eq!(needed(&gpu(8, 32), 1024), 5_000_000_000);
        // A Mac uses the Metal point alone: below it, that one.
        assert_eq!(needed(&apple(16, 32), 1024), 6_500_000_000);
        assert_eq!(needed(&apple(16, 32), 768), 6_500_000_000);
        // No point of the newest build on the processor: all of that build's points, the
        // higher of two at one size.
        assert_eq!(needed(&cpu(32), 1024), 6_500_000_000);
        assert_eq!(needed(&cpu(32), 768), 4_100_000_000);
    }

    #[test]
    fn wan_2_2_ti2v_counts_tokens_in_32_pixel_patches() {
        assert_eq!(tokens(&wan22(), clip(832, 480, 49)), 26 * 15 * 13);
        // Wan 2.1 in 16-pixel patches.
        assert_eq!(tokens(&wan21(), clip(832, 480, 33)), 52 * 30 * 9);
    }

    /// The shape and size helpers Studio's local runs share with the suggestion (ruling
    /// L14-shape-helpers): the fit, the estimate and the job describe one size.
    #[test]
    fn a_size_at_a_shape_keeps_within_a_longest_side_when_given_one() {
        // 480p and 720p at 16:9 on a multiple of 32, with no longest side.
        assert_eq!(at_shape(480, (16, 9), 32, None), Some((832, 480)));
        assert_eq!(at_shape(720, (16, 9), 32, None), Some((1280, 704)));
        assert_eq!(at_shape(480, (9, 16), 32, None), Some((480, 832)));
        // 1024 at 16:9 on 16: 1820 rounds down to 1808; within a longest side of 1536, the
        // longer side is 1536 and the shorter one keeps the shape (864).
        assert_eq!(at_shape(1024, (16, 9), 16, None), Some((1808, 1024)));
        assert_eq!(at_shape(1024, (16, 9), 16, Some(1536)), Some((1536, 864)));
        assert_eq!(at_shape(1024, (9, 16), 16, Some(1536)), Some((864, 1536)));
        // A longest side the size keeps within anyway changes nothing.
        assert_eq!(at_shape(720, (16, 9), 32, Some(2048)), Some((1280, 704)));
        // Nothing left at the multiple: none.
        assert_eq!(at_shape(10, (1, 1), 16, None), None);
        // The nearest offered shape of a size rounded to a multiple.
        assert_eq!(shape_of(clip(1280, 704, 1)), (16, 9));
        assert_eq!(shape_of(clip(880, 1168, 1)), (3, 4));
        assert_eq!(shape_of(sq(1024)), (1, 1));
    }

    #[test]
    fn the_size_serializes_for_the_webview() {
        assert_eq!(
            serde_json::to_value(clip(832, 480, 49)).unwrap(),
            serde_json::json!({"width": 832, "height": 480, "frames": 49})
        );
        assert_eq!(
            serde_json::to_value(GenFit::OnePartAtATime).unwrap(),
            serde_json::json!("onePartAtATime")
        );
    }
}
