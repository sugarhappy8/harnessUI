//! Where a model's parts go on this computer, and whether that fits (0.0.6 item 6:
//! "Where the model goes", Fine control, the fit marks). Pure arithmetic on numbers from
//! the model file and the system, no I/O; every operation saturates, so hostile numbers
//! never wrap or panic.
//!
//! llama.cpp counts a model in *units*: its blocks (`blk.0` … `blk.(B-1)`) and the output
//! unit, B + 1 in all ("offloaded N/M layers"). `-ngl N` puts N units on the graphics card
//! from the back: the output unit first, then `blk.(B-1)`, `blk.(B-2)`, …, so blocks
//! `0 … B−N` stay on the processor (llama-model.cpp, `i_gpu_start`). The input layer
//! (`token_embd` and the rest of [`crate::layers::INPUT_TENSORS`]) always stays on the
//! processor.

use serde::Serialize;

use crate::fit::{AutoCtx, Room, SPARE_BYTES, Verdict};
use crate::layers::{LayerSizes, MAX_BLOCKS};
use crate::summary::{KvCacheType, ModelSummary};

/// The facts a placement is worked out from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementInput<'a> {
    pub sizes: &'a LayerSizes,
    /// Conversation memory per block, in bytes, at the context and cache type chosen
    /// ([`kv_per_layer`]); a missing entry counts as 0.
    pub kv_per_layer: Vec<u64>,
    /// The working space of the device that holds the output unit (fit.rs's allowance,
    /// [`crate::fit::FitInput::compute_bytes`]).
    pub compute_gpu: u64,
    /// The working space the processor needs besides, when the output unit is on the
    /// graphics card and some work still runs on the processor (fit.rs's
    /// `COMPUTE_BASE_BYTES`).
    pub compute_cpu: u64,
    /// The image reader (projector) loaded with the model: on the graphics card whenever
    /// one is used, else in computer memory.
    pub projector: u64,
}

/// How many units are on the graphics card, counted from the back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layers {
    All,
    Count(u32),
}

/// Which blocks keep their expert parts on the processor (only blocks on the graphics card
/// are affected; blocks on the processor have everything there anyway).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpertsPick {
    None,
    All,
    /// `--n-cpu-moe K`: blocks `0 … K−1`.
    FirstLayers(u32),
    /// The last K blocks: As much as fits' own result (llama.cpp's fit); never a launch
    /// value.
    LastLayers(u32),
}

/// One way of placing the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Where {
    /// A graphics device is used at all (false: everything on the processor).
    pub graphics: bool,
    pub layers: Layers,
    pub experts_on_cpu: ExpertsPick,
    /// The conversation memory of each block goes with the block (false: `-nkvo`, all of
    /// it in computer memory).
    pub kv_on_gpu: bool,
}

/// What one side holds, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Parts {
    pub model: u64,
    pub context: u64,
    pub compute: u64,
    pub image_reader: u64,
}

impl Parts {
    pub fn total(&self) -> u64 {
        self.model
            .saturating_add(self.context)
            .saturating_add(self.compute)
            .saturating_add(self.image_reader)
    }

    /// Both sides together, part by part (one bar for unified memory).
    pub fn plus(&self, other: &Parts) -> Parts {
        Parts {
            model: self.model.saturating_add(other.model),
            context: self.context.saturating_add(other.context),
            compute: self.compute.saturating_add(other.compute),
            image_reader: self.image_reader.saturating_add(other.image_reader),
        }
    }
}

/// Where everything went.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Split {
    pub gpu: Parts,
    pub cpu: Parts,
    pub layers_on_gpu: u32,
    pub units: u32,
    /// Blocks on the graphics card whose expert parts are on the processor.
    pub expert_layers_on_cpu: u32,
}

impl ExpertsPick {
    /// Whether block `index` of `blocks` keeps its expert parts on the processor (when the
    /// block itself is on the graphics card).
    fn moves(self, index: usize, blocks: usize) -> bool {
        let count = |k: u32| usize::try_from(k).unwrap_or(usize::MAX);
        match self {
            Self::None => false,
            Self::All => true,
            Self::FirstLayers(k) => index < count(k),
            Self::LastLayers(k) => index >= blocks.saturating_sub(count(k)),
        }
    }
}

