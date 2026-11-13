//! Where a model's parts go on this computer, and whether that fits (0.0.6 item 6:
//! "Where the model goes", Fine control, the fit marks). Pure arithmetic on numbers from
//! the model file and the system, no I/O; every operation saturates, so hostile numbers
//! never wrap or panic.
//!
//! llama.cpp counts a model in *units*: its blocks (`blk.0` … `blk.(B-1)`) and the output
//! unit, B + 1 in all ("offloaded N/M layers"). `-ngl N` puts N units on the graphics card
//! from the back: the output unit first, then `blk.(B-1)`, `blk.(B-2)`, …, so blocks
//! `0 … B−N` stay on the processor (llama-model.cpp, `i_gpu_start`). The input layer
//! (`token_embd`) always stays on the processor.

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

pub fn split(input: &PlacementInput, place: &Where) -> Split {
    let _ = (input, place);
    todo!()
}

pub fn fit_target(free_bytes: u64, margin_bytes: u64) -> u64 {
    let _ = (free_bytes, margin_bytes);
    todo!()
}

pub fn fit_layers(input: &PlacementInput, target_bytes: u64) -> (Layers, ExpertsPick) {
    fit_layers_where(input, target_bytes, true)
}

pub fn fit_layers_where(
    input: &PlacementInput,
    target_bytes: u64,
    kv_on_gpu: bool,
) -> (Layers, ExpertsPick) {
    let _ = (input, target_bytes, kv_on_gpu);
    todo!()
}

pub fn fit_order(sizes: &LayerSizes) -> Vec<(Layers, ExpertsPick)> {
    let _ = sizes;
    todo!()
}

pub fn explain(sizes: &LayerSizes, gpu_model_bytes: u64) -> Option<(Layers, ExpertsPick)> {
    let _ = (sizes, gpu_model_bytes);
    todo!()
}

pub fn effective_expert_layers(units: u32, layers: Layers, k: u32) -> u32 {
    let _ = (units, layers, k);
    todo!()
}

pub fn kv_per_layer(summary: &ModelSummary, ctx: u64, cache: KvCacheType) -> Vec<u64> {
    let _ = (summary, ctx, cache, MAX_BLOCKS);
    todo!()
}

/// The detail of the one verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Fits {
    Fits,
    Tight,
    Partial,
    WontFitGraphics,
    WontFitAtAll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryKind {
    Processor,
    Separate,
    Unified,
}

pub fn judge(split: &Split, room: Room, gpu_choice_is_fit: bool) -> (Verdict, Fits) {
    let _ = (split, room, gpu_choice_is_fit, SPARE_BYTES);
    todo!()
}

pub fn judge_in(
    split: &Split,
    room: Room,
    gpu_choice_is_fit: bool,
    kind: MemoryKind,
) -> (Verdict, Fits) {
    let _ = (split, room, gpu_choice_is_fit, kind);
    todo!()
}

pub fn auto_ctx_placed(
    at: impl Fn(u64) -> Split,
    room: Room,
    floor: u64,
    cap: u64,
) -> Option<AutoCtx> {
    let _ = (at(0), room, floor, cap);
    todo!()
}

pub fn auto_ctx_placed_in(
    at: impl Fn(u64) -> Split,
    room: Room,
    floor: u64,
    cap: u64,
    kind: MemoryKind,
) -> Option<AutoCtx> {
    let _ = (at(0), room, floor, cap, kind);
    todo!()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capacity {
    pub total_bytes: u64,
    pub free_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bar {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub other_bytes: u64,
    pub need: Parts,
    pub free_after_bytes: u64,
}

pub fn bar(capacity: Capacity, need: Parts) -> Bar {
    let _ = (capacity, need);
    todo!()
}

pub fn bars(
    split: &Split,
    kind: MemoryKind,
    graphics: Option<Capacity>,
    computer: Capacity,
) -> (Option<Bar>, Bar) {
    let _ = (split, kind, graphics, computer);
    todo!()
}

#[cfg(test)]
mod tests;