/// The units on the graphics card for `layers` of a model with `units` units.
fn units_on_card(layers: Layers, units: u32) -> u32 {
    match layers {
        Layers::All => units,
        Layers::Count(n) => n.min(units),
    }
}

fn add(to: &mut u64, bytes: u64) {
    *to = to.saturating_add(bytes);
}

/// Where every part of the model goes for one placement, in bytes.
///
/// - The input layer always stays on the processor.
/// - `layers` units are on the graphics card from the back (finding 6): the output unit,
///   which carries the tensors that belong to no block (`other_bytes`), then the last
///   blocks. A block's conversation memory goes with it, unless `kv_on_gpu` is false
///   (`-nkvo`), when all of it is in computer memory.
/// - `experts_on_cpu` moves the expert parts of the chosen blocks that are on the card;
///   blocks on the processor have everything there anyway, and blocks without expert
///   parts are not counted.
/// - The device holding the output unit has the working space (`compute_gpu`); the
///   processor needs `compute_cpu` besides whenever it still does some of the work (a
///   block, an expert part or the conversation memory is there).
/// - The image reader is on the graphics card whenever one is used.
/// - Without a graphics device everything is in computer memory, with the whole working
///   space.
/// - A tied output (embeddings shared with the input) is counted twice wherever the output
///   unit is, as [`LayerSizes::output_bytes`] says: right on a graphics device and for
///   weights the processor repacks, high by the embedding when the processor shares the
///   input's copy (the safe side; the measurement replaces the estimate).
pub fn split(input: &PlacementInput, place: &Where) -> Split {
    let sizes = input.sizes;
    let units = sizes.units();
    let blocks = sizes.layers.len();
    let kv = |index: usize| input.kv_per_layer.get(index).copied().unwrap_or(0);
    if !place.graphics {
        return Split {
            gpu: Parts::default(),
            cpu: Parts {
                model: sizes.total(),
                context: (0..blocks).map(kv).fold(0, u64::saturating_add),
                compute: input.compute_gpu,
                image_reader: input.projector,
            },
            layers_on_gpu: 0,
            units,
            expert_layers_on_cpu: 0,
        };
    }
    let on_card = units_on_card(place.layers, units);
    // The first block on the card: units − on_card (the output unit is the last unit), or
    // none at all when nothing is on the card.
    let first_on_card = if on_card == 0 {
        blocks
    } else {
        usize::try_from(units - on_card)
            .unwrap_or(blocks)
            .min(blocks)
    };
    let mut gpu = Parts::default();
    let mut cpu = Parts {
        model: sizes.input_bytes,
        ..Parts::default()
    };
    let output = sizes.output_bytes.saturating_add(sizes.other_bytes);
    add(
        if on_card > 0 {
            &mut gpu.model
        } else {
            &mut cpu.model
        },
        output,
    );
    let mut expert_layers_on_cpu = 0u32;
    for (index, layer) in sizes.layers.iter().enumerate() {
        if index < first_on_card {
            add(&mut cpu.model, layer.total());
            add(&mut cpu.context, kv(index));
            continue;
        }
        if layer.expert_bytes > 0 && place.experts_on_cpu.moves(index, blocks) {
            add(&mut gpu.model, layer.dense_bytes);
            add(&mut cpu.model, layer.expert_bytes);
            expert_layers_on_cpu = expert_layers_on_cpu.saturating_add(1);
        } else {
            add(&mut gpu.model, layer.total());
        }
        add(
            if place.kv_on_gpu {
                &mut gpu.context
            } else {
                &mut cpu.context
            },
            kv(index),
        );
    }
    if on_card > 0 {
        gpu.compute = input.compute_gpu;
        if first_on_card > 0 || expert_layers_on_cpu > 0 || !place.kv_on_gpu {
            cpu.compute = input.compute_cpu;
        }
    } else {
        cpu.compute = input.compute_gpu;
    }
    gpu.image_reader = input.projector;
    Split {
        gpu,
        cpu,
        layers_on_gpu: on_card,
        units,
        expert_layers_on_cpu,
    }
}

/// What llama.cpp's fit may fill: the free memory less the margin it leaves (Leave free on
/// the graphics card, 1,024 MiB when Automatic). A free amount below the margin gives 0.
pub fn fit_target(free_bytes: u64, margin_bytes: u64) -> u64 {
    free_bytes.saturating_sub(margin_bytes)
}

/// What llama.cpp's fit counts on the card: everything but the image reader, which its
/// measurement leaves out (server README: projectors are not measured).
fn fit_need(split: &Split) -> u64 {
    split
        .gpu
        .model
        .saturating_add(split.gpu.context)
        .saturating_add(split.gpu.compute)
}

fn as_layers(on_card: u32, units: u32) -> Layers {
    if on_card >= units {
        Layers::All
    } else {
        Layers::Count(on_card)
    }
}

fn last_layers(k: u32) -> ExpertsPick {
    if k == 0 {
        ExpertsPick::None
    } else {
        ExpertsPick::LastLayers(k)
    }
}

/// llama.cpp's step 3 with `on_card` units on the card (common/fit.cpp, line 569): for a
/// mixture of experts the blocks on the card are "dense-only" with their expert parts on
/// the processor, except the frontmost one. llama.cpp sets `n_part = n_layer − 1` (the
/// output unit "must always be full") and writes the expert overrides for the *last*
/// `n_part` unit numbers, the first of which is the block in front of the output; the
/// override past the last block matches nothing, so the frontmost block keeps its expert
/// parts on the card.
fn step3(on_card: u32, units: u32, experts: bool) -> (Layers, ExpertsPick) {
    let pick = if experts {
        last_layers(on_card.saturating_sub(2))
    } else {
        ExpertsPick::None
    };
    (as_layers(on_card, units), pick)
}

/// llama.cpp's fit for one device (common/fit.cpp steps 3 and 4), whole layers only, with
/// the conversation memory on the card. See [`fit_layers_where`].
pub fn fit_layers(input: &PlacementInput, target_bytes: u64) -> (Layers, ExpertsPick) {
    fit_layers_where(input, target_bytes, true)
}

/// llama.cpp's fit for one device, whole layers only: all units when everything fits
/// under `target_bytes`; else step 3, the most units back to front that fit (for a
/// mixture of experts, with the expert parts of the blocks on the card on the processor
/// as [`step3`] describes); then, for a mixture of experts whose dense parts all fit with
/// every expert part on the processor (llama.cpp's "surplus"), step 4: expert parts come
/// back from the front of the card's blocks while they fit, so the expert parts of the
/// last K blocks stay on the processor.
///
/// llama.cpp searches by false position and assumes memory grows with every layer; this
/// takes the most layers that fit, which is the same when it does. llama.cpp's step 4
/// also puts part of one more block on the card (only its up, or up and gate, expert
/// tensors); whole layers leave that out, so the answer can be up to one block's expert
/// parts short of what llama.cpp places. The image reader is not counted.
pub fn fit_layers_where(
    input: &PlacementInput,
    target_bytes: u64,
    kv_on_gpu: bool,
) -> (Layers, ExpertsPick) {
    let need = |(layers, experts_on_cpu): (Layers, ExpertsPick)| {
        fit_need(&split(
            input,
            &Where {
                graphics: true,
                layers,
                experts_on_cpu,
                kv_on_gpu,
            },
        ))
    };
    let fits = |place| need(place) <= target_bytes;
    let whole = (Layers::All, ExpertsPick::None);
    if fits(whole) {
        return whole;
    }
    let units = input.sizes.units();
    let experts = input.sizes.has_experts();
    let on_card = (0..=units)
        .rev()
        .find(|&n| fits(step3(n, units, experts)))
        .unwrap_or(0);
    // llama.cpp goes on to step 4 only with a surplus: free − (use + margin) > 0.
    if !experts || need((Layers::All, ExpertsPick::All)) >= target_bytes {
        return step3(on_card, units, experts);
    }
    let layers = as_layers(on_card, units);
    let mut moved = on_card.saturating_sub(2);
    while moved > 0 && fits((layers, last_layers(moved - 1))) {
        moved -= 1;
    }
    (layers, last_layers(moved))
}

/// Every placement llama.cpp's fit can answer with for one device, whole layers only, in
/// its own order: step 3 from no unit to all of them, then (mixture of experts) step 4
/// from every expert part of the card's blocks but the frontmost on the processor to none.
/// Its bytes on the card grow along the order when every block weighs the same.
pub fn fit_order(sizes: &LayerSizes) -> Vec<(Layers, ExpertsPick)> {
    let units = sizes.units();
    let experts = sizes.has_experts();
    let mut order: Vec<_> = (0..=units).map(|n| step3(n, units, experts)).collect();
    if experts {
        order.extend(
            (0..units.saturating_sub(2))
                .rev()
                .map(|k| (Layers::All, last_layers(k))),
        );
    }
    order
}

/// The placement in fit's own order ([`fit_order`]) that explains `gpu_model_bytes`, the
/// model bytes on the graphics card after a load with As much as fits (the ready line's
/// memory report), or `None` when the bytes cannot tell (the menu then gives amounts
/// only).
///
/// A placement explains the bytes when they lie within a quarter of the gap to its
/// neighbours in the order (llama.cpp's buffers add a little alignment to the file's
/// sizes); bytes further from every placement are "between steps". Above a placement with
/// every unit on the card that keeps the expert parts of the last K blocks on the processor
/// (step 4, or the end of step 3 where step 4 starts), the window reaches three quarters
/// (rounded up) of the frontmost of those blocks' expert parts: llama.cpp's step 4 puts the
/// up (or the up and gate) expert tensors of that block on the card ("part of one more
/// layer"), so the answer stays right within one layer. Step 3 never places part of a
/// layer, so its placements with fewer units keep the quarter-gap window. When windows
/// overlap, the nearest placement wins, and of equals the first.
pub fn explain(sizes: &LayerSizes, gpu_model_bytes: u64) -> Option<(Layers, ExpertsPick)> {
    let order = fit_order(sizes);
    let model_only = PlacementInput {
        sizes,
        kv_per_layer: Vec::new(),
        compute_gpu: 0,
        compute_cpu: 0,
        projector: 0,
    };
    let bytes: Vec<u64> = order
        .iter()
        .map(|&(layers, experts_on_cpu)| {
            split(
                &model_only,
                &Where {
                    graphics: true,
                    layers,
                    experts_on_cpu,
                    kv_on_gpu: true,
                },
            )
            .gpu
            .model
        })
        .collect();
    let blocks = sizes.layers.len();
    let mut best: Option<(u64, usize)> = None;
    for (index, &at) in bytes.iter().enumerate() {
        let before = index
            .checked_sub(1)
            .and_then(|i| bytes.get(i))
            .map(|&b| b.abs_diff(at));
        let after = bytes.get(index + 1).map(|&b| b.abs_diff(at));
        let below = before.or(after).unwrap_or(0) / 4;
        let mut above = after.or(before).unwrap_or(0) / 4;
        if let Some((Layers::All, ExpertsPick::LastLayers(k))) = order.get(index) {
            let frontmost = blocks.saturating_sub(usize::try_from(*k).unwrap_or(usize::MAX));
            if let Some(layer) = sizes.layers.get(frontmost) {
                above = above.max(layer.expert_bytes - layer.expert_bytes / 4);
            }
        }
        let distance = gpu_model_bytes.abs_diff(at);
        let inside = distance <= if gpu_model_bytes >= at { above } else { below };
        if inside && best.is_none_or(|(nearest, _)| distance < nearest) {
            best = Some((distance, index));
        }
    }
    best.and_then(|(_, index)| order.get(index).copied())
}

/// Expert layers that `FirstLayers(k)` actually takes off the card when `layers` of
/// `units` are on it: max(0, min(k, blocks) − blocks on the processor). The first blocks
/// are the ones on the processor (finding 6), so their expert parts are there already.
pub fn effective_expert_layers(units: u32, layers: Layers, k: u32) -> u32 {
    let blocks = units.saturating_sub(1);
    let on_card = units_on_card(layers, units);
    let on_processor = if on_card == 0 {
        blocks
    } else {
        units - on_card
    };
    k.min(blocks).saturating_sub(on_processor)
}

/// The conversation memory of each block, in bytes, for `ctx` tokens stored as `cache`:
/// a key row and a value row per token, as [`ModelSummary::kv_bytes_per_token_as`] counts
/// them (recurrent layers, with no key/value heads, hold none). One entry per block, at
/// most [`MAX_BLOCKS`]; the same sliding-window caveat as the summary's (counted at full
/// length, the safe side).
pub fn kv_per_layer(summary: &ModelSummary, ctx: u64, cache: KvCacheType) -> Vec<u64> {
    let blocks = usize::try_from(summary.block_count.min(MAX_BLOCKS)).unwrap_or(0);
    summary
        .head_count_kv
        .iter()
        .take(blocks)
        .map(|&heads| {
            cache
                .row_bytes(heads.saturating_mul(summary.key_length))
                .saturating_add(cache.row_bytes(heads.saturating_mul(summary.value_length)))
                .saturating_mul(ctx)
        })
        .collect()
}

/// The detail of the one verdict (section 4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Fits {
    /// Green: everything fits with the spare left free.
    Fits,
    /// Yellow, All of it or a custom placement: the card holds its part, but not with the
    /// spare (games or video may stop it from loading).
    Tight,
    /// Yellow, As much as fits: part of the model runs on the processor (no warning; the
    /// layer line explains it).
    Partial,
    /// Yellow, All of it or a custom placement: the card's part is larger than its free
    /// memory, but the model fits with part on the processor (the step-down would load it).
    WontFitGraphics,
    /// Red: not even with part on the processor.
    WontFitAtAll,
}

/// What kind of memory the graphics side has, which decides how the computer's part is
/// checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryKind {
    /// No graphics device: everything is in computer memory, and the room is computer
    /// memory less the reserve ([`Room::processor`]).
    Processor,
    /// A graphics card with its own memory ([`Room::graphics_card`]): the card's part must
    /// fit its free memory, and the computer's part must fit computer memory less the
    /// reserve on its own.
    Separate,
    /// Apple silicon ([`Room::apple`]): one memory, so the computer's part is checked only
    /// with the whole model.
    Unified,
}

/// The contract's verdict (item 6, section 2.2), which cannot tell a graphics card's own
/// memory from Apple's shared memory: a [`Room`] holds the same two numbers for both.
///
/// It is [`judge_in`] with [`MemoryKind::Unified`] (the computer's part checked only
/// through the total, as on Apple silicon), and with [`MemoryKind::Processor`] for a split
/// with nothing on the graphics side ([`Room::verdict`] of the total, measured against
/// `room.whole`). On a graphics card with its own memory that is wrong: with few layers on
/// the card, or the conversation memory in computer memory, it says Green for a computer
/// part that does not fit computer memory. Every caller knows the backend
/// (`load_settings::Machine`), so it calls [`judge_in`] with the kind instead: Processor
/// without a graphics device, Unified for Apple graphics (Metal), Separate for every other
/// graphics card. Kept, deprecated, until the lead amends the contract.
#[deprecated(
    note = "cannot tell a graphics card from Apple's shared memory; call judge_in with the \
            MemoryKind of the backend (Separate for a graphics card)"
)]
pub fn judge(split: &Split, room: Room, gpu_choice_is_fit: bool) -> (Verdict, Fits) {
    judge_in(split, room, gpu_choice_is_fit, contract_kind(split))
}

/// The kind [`judge`] assumes: the processor when nothing is on the graphics side, else
/// Apple's shared memory.
fn contract_kind(split: &Split) -> MemoryKind {
    if split.gpu.total() == 0 && split.layers_on_gpu == 0 {
        MemoryKind::Processor
    } else {
        MemoryKind::Unified
    }
}

/// The one answer to "will it fit?" (section 4.1) for a known kind of memory: the
/// three-way mark of the panel and the picture's detail, with fit.rs's margins
/// ([`SPARE_BYTES`] on the main device; the reserve, already taken out of the room, in
/// computer memory). `room` comes from the matching constructor: [`Room::processor`],
/// [`Room::graphics_card`] (Separate) or [`Room::apple`] (Unified).
///
/// - Processor: [`Room::verdict`] of the total (Green / `fits`, Red / `wontFitAtAll`).
/// - Red / `wontFitAtAll` when the total with the spare is larger than `room.partial`, or
///   (`Separate`) the computer's part is larger than computer memory less the reserve.
/// - As much as fits: Yellow / `partial` when any unit or expert part is on the
///   processor; else Green / `fits` when the card's part fits `room.whole` with the
///   spare, else Yellow / `tight`. Never `wontFitGraphics`: llama.cpp fits again at load
///   time.
/// - All of it or a custom placement: Green / `fits` with the spare, Yellow / `tight`
///   without it, Yellow / `wontFitGraphics` when the card's part is larger than
///   `room.whole`.
pub fn judge_in(
    split: &Split,
    room: Room,
    gpu_choice_is_fit: bool,
    kind: MemoryKind,
) -> (Verdict, Fits) {
    let total = split.gpu.total().saturating_add(split.cpu.total());
    if kind == MemoryKind::Processor {
        return match room.verdict(total) {
            Verdict::Green => (Verdict::Green, Fits::Fits),
            Verdict::Yellow => (Verdict::Yellow, Fits::Partial),
            Verdict::Red => (Verdict::Red, Fits::WontFitAtAll),
        };
    }
    let computer = room.partial.saturating_sub(room.whole);
    if total.saturating_add(SPARE_BYTES) > room.partial
        || (kind == MemoryKind::Separate && split.cpu.total() > computer)
    {
        return (Verdict::Red, Fits::WontFitAtAll);
    }
    let card = split.gpu.total();
    let with_spare = card.saturating_add(SPARE_BYTES) <= room.whole;
    if gpu_choice_is_fit {
        if split.layers_on_gpu < split.units || split.expert_layers_on_cpu > 0 {
            (Verdict::Yellow, Fits::Partial)
        } else if with_spare {
            (Verdict::Green, Fits::Fits)
        } else {
            (Verdict::Yellow, Fits::Tight)
        }
    } else if with_spare {
        (Verdict::Green, Fits::Fits)
    } else if card <= room.whole {
        (Verdict::Yellow, Fits::Tight)
    } else {
        (Verdict::Yellow, Fits::WontFitGraphics)
    }
}

/// The context sizes Automatic chooses from, as `fit::auto_ctx` does: the floor, the
/// multiples of 1,024 between, and the cap (a floor above the cap gives the cap alone).
/// Never built as a list: the cap may be any number.
struct Sizes {
    floor: u64,
    cap: u64,
    first_multiple: u64,
    multiples: u64,
}

impl Sizes {
    fn new(floor: u64, cap: u64) -> Self {
        let floor = floor.min(cap);
        let first_multiple = (floor / 1024)
            .checked_add(1)
            .and_then(|k| k.checked_mul(1024));
        let multiples = match first_multiple {
            Some(first) if cap > first => (cap - 1 - first) / 1024 + 1,
            _ => 0,
        };
        Self {
            floor,
            cap,
            first_multiple: first_multiple.unwrap_or(u64::MAX),
            multiples,
        }
    }

    /// The number of sizes (at least 1, at most u64::MAX / 1024 + 2).
    fn len(&self) -> u64 {
        1 + self.multiples + u64::from(self.cap > self.floor)
    }

    /// The `index`th size, smallest first.
    fn get(&self, index: u64) -> u64 {
        if index == 0 {
            self.floor
        } else if index <= self.multiples {
            self.first_multiple + (index - 1) * 1024
        } else {
            self.cap
        }
    }
}

/// The largest size whose verdict is Green, else the largest that is Yellow, else `None`
/// (the floor is Red). Assumes the verdict only gets worse as the size grows; a binary
/// search, so a huge cap costs about 60 verdicts.
fn auto_by(verdict: impl Fn(u64) -> Verdict, floor: u64, cap: u64) -> Option<AutoCtx> {
    let sizes = Sizes::new(floor, cap);
    let wanted = verdict(sizes.get(0));
    let accept = |index: u64| match wanted {
        Verdict::Green => verdict(sizes.get(index)) == Verdict::Green,
        _ => verdict(sizes.get(index)) != Verdict::Red,
    };
    if wanted == Verdict::Red {
        return None;
    }
    let (mut low, mut high) = (0u64, sizes.len() - 1);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if accept(mid) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    Some(AutoCtx {
        ctx: sizes.get(low),
        verdict: wanted,
    })
}

/// The contract's Automatic for a custom placement (item 6, section 2.2): [`judge`]'s rule,
/// so on a graphics card with its own memory it can choose a size whose conversation
/// memory in computer memory does not fit there. Callers use [`auto_ctx_placed_in`] with
/// the backend's [`MemoryKind`] instead. Kept, deprecated, until the lead amends the
/// contract.
#[deprecated(
    note = "cannot tell a graphics card from Apple's shared memory; call auto_ctx_placed_in \
            with the MemoryKind of the backend (Separate for a graphics card)"
)]
pub fn auto_ctx_placed(
    at: impl Fn(u64) -> Split,
    room: Room,
    floor: u64,
    cap: u64,
) -> Option<AutoCtx> {
    auto_by(
        |ctx| {
            let split = at(ctx);
            judge_in(&split, room, false, contract_kind(&split)).0
        },
        floor,
        cap,
    )
}

/// Automatic (D2b) for a custom placement or conversation memory in computer memory: the
/// largest context from `floor` to `cap`, as `fit::auto_ctx` chooses them, whose split
/// (`at(ctx)`) [`judge_in`] marks Green for this kind of memory and graphics choice, else
/// the largest it marks Yellow; `None` when not even the floor fits. As much as fits marks
/// a split with part on the processor Yellow, so Green is the whole model on the card, as
/// `fit::auto_ctx` prefers.
pub fn auto_ctx_placed_in(
    at: impl Fn(u64) -> Split,
    room: Room,
    floor: u64,
    cap: u64,
    kind: MemoryKind,
    gpu_choice_is_fit: bool,
) -> Option<AutoCtx> {
    auto_by(
        |ctx| judge_in(&at(ctx), room, gpu_choice_is_fit, kind).0,
        floor,
        cap,
    )
}

/// One memory as the picture draws it, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capacity {
    /// The memory's size; 0 when the system does not say.
    pub total_bytes: u64,
    /// Free now (with the loaded model counted as free, when it will be unloaded).
    pub free_bytes: u64,
}

/// One bar of the picture: the memory's size split into what other programs use, what
/// the model needs and what is left after loading. `need.total() + other_bytes +
/// free_after_bytes == total_bytes` whenever the model fits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bar {
    pub total_bytes: u64,
    pub free_bytes: u64,
    /// Used by other programs: the size less the free memory.
    pub other_bytes: u64,
    pub need: Parts,
    /// Free after loading: the free memory less what the model needs (0 when it needs
    /// more).
    pub free_after_bytes: u64,
}

/// The bar of `capacity` holding `need`. A free figure above the size is cut to the size;
/// an unknown size (0) is taken to be the free figure.
pub fn bar(capacity: Capacity, need: Parts) -> Bar {
    let total_bytes = if capacity.total_bytes == 0 {
        capacity.free_bytes
    } else {
        capacity.total_bytes
    };
    let free_bytes = capacity.free_bytes.min(total_bytes);
    Bar {
        total_bytes,
        free_bytes,
        other_bytes: total_bytes - free_bytes,
        need,
        free_after_bytes: free_bytes.saturating_sub(need.total()),
    }
}

/// The picture's bars for `split`: a graphics card gets its own bar (from `graphics`, or
/// an empty one when its size is not known, so the need is never lost) beside computer
/// memory; Apple silicon and the processor get one bar of computer memory holding both
/// parts.
pub fn bars(
    split: &Split,
    kind: MemoryKind,
    graphics: Option<Capacity>,
    computer: Capacity,
) -> (Option<Bar>, Bar) {
    match kind {
        MemoryKind::Separate => (
            Some(bar(graphics.unwrap_or_default(), split.gpu)),
            bar(computer, split.cpu),
        ),
        MemoryKind::Processor | MemoryKind::Unified => {
            (None, bar(computer, split.gpu.plus(&split.cpu)))
        }
    }
}

#[cfg(test)]
mod tests;
