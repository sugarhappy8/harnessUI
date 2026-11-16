//! The first-tier fit check (ARCHITECTURE.md 11): will this model fit before it is
//! downloaded? Weights + KV cache + projector + a stated compute allowance, compared
//! with the memory the selected backend can use. The KV cache is a function of the
//! context ([`KvSize`]): short-memory layers stop growing at their window. The saved
//! copies llama-server keeps in the computer's memory during a long chat (context
//! checkpoints) are sized for each load from what the computer has left ([`copies`]) and
//! never decide the context or turn a verdict red.

use serde::Serialize;

use crate::kv::{KvSize, MAX_CTX_CHECKPOINTS};

/// Headroom that a green verdict must leave free.
pub const SPARE_BYTES: u64 = 1_000_000_000;
/// The smallest context a yellow verdict suggests; below this chat is not useful.
pub const MIN_USEFUL_CTX: u64 = 4096;
/// Fixed part of the compute-buffer allowance.
pub const COMPUTE_BASE_BYTES: u64 = 256 * 1024 * 1024;
/// Batch size the compute allowance assumes (llama-server's default `--ubatch-size`).
pub const UBATCH: u64 = 512;
/// Memory kept free for the OS and other apps when the model runs from system RAM.
pub const RAM_RESERVE_BYTES: u64 = 2_000_000_000;

/// Where the model would run.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Target {
    /// A discrete or integrated GPU with its own free memory figure, and the computer's
    /// (`None` when the system does not say).
    Gpu {
        name: String,
        free_bytes: u64,
        ram_free_bytes: Option<u64>,
    },
    /// Apple silicon: the Metal working-set limit applies.
    AppleUnified { working_set_bytes: u64 },
    /// CPU only.
    Cpu { ram_free_bytes: u64 },
}

impl Target {
    fn primary_bytes(&self) -> u64 {
        match *self {
            Target::Gpu { free_bytes, .. } => free_bytes,
            Target::AppleUnified { working_set_bytes } => working_set_bytes,
            Target::Cpu { ram_free_bytes } => ram_free_bytes.saturating_sub(RAM_RESERVE_BYTES),
        }
    }
    fn place(&self) -> String {
        match self {
            Target::Gpu { .. } => "your GPU has".to_string(),
            Target::AppleUnified { .. } => "this Mac can give the GPU".to_string(),
            Target::Cpu { .. } => "your computer has".to_string(),
        }
    }
    /// Where the model would run, for [`FitResult::device`].
    fn device(&self) -> FitDevice {
        match self {
            Target::Gpu { .. } => FitDevice::GraphicsCard,
            Target::AppleUnified { .. } => FitDevice::AppleGraphics,
            Target::Cpu { .. } => FitDevice::Processor,
        }
    }
    /// The free memory there as the system reports it, for [`FitResult::available_bytes`]:
    /// the processor's figure before [`RAM_RESERVE_BYTES`] is kept back.
    fn free_bytes(&self) -> u64 {
        match *self {
            Target::Gpu { free_bytes, .. } => free_bytes,
            Target::AppleUnified { working_set_bytes } => working_set_bytes,
            Target::Cpu { ram_free_bytes } => ram_free_bytes,
        }
    }
    /// The memory as [`Room`] counts it: on a graphics card its free memory and, with part
    /// of the model on the processor, the computer's memory besides; on Apple silicon the
    /// working set only (this target does not say how much of the rest is free).
    fn room(&self) -> Room {
        match *self {
            Target::Gpu {
                free_bytes,
                ram_free_bytes,
                ..
            } => Room::graphics_card(free_bytes, ram_free_bytes.unwrap_or(0)),
            Target::AppleUnified { working_set_bytes } => Room {
                whole: working_set_bytes,
                partial: working_set_bytes,
                separate: false,
            },
            Target::Cpu { ram_free_bytes } => Room::processor(ram_free_bytes),
        }
    }
    /// The computer's free memory is known (always but on a graphics card whose computer
    /// did not say).
    fn computer_known(&self) -> bool {
        !matches!(
            self,
            Target::Gpu {
                ram_free_bytes: None,
                ..
            }
        )
    }
}

/// Where an estimate puts the model (0.0.6, compare-fit-words: the screens say it in words).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FitDevice {
    /// A graphics card with its own memory.
    GraphicsCard,
    /// Apple silicon: the share of memory Apple graphics may use.
    AppleGraphics,
    /// The processor, from the computer's memory.
    Processor,
}

/// What the estimate is made of. `Default` lets callers write `..Default::default()`, so
/// fields added later (0.0.6 WP1b) need no change at existing call sites.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FitInput {
    /// Sum of the model's file sizes.
    pub weights_bytes: u64,
    /// The projector's size, when images will be used.
    pub projector_bytes: u64,
    /// The conversation memory as a function of the context, from
    /// [`crate::summary::ModelSummary::kv_size`] (0.0.7 P1; it was bytes per token).
    pub kv: KvSize,
    pub vocab_size: u64,
    /// Context the user wants.
    pub ctx: u64,
    /// The model's trained context; suggestions never exceed it.
    pub max_ctx: Option<u64>,
}

impl FitInput {
    /// Output-logits buffer (`vocab × ubatch × 4 bytes`) plus a fixed allowance.
    pub fn compute_bytes(&self) -> u64 {
        COMPUTE_BASE_BYTES.saturating_add(self.vocab_size.saturating_mul(UBATCH).saturating_mul(4))
    }
    /// What the model needs where it runs at `ctx` tokens: weights, projector, the
    /// conversation memory llama.cpp allocates ([`KvSize::at`]) and the working space.
    pub fn needed_at(&self, ctx: u64) -> u64 {
        self.weights_bytes
            .saturating_add(self.projector_bytes)
            .saturating_add(self.kv.at(ctx))
            .saturating_add(self.compute_bytes())
    }
    /// The saved copies llama-server may keep at `ctx` tokens with `spare` bytes of the
    /// computer's memory left for them ([`copies`]; `None` when the computer's memory could
    /// not be read).
    pub fn copies_at(&self, ctx: u64, spare: Option<u64>) -> Copies {
        copies(self.kv.copy_bytes(ctx), spare)
    }
    /// Whether `ctx` tokens fit `room` leaving `spare` free: (green, at all), as
    /// [`Room::verdict`] counts it.
    fn fits(&self, ctx: u64, room: Room, spare: u64) -> (bool, bool) {
        room.fits(self.needed_at(ctx), spare)
    }
}

/// The share of the computer's spare memory the saved copies may take: they get half of
/// what is left after the reserve and the model's own part there, so a long chat leaves as
/// much again for other programs.
pub const COPIES_SHARE_DIVISOR: u64 = 2;

/// The saved copies of the conversation a load lets llama-server keep (0.0.7: context
/// checkpoints, `--ctx-checkpoints`). They build up during a long chat, in the computer's
/// memory wherever the model runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Copies {
    /// How many (`--ctx-checkpoints`): 1 to [`MAX_CTX_CHECKPOINTS`].
    pub count: u32,
    /// What that many take at most.
    pub bytes: u64,
    /// Not even one copy fits the computer's memory that is left after the reserve and the
    /// model's own part there: long chats may run short of memory. Never when that memory
    /// is unknown.
    pub short: bool,
}

/// How many saved copies of `copy_bytes` each ([`KvSize::copy_bytes`]) a load keeps when
/// `spare` bytes of the computer's memory are left after the reserve and the model's own
/// part there: as many as fit [`COPIES_SHARE_DIVISOR`]'s share of it, at least 1 and at most
/// [`MAX_CTX_CHECKPOINTS`]. Never 0: without a copy llama-server reads the whole
/// conversation again whenever the next message does not continue exactly where the last
/// reply ended (a thinking model's reply is shortened in the history), and with one it
/// still goes back to the end of the last message. With `spare` unknown the app cannot
/// size them and passes the most, a quarter of what llama.cpp keeps by itself. A model
/// without copies (`copy_bytes` 0) also gets the most, which costs nothing.
pub fn copies(copy_bytes: u64, spare: Option<u64>) -> Copies {
    let count = match spare {
        Some(spare) if copy_bytes > 0 => {
            let fit = spare / COPIES_SHARE_DIVISOR / copy_bytes;
            u32::try_from(fit)
                .unwrap_or(u32::MAX)
                .clamp(1, MAX_CTX_CHECKPOINTS)
        }
        _ => MAX_CTX_CHECKPOINTS,
    };
    Copies {
        count,
        bytes: u64::from(count).saturating_mul(copy_bytes),
        short: spare.is_some_and(|spare| copy_bytes > spare),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Green,
    Yellow,
    Red,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FitResult {
    pub verdict: Verdict,
    /// What the model needs in the memory that holds it (on a graphics card, the card's
    /// share).
    pub needed_bytes: u64,
    /// The free memory there as the system reports it: the graphics card's, the Metal
    /// working set, or the computer's (the verdict also keeps [`RAM_RESERVE_BYTES`] of the
    /// computer's memory for other programs).
    pub available_bytes: u64,
    /// A smaller context that fits, for yellow verdicts.
    pub suggested_ctx: Option<u64>,
    /// Some layers would run on the CPU (yellow, discrete GPU only).
    pub partial_offload: bool,
    /// Where the model would run.
    pub device: FitDevice,
    /// On a graphics card, what the model needs besides in the computer's memory when its
    /// placement is known (an installed file); 0 otherwise: on Apple graphics and the
    /// processor [`Self::needed_bytes`] counts all of it.
    pub computer_bytes: u64,
    /// With [`Self::computer_bytes`] known, the computer's free memory as the system reports
    /// it (the settings panel's second bar); 0 otherwise.
    pub computer_free_bytes: u64,
    /// Red on a graphics card because the part in the computer's memory is larger than its
    /// free memory less [`RAM_RESERVE_BYTES`] (`placement::judge_in`), not only because the
    /// whole does not fit: the screen then names the computer's memory as what is short.
    pub computer_short: bool,
    /// What the saved copies of a long chat take at most in the computer's memory, with
    /// as many as this computer gives the model ([`Copies`]); 0 for a model without them.
    /// Not in [`Self::needed_bytes`]: they build up during a chat.
    pub copies_bytes: u64,
    /// Not even one saved copy fits what the computer has left ([`Copies::short`]): the
    /// verdict is then yellow at most for that, never red.
    pub copies_short: bool,
    /// The computer's free memory could not be read (a graphics card's computer): the
    /// copies and any part on the processor are not checked against it.
    pub computer_unknown: bool,
    /// The 0.0.4 sentence (logs and scripts). The 0.0.6 screens word the figures instead
    /// (`fitLine` in src/lib/words.ts); empty when the placement judge made the result.
    pub sentence: String,
}

/// Decimal gigabytes with one decimal, the unit download sizes are shown in.
pub fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// The largest context that is green in `room` with [`SPARE_BYTES`] to spare: a multiple
/// of 1024 from [`MIN_USEFUL_CTX`] up to the context asked for and the trained length, or
/// the smaller of those two itself.
fn largest_ctx(input: &FitInput, room: Room) -> Option<u64> {
    let cap = input.max_ctx.map_or(input.ctx, |max| max.min(input.ctx));
    if cap < MIN_USEFUL_CTX {
        return None;
    }
    largest_size(MIN_USEFUL_CTX, cap, |ctx| {
        input.fits(ctx, room, SPARE_BYTES).0
    })
}

/// The context sizes a search chooses from: the floor, the multiples of 1,024 between, and
/// the cap (a floor above the cap gives the cap alone). Never built as a list: the cap may
/// be any number.
pub(crate) struct Sizes {
    floor: u64,
    cap: u64,
    first_multiple: u64,
    multiples: u64,
}

impl Sizes {
    pub(crate) fn new(floor: u64, cap: u64) -> Self {
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
    pub(crate) fn len(&self) -> u64 {
        1 + self.multiples + u64::from(self.cap > self.floor)
    }

    /// The `index`th size, smallest first.
    pub(crate) fn get(&self, index: u64) -> u64 {
        if index == 0 {
            self.floor
        } else if index <= self.multiples {
            self.first_multiple + (index - 1) * 1024
        } else {
            self.cap
        }
    }
}

/// The largest of [`Sizes::new`]`(floor, cap)` that `fits`, or `None` when not even the
/// floor does. `fits` must hold for every size below one that it holds for (memory only
/// grows with the context); a binary search, so a huge cap costs about 60 calls.
pub(crate) fn largest_size(floor: u64, cap: u64, fits: impl Fn(u64) -> bool) -> Option<u64> {
    let sizes = Sizes::new(floor, cap);
    if !fits(sizes.get(0)) {
        return None;
    }
    let (mut low, mut high) = (0u64, sizes.len() - 1);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if fits(sizes.get(mid)) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    Some(sizes.get(low))
}

// ---- 0.0.5: Automatic sizing and fit marks per choice (D2b, WP1b) -------------------

/// The smallest context Automatic picks, unless the model was trained for less (D2b).
pub const AUTO_FLOOR_CTX: u64 = 4096;
/// The largest context Automatic picks (D2b): a longer one leaves less memory for the
/// rest of the computer, and most chats never need it.
pub const AUTO_CAP_CTX: u64 = 32_768;

/// The memory a model may use on this computer for one way of loading it, in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Room {
    /// With the whole model on the main device: free graphics memory, the Metal working
    /// set, or system memory less [`RAM_RESERVE_BYTES`] on the processor.
    pub whole: u64,
    /// With part of the model on the processor ("As much as fits", which the step-down
    /// after a graphics-card memory failure also reaches): `whole` plus system memory less
    /// the reserve. The same as `whole` when the model runs on the processor.
    pub partial: u64,
    /// The main device has memory of its own (a graphics card), so the computer's memory
    /// is `partial − whole` (0.0.7, [`Self::computer`]). False on the processor and on
    /// Apple silicon, where it is all one memory.
    pub separate: bool,
}

impl Room {
    /// The processor only: system memory less the reserve kept for the OS and other apps.
    pub fn processor(ram_free_bytes: u64) -> Self {
        let whole = ram_free_bytes.saturating_sub(RAM_RESERVE_BYTES);
        Self {
            whole,
            partial: whole,
            separate: false,
        }
    }

    /// A graphics card with its own memory; the rest of the model may spill into system
    /// memory.
    pub fn graphics_card(free_bytes: u64, ram_free_bytes: u64) -> Self {
        Self {
            whole: free_bytes,
            partial: free_bytes.saturating_add(ram_free_bytes.saturating_sub(RAM_RESERVE_BYTES)),
            separate: true,
        }
    }

    /// Apple silicon: the graphics side may use the Metal working set; layers beyond it
    /// run on the processor from the same memory, so the partial room is all of system
    /// memory less the reserve (never less than the working set).
    pub fn apple(working_set_bytes: u64, ram_free_bytes: u64) -> Self {
        Self {
            whole: working_set_bytes,
            partial: working_set_bytes.max(ram_free_bytes.saturating_sub(RAM_RESERVE_BYTES)),
            separate: false,
        }
    }

    /// The mark for something that needs `needed` bytes: green when it fits on the main
    /// device with [`SPARE_BYTES`] to spare, yellow when it fits only with part of it on
    /// the processor, red otherwise.
    pub fn verdict(&self, needed: u64) -> Verdict {
        match self.fits(needed, SPARE_BYTES) {
            (true, _) => Verdict::Green,
            (false, true) => Verdict::Yellow,
            (false, false) => Verdict::Red,
        }
    }

    /// (green, at all) for `needed` bytes leaving `spare` free.
    fn fits(&self, needed: u64, spare: u64) -> (bool, bool) {
        let device = needed.saturating_add(spare);
        let green = device <= self.whole;
        (green, green || device <= self.partial)
    }

    /// The computer's memory less the reserve, which the model's own part there and the
    /// saved copies of a long chat share: on a graphics card what there is besides the card
    /// (`partial − whole`); on the processor and on Apple silicon all of it (`partial`).
    pub fn computer(&self) -> u64 {
        if self.separate {
            self.partial.saturating_sub(self.whole)
        } else {
            self.partial
        }
    }
}

/// The fit mark of `input` at a context of `ctx` tokens (the saved copies left out: they
/// never decide it, [`check`]).
pub fn verdict_at(input: &FitInput, ctx: u64, room: Room) -> Verdict {
    room.verdict(input.needed_at(ctx))
}

/// What Automatic picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoCtx {
    /// The context in tokens.
    pub ctx: u64,
    /// Green: the whole model fits on the main device; yellow: only with part of it on
    /// the processor.
    pub verdict: Verdict,
}

/// Automatic (D2b): the largest context that fits with [`SPARE_BYTES`] to spare, a
/// multiple of 1,024 between the floor ([`AUTO_FLOOR_CTX`], or the trained length when it
/// is smaller) and the cap ([`AUTO_CAP_CTX`], or the trained length when it is smaller).
/// The whole model on the main device is preferred: only when not even the floor fits
/// there is the partial room used. `None` when not even the floor fits at all.
/// `input.ctx` is not used; `input.max_ctx` is the trained length.
pub fn auto_ctx(input: &FitInput, room: Room) -> Option<AutoCtx> {
    auto_ctx_with_margin(input, room, SPARE_BYTES)
}

/// [`auto_ctx`] leaving `spare_bytes` free instead of [`SPARE_BYTES`] (0.0.6 item 6: As
/// much as fits with "Leave free on the graphics card" set).
///
/// The memory is not a straight line in the context (0.0.7 P1): short-memory layers stop
/// growing at their window and llama.cpp counts cells in steps of 256, so the largest
/// size is searched for exactly among the sizes Automatic chooses from ([`largest_size`]),
/// each checked with [`FitInput::needed_at`], which only grows with the context. The saved
/// copies of a long chat are not counted: they are sized to what is left ([`copies`]).
pub fn auto_ctx_with_margin(input: &FitInput, room: Room, spare_bytes: u64) -> Option<AutoCtx> {
    let floor = input
        .max_ctx
        .map_or(AUTO_FLOOR_CTX, |m| m.min(AUTO_FLOOR_CTX));
    let cap = input.max_ctx.map_or(AUTO_CAP_CTX, |m| m.min(AUTO_CAP_CTX));
    if let Some(ctx) = largest_size(floor, cap, |ctx| input.fits(ctx, room, spare_bytes).0) {
        return Some(AutoCtx {
            ctx,
            verdict: Verdict::Green,
        });
    }
    largest_size(floor, cap, |ctx| input.fits(ctx, room, spare_bytes).1).map(|ctx| AutoCtx {
        ctx,
        verdict: Verdict::Yellow,
    })
}

/// Will `input` fit `target`? The verdict counts what a load takes ([`FitInput::needed_at`]);
/// the saved copies of a long chat are sized from what the computer has left after the
/// model's own part there ([`copies`]: on a graphics card nothing when the whole model fits
/// on it, else what does not; elsewhere all of it) and, when not even one fits, make a
/// green verdict yellow, never red (0.0.7).
pub fn check(input: &FitInput, target: &Target) -> FitResult {
    let needed = input.needed_at(input.ctx);
    let available = target.primary_bytes();
    let room = target.room();
    let place = target.place();
    let graphics = matches!(target, Target::Gpu { .. });
    let (green, at_all) = room.fits(needed, SPARE_BYTES);
    let computer_part = match (graphics, green) {
        (true, true) => 0,
        (true, false) => needed.saturating_add(SPARE_BYTES).saturating_sub(room.whole),
        (false, _) => needed,
    };
    let known = target.computer_known();
    let copies = input.copies_at(
        input.ctx,
        known.then(|| room.computer().saturating_sub(computer_part)),
    );
    let base = |verdict, suggested_ctx, partial_offload, sentence| FitResult {
        verdict,
        needed_bytes: needed,
        available_bytes: target.free_bytes(),
        suggested_ctx,
        partial_offload,
        device: target.device(),
        computer_bytes: 0,
        computer_free_bytes: 0,
        computer_short: false,
        copies_bytes: copies.bytes,
        copies_short: copies.short,
        computer_unknown: !known,
        sentence,
    };

    if green {
        let free_word = if matches!(target, Target::AppleUnified { .. }) {
            ""
        } else {
            " free"
        };
        let verdict = if copies.short {
            Verdict::Yellow
        } else {
            Verdict::Green
        };
        return base(
            verdict,
            None,
            false,
            format!(
                "Needs about {}; {place} {}{free_word}. Fits.",
                gb(needed),
                gb(available)
            ),
        );
    }

    if let Some(ctx) = largest_ctx(input, room) {
        return base(
            Verdict::Yellow,
            Some(ctx),
            false,
            format!(
                "Needs about {} at this context; {place} {}. Fits with a context of {} tokens.",
                gb(needed),
                gb(available),
                ctx
            ),
        );
    }

    if graphics && at_all {
        return base(
            Verdict::Yellow,
            None,
            true,
            format!(
                "Needs about {}; {place} {} free, so some layers will run on the CPU and replies will be slower.",
                gb(needed),
                gb(available)
            ),
        );
    }

    base(
        Verdict::Red,
        None,
        false,
        format!(
            "Needs about {}; {place} only {}. Too large for this computer.",
            gb(needed),
            gb(available)
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;

    fn input(weights: u64, ctx: u64) -> FitInput {
        FitInput {
            weights_bytes: weights,
            projector_bytes: 0,
            kv: KvSize::per_token(131_072), // 8B-class model, f16 cache
            vocab_size: 128_256,
            ctx,
            max_ctx: Some(131_072),
        }
    }

    #[test]
    fn compute_allowance_is_stated() {
        let i = input(0, 0);
        assert_eq!(i.compute_bytes(), 256 * 1024 * 1024 + 128_256 * 512 * 4);
    }

    #[test]
    fn green_when_one_gb_spare() {
        let i = input(5 * GB, 8192);
        let needed = i.needed_at(8192);
        let r = check(
            &i,
            &Target::Gpu {
                name: "GPU".into(),
                free_bytes: needed + SPARE_BYTES,
                ram_free_bytes: Some(0),
            },
        );
        assert_eq!(r.verdict, Verdict::Green);
        assert!(r.sentence.ends_with("Fits."), "{}", r.sentence);
        assert!(r.sentence.starts_with("Needs about "));
        let r = check(
            &i,
            &Target::Gpu {
                name: "GPU".into(),
                free_bytes: needed + SPARE_BYTES - 1,
                ram_free_bytes: Some(0),
            },
        );
        assert_ne!(r.verdict, Verdict::Green);
    }

    #[test]
    fn yellow_suggests_a_smaller_context() {
        // 5 GB weights; 128k context needs ~17 GB of KV; 12 GB free.
        let i = input(5 * GB, 131_072);
        let r = check(
            &i,
            &Target::Gpu {
                name: "GPU".into(),
                free_bytes: 12 * GB,
                ram_free_bytes: Some(0),
            },
        );
        assert_eq!(r.verdict, Verdict::Yellow);
        let ctx = r.suggested_ctx.unwrap();
        assert!((MIN_USEFUL_CTX..131_072).contains(&ctx) && ctx.is_multiple_of(1024));
        assert!(i.needed_at(ctx) + SPARE_BYTES <= 12 * GB);
        assert!(i.needed_at(ctx + 1024) + SPARE_BYTES > 12 * GB);
    }

    #[test]
    fn yellow_with_partial_offload_on_discrete_gpu() {
        let i = input(14 * GB, 4096);
        let r = check(
            &i,
            &Target::Gpu {
                name: "GPU".into(),
                free_bytes: 8 * GB,
                ram_free_bytes: Some(32 * GB),
            },
        );
        assert_eq!(r.verdict, Verdict::Yellow);
        assert!(r.partial_offload);
        assert!(r.sentence.contains("CPU"));
    }

    #[test]
    fn red_when_nothing_fits() {
        let i = input(40 * GB, 4096);
        let r = check(
            &i,
            &Target::AppleUnified {
                working_set_bytes: 11 * GB,
            },
        );
        assert_eq!(r.verdict, Verdict::Red);
        let r = check(
            &i,
            &Target::Cpu {
                ram_free_bytes: 16 * GB,
            },
        );
        assert_eq!(r.verdict, Verdict::Red);
    }

    #[test]
    fn cpu_keeps_a_reserve_for_the_os() {
        let i = input(3 * GB, 4096);
        let needed = i.needed_at(4096);
        let r = check(
            &i,
            &Target::Cpu {
                ram_free_bytes: needed + SPARE_BYTES,
            },
        );
        assert_ne!(r.verdict, Verdict::Green);
        let r = check(
            &i,
            &Target::Cpu {
                ram_free_bytes: needed + SPARE_BYTES + RAM_RESERVE_BYTES,
            },
        );
        assert_eq!(r.verdict, Verdict::Green);
    }

    #[test]
    fn suggestion_respects_trained_context() {
        let mut i = input(GB, 65_536);
        i.max_ctx = Some(32_768);
        let r = check(
            &i,
            &Target::Gpu {
                name: "GPU".into(),
                free_bytes: 6 * GB,
                ram_free_bytes: Some(0),
            },
        );
        // 65,536 tokens need ~8.6 GB of KV; the suggestion is capped by memory, not above training.
        if let Some(ctx) = r.suggested_ctx {
            assert!(ctx <= 32_768);
        }
    }

    // ---- 0.0.5 Automatic on fixed hardware numbers -----------------------------------

    /// An 8B-class model at Q4_K_M: 4.9 GB, f16 cache 131,072 bytes per token (q8_0:
    /// 69,632), trained for 131,072 tokens.
    fn eight_b(kv: u64) -> FitInput {
        FitInput {
            weights_bytes: 4_900_000_000,
            kv: KvSize::per_token(kv),
            vocab_size: 128_256,
            max_ctx: Some(131_072),
            ..Default::default()
        }
    }
    const F16: u64 = 131_072;
    const Q8: u64 = 69_632;

    #[test]
    fn room_marks() {
        let room = Room::graphics_card(10 * GB, 32 * GB);
        assert_eq!(room.partial, 10 * GB + 30 * GB);
        assert_eq!(room.verdict(9 * GB), Verdict::Green);
        assert_eq!(room.verdict(9 * GB + 1), Verdict::Yellow);
        assert_eq!(room.verdict(39 * GB), Verdict::Yellow);
        assert_eq!(room.verdict(39 * GB + 1), Verdict::Red);
        // The processor has no second place to spill to.
        let cpu = Room::processor(12 * GB);
        assert_eq!((cpu.whole, cpu.partial), (10 * GB, 10 * GB));
        assert_eq!(cpu.verdict(9 * GB), Verdict::Green);
        assert_eq!(cpu.verdict(9 * GB + 1), Verdict::Red);
        // Apple: the working set, then the rest of the same memory.
        let mac = Room::apple(11 * GB, 12 * GB);
        assert_eq!((mac.whole, mac.partial), (11 * GB, 11 * GB));
        let mac = Room::apple(11 * GB, 15 * GB);
        assert_eq!(mac.partial, 13 * GB);
        // Less free memory than the reserve leaves nothing, never an underflow.
        assert_eq!(Room::processor(GB), Room::default());
    }

    /// The processor (16 GB computer, 12 GB available): 10 GB after the reserve.
    #[test]
    fn automatic_on_the_processor() {
        let room = Room::processor(12 * GB);
        let i = eight_b(F16);
        // 10 GB − 4.9 GB weights − 0.53 GB compute − 1 GB spare = 3.57 GB of cache:
        // 27,228 tokens, rounded down to 26 × 1,024.
        let fixed = i.needed_at(0) + SPARE_BYTES;
        assert_eq!((10 * GB - fixed) / F16, 27_228);
        assert_eq!(
            auto_ctx(&i, room),
            Some(AutoCtx {
                ctx: 26_624,
                verdict: Verdict::Green
            })
        );
        assert!(i.needed_at(26_624) + SPARE_BYTES <= room.whole);
        assert!(i.needed_at(26_624 + 1024) + SPARE_BYTES > room.whole);
        // Save memory: the same memory holds the cap.
        assert_eq!(auto_ctx(&eight_b(Q8), room).map(|a| a.ctx), Some(32_768));
    }

    /// A discrete graphics card (16 GB, 15 GB free; and an 8 GB card with 7.5 GB free).
    #[test]
    fn automatic_on_a_discrete_graphics_card() {
        let big = Room::graphics_card(15 * GB, 24 * GB);
        assert_eq!(
            auto_ctx(&eight_b(F16), big),
            Some(AutoCtx {
                ctx: 32_768,
                verdict: Verdict::Green
            }),
            "capped at 32,768 although 65,000 tokens would fit"
        );
        let small = Room::graphics_card(7_500_000_000, 24 * GB);
        // 7.5 − 6.43 GB = 1.07 GB of cache: 8,155 tokens → 7,168.
        assert_eq!(
            auto_ctx(&eight_b(F16), small),
            Some(AutoCtx {
                ctx: 7_168,
                verdict: Verdict::Green
            })
        );
        // Save memory nearly doubles it on the same card (15,350 tokens → 14,336).
        assert_eq!(auto_ctx(&eight_b(Q8), small).map(|a| a.ctx), Some(14_336));
        // A card too small for even the floor: the whole model is preferred, so the
        // partial room is used only now, and the mark says part runs on the processor.
        let tiny = Room::graphics_card(4 * GB, 24 * GB);
        assert_eq!(
            auto_ctx(&eight_b(F16), tiny),
            Some(AutoCtx {
                ctx: 32_768,
                verdict: Verdict::Yellow
            })
        );
        // No system memory to spill into either: not even the floor fits.
        assert_eq!(
            auto_ctx(&eight_b(F16), Room::graphics_card(4 * GB, GB)),
            None
        );
    }

    /// Apple unified memory: a 16 GB Mac whose Metal working set is about 11 GB.
    #[test]
    fn automatic_on_apple_unified_memory() {
        let room = Room::apple(11 * GB, 9 * GB);
        assert_eq!(auto_ctx(&eight_b(F16), room).map(|a| a.ctx), Some(32_768));
        // A 14B-class model (8.4 GB, 48 layers: 196,608 cache bytes per token).
        let fourteen = FitInput {
            weights_bytes: 8_400_000_000,
            kv: KvSize::per_token(196_608),
            vocab_size: 152_064,
            max_ctx: Some(40_960),
            ..Default::default()
        };
        // 11 − 8.4 − 0.58 − 1 GB = 1.02 GB of cache: 5,180 tokens → 5,120.
        assert_eq!(
            auto_ctx(&fourteen, room),
            Some(AutoCtx {
                ctx: 5_120,
                verdict: Verdict::Green
            })
        );
        // A 24 GB model does not fit in 16 GB at all.
        let big = FitInput {
            weights_bytes: 24 * GB,
            ..fourteen
        };
        assert_eq!(auto_ctx(&big, room), None);
    }

    /// The floor and the cap follow a short trained length; an unknown cache size (no
    /// attention layers, or an unreadable header) takes the cap when the rest fits.
    #[test]
    fn automatic_follows_the_trained_length() {
        let room = Room::processor(12 * GB);
        let short = FitInput {
            max_ctx: Some(2048),
            ..eight_b(F16)
        };
        assert_eq!(auto_ctx(&short, room).map(|a| a.ctx), Some(2048));
        let odd = FitInput {
            max_ctx: Some(20_000),
            ..eight_b(F16)
        };
        assert_eq!(auto_ctx(&odd, room).map(|a| a.ctx), Some(20_000));
        let unknown_length = FitInput {
            max_ctx: None,
            ..eight_b(Q8)
        };
        assert_eq!(auto_ctx(&unknown_length, room).map(|a| a.ctx), Some(32_768));
        let no_cache = FitInput {
            kv: KvSize::default(),
            ..eight_b(F16)
        };
        assert_eq!(auto_ctx(&no_cache, room).map(|a| a.ctx), Some(32_768));
        // Just above the floor rounds down to the floor, never below it.
        let fixed = eight_b(F16).needed_at(0) + SPARE_BYTES;
        let tight = Room {
            whole: fixed + F16 * 4_500,
            partial: fixed + F16 * 4_500,
            separate: false,
        };
        assert_eq!(auto_ctx(&eight_b(F16), tight).map(|a| a.ctx), Some(4096));
        let too_tight = Room {
            whole: fixed + F16 * 4_095,
            partial: fixed + F16 * 4_095,
            separate: false,
        };
        assert_eq!(auto_ctx(&eight_b(F16), too_tight), None);
        assert_eq!(verdict_at(&eight_b(F16), 4096, too_tight), Verdict::Red);
        // 0.0.7 P1: llama.cpp gives a context cells in steps of 256 (`GGML_PAD(n_ctx,
        // 256)`), so 4,095 tokens take 4,096 cells, and 3,841 too; 3,840 is 15 steps.
        assert_eq!(verdict_at(&eight_b(F16), 4095, too_tight), Verdict::Red);
        assert_eq!(verdict_at(&eight_b(F16), 3841, too_tight), Verdict::Red);
        assert_eq!(verdict_at(&eight_b(F16), 3840, too_tight), Verdict::Green);
    }

    /// 0.0.6 item 6: the margin can be chosen; at SPARE_BYTES it is `auto_ctx` itself.
    #[test]
    fn auto_ctx_with_margin_equals_auto_ctx_at_the_spare() {
        let rooms = [
            Room::processor(12 * GB),
            Room::graphics_card(15 * GB, 24 * GB),
            Room::graphics_card(7_500_000_000, 24 * GB),
            Room::graphics_card(4 * GB, 24 * GB),
            Room::apple(11 * GB, 9 * GB),
            Room::default(),
            Room {
                whole: u64::MAX,
                partial: u64::MAX,
                separate: false,
            },
        ];
        let inputs = [
            eight_b(F16),
            eight_b(Q8),
            FitInput {
                max_ctx: Some(2048),
                ..eight_b(F16)
            },
            FitInput {
                kv: KvSize::default(),
                ..eight_b(F16)
            },
            FitInput {
                weights_bytes: u64::MAX,
                kv: KvSize::per_token(u64::MAX),
                vocab_size: u64::MAX,
                ..eight_b(F16)
            },
        ];
        for room in rooms {
            for input in &inputs {
                assert_eq!(
                    auto_ctx_with_margin(input, room, SPARE_BYTES),
                    auto_ctx(input, room)
                );
                // Never a panic, whatever the margin.
                let _ = auto_ctx_with_margin(input, room, u64::MAX);
            }
        }
        // A hostile vocabulary size saturates the working space (it overflowed before).
        assert_eq!(inputs[4].compute_bytes(), u64::MAX);
        // 4 GB left free instead of 1 GB: 7,168 tokens on the 7.5 GB card drop below the
        // floor, so only the partial room is left; half a GB free gives more.
        let small = Room::graphics_card(7_500_000_000, 24 * GB);
        assert_eq!(
            auto_ctx_with_margin(&eight_b(F16), small, 4 * GB),
            Some(AutoCtx {
                ctx: 32_768,
                verdict: Verdict::Yellow
            })
        );
        // 7.5 − 6.43 + 0.5 GB = 1.57 GB of cache: 11,970 tokens → 11,264.
        assert_eq!(
            auto_ctx_with_margin(&eight_b(F16), small, 500_000_000),
            Some(AutoCtx {
                ctx: 11_264,
                verdict: Verdict::Green
            })
        );
        assert_eq!(
            auto_ctx_with_margin(&eight_b(F16), Room::processor(12 * GB), u64::MAX),
            None
        );
    }

    #[test]
    fn example_sentence_shape() {
        let i = FitInput {
            weights_bytes: 11_000_000_000,
            projector_bytes: 0,
            kv: KvSize::default(),
            vocab_size: 0,
            ctx: 4096,
            max_ctx: None,
        };
        let r = check(
            &i,
            &Target::Gpu {
                name: "RX".into(),
                free_bytes: 15_200_000_000,
                ram_free_bytes: Some(0),
            },
        );
        assert_eq!(
            r.sentence,
            "Needs about 11.3 GB; your GPU has 15.2 GB free. Fits."
        );
    }

    /// compare-fit-words: the screens word the figures themselves, so the result says where
    /// the model would run and what is free there as the system reports it (the processor's
    /// figure before the reserve); the verdict is unchanged.
    #[test]
    fn says_where_and_the_free_memory_there() {
        let i = input(4 * GB, 4096);
        let gpu = check(
            &i,
            &Target::Gpu {
                name: "RX".into(),
                free_bytes: 15 * GB,
                ram_free_bytes: Some(30 * GB),
            },
        );
        assert_eq!(
            (gpu.device, gpu.available_bytes, gpu.verdict),
            (FitDevice::GraphicsCard, 15 * GB, Verdict::Green)
        );
        let mac = check(
            &i,
            &Target::AppleUnified {
                working_set_bytes: 10 * GB,
            },
        );
        assert_eq!(
            (mac.device, mac.available_bytes),
            (FitDevice::AppleGraphics, 10 * GB)
        );
        // 6 GB free: 4 GB after the reserve, which the verdict keeps back.
        let cpu = check(
            &i,
            &Target::Cpu {
                ram_free_bytes: 6 * GB,
            },
        );
        assert_eq!(
            (cpu.device, cpu.available_bytes, cpu.verdict),
            (FitDevice::Processor, 6 * GB, Verdict::Red)
        );
        assert!(
            cpu.sentence
                .ends_with("only 4.0 GB. Too large for this computer.")
        );
        assert_eq!(
            serde_json::to_value(&gpu).unwrap()["device"],
            serde_json::json!("graphicsCard")
        );
        assert_eq!(
            serde_json::to_value(&cpu).unwrap()["device"],
            serde_json::json!("processor")
        );
    }

    // ---- 0.0.7 P1: short-memory layers (Gemma 4) ---------------------------------------

    const GIB: u64 = 1024 * 1024 * 1024;

    /// Gemma 4 12B's conversation memory in f16, as `ModelSummary::kv_size` reads its
    /// header (`crate::kv` tests): 8 full layers of 16,384 bytes a cell, 40 short-memory
    /// layers of 327,680 bytes for at most 1,536 cells, saved copies of 1,024 cells.
    fn gemma4_kv() -> KvSize {
        KvSize {
            per_cell: 16_384,
            window_per_cell: 327_680,
            window: 1024,
            fixed: 0,
            checkpoint_per_cell: 327_680,
            copy_fixed: 0,
        }
    }

    /// One saved copy of Gemma 4 12B in f16: 1,024 cells of 327,680 bytes (320 MiB).
    const GEMMA4_COPY: u64 = 335_544_320;

    /// What 0.0.6 counted for the same header: every layer at full length with the full
    /// layers' head size, 328 heads × (512 + 512) × 2 bytes a token.
    fn gemma4_kv_0_0_6() -> KvSize {
        KvSize::per_token(671_744)
    }

    /// The files of `gemma-4-12B-coder-fable5-composer2.5-v1-uncensored-heretic-GGUF`, with
    /// its image file (122,031,232 bytes) as the repository page counts it.
    fn gemma4(weights: u64, kv: KvSize) -> FitInput {
        FitInput {
            weights_bytes: weights,
            projector_bytes: 122_031_232,
            kv,
            vocab_size: 262_144,
            ctx: 65_536,
            max_ctx: Some(262_144),
        }
    }
    const Q8_0: u64 = 12_669_649_120;
    const Q6_K: u64 = 9_786_024_160;
    const Q4_K_M: u64 = 7_381_385_440;

    /// The owner's screen: a 16 GB graphics card with 15.1 GB free (the app shows binary
    /// gigabytes) and 65,536 tokens. 0.0.6 said Q8_0 was too large ("needs about 53.7
    /// GB") and Q6_K fitted only up to 6,144 tokens; both fit.
    #[test]
    fn the_owners_gemma4_screen() {
        let card = Target::Gpu {
            name: "AMD Radeon RX 9070 XT".into(),
            free_bytes: 15 * GIB + GIB / 10,
            ram_free_bytes: Some(24 * GB),
        };
        // 0.0.6's numbers, reproduced: 57.6 GB (53.7 binary GB), too large; Q6_K up to
        // 6,144 tokens.
        let old = check(&gemma4(Q8_0, gemma4_kv_0_0_6()), &card);
        assert_eq!(old.verdict, Verdict::Red);
        assert_eq!(
            (old.needed_bytes as f64 / GIB as f64 * 10.0).round() / 10.0,
            53.7
        );
        let old = check(&gemma4(Q6_K, gemma4_kv_0_0_6()), &card);
        assert_eq!(
            (old.verdict, old.suggested_ctx),
            (Verdict::Yellow, Some(6144))
        );
        // Now: Q8_0 needs 12,669,649,120 + 122,031,232 + 1,577,058,304 (the memory llama.cpp
        // allocates at 65,536 tokens) + 805,306,368 (working space) = 15,174,045,024 bytes,
        // which fits the card with the 1 GB spare. With 22 GB of the computer's memory left
        // after the reserve, a long chat may keep the most saved copies, 8 of 320 MiB
        // (2.5 GiB), besides.
        let q8 = check(&gemma4(Q8_0, gemma4_kv()), &card);
        assert_eq!(q8.verdict, Verdict::Green);
        assert_eq!(q8.needed_bytes, 15_174_045_024);
        assert_eq!(q8.copies_bytes, 8 * GEMMA4_COPY);
        assert_eq!(q8.copies_bytes, 5 * GIB / 2);
        assert!(!q8.copies_short && !q8.computer_unknown && !q8.computer_short);
        assert_eq!(q8.computer_bytes, 0);
        let q6 = check(&gemma4(Q6_K, gemma4_kv()), &card);
        assert_eq!(q6.verdict, Verdict::Green);
        // With 15.05 GB free Q8_0 only just misses the spare at 65,536 tokens: a shorter
        // memory fits (KV ≤ free − spare − the rest: 64,512 tokens), never "too large".
        let tighter = Target::Gpu {
            name: "AMD Radeon RX 9070 XT".into(),
            free_bytes: 15 * GIB + GIB / 20,
            ram_free_bytes: Some(24 * GB),
        };
        let q8 = check(&gemma4(Q8_0, gemma4_kv()), &tighter);
        assert_eq!(
            (q8.verdict, q8.suggested_ctx),
            (Verdict::Yellow, Some(64_512))
        );
        // At the trained 262,144 tokens Q4_K_M still fits whole; Q8_0 up to 67,584 tokens:
        // 16,213,501,542 free − 1,000,000,000 spare − 13,596,986,720 for the rest leave
        // 1,616,514,822 bytes, of which the short layers take 503,316,480 and the full ones
        // 16,384 a token (67,944 tokens, 66 × 1,024).
        let mut long = gemma4(Q4_K_M, gemma4_kv());
        long.ctx = 262_144;
        assert_eq!(check(&long, &card).verdict, Verdict::Green);
        let mut long = gemma4(Q8_0, gemma4_kv());
        long.ctx = 262_144;
        assert_eq!(check(&long, &card).suggested_ctx, Some(67_584));
    }

    /// Automatic on the same card reaches its cap instead of a few thousand tokens.
    #[test]
    fn gemma4_automatic_on_a_16_gb_card() {
        let room = Room::graphics_card(15 * GIB + GIB / 10, 24 * GB);
        // 0.0.6: (free − weights − working space − spare) / 671,744 = 10,460 tokens.
        let mut old = gemma4(Q4_K_M, gemma4_kv_0_0_6());
        old.projector_bytes = 0;
        assert_eq!(auto_ctx(&old, room).map(|a| a.ctx), Some(10_240));
        let mut q4 = gemma4(Q4_K_M, gemma4_kv());
        q4.projector_bytes = 0;
        assert_eq!(
            auto_ctx(&q4, room),
            Some(AutoCtx {
                ctx: AUTO_CAP_CTX,
                verdict: Verdict::Green
            })
        );
        // Q8_0: 0.0.6 had to put part of it on the processor; now all of it fits the card.
        let mut old = gemma4(Q8_0, gemma4_kv_0_0_6());
        old.projector_bytes = 0;
        assert_eq!(
            auto_ctx(&old, room).map(|a| a.verdict),
            Some(Verdict::Yellow)
        );
        let mut q8 = gemma4(Q8_0, gemma4_kv());
        q8.projector_bytes = 0;
        assert_eq!(
            auto_ctx(&q8, room),
            Some(AutoCtx {
                ctx: AUTO_CAP_CTX,
                verdict: Verdict::Green
            })
        );
    }

    /// Automatic's answer is the largest size it chooses from that fits, found exactly for
    /// memory that is not a straight line (short-memory layers, 256-cell steps): the same as
    /// trying every size.
    #[test]
    fn automatic_is_exact_for_short_memory_layers() {
        let gpt_oss = KvSize {
            per_cell: 24_576,
            window_per_cell: 24_576,
            window: 128,
            fixed: 0,
            checkpoint_per_cell: 24_576,
            copy_fixed: 0,
        };
        let kvs = [
            gemma4_kv(),
            gpt_oss,
            KvSize::per_token(131_072),
            KvSize::default(),
        ];
        for kv in kvs {
            for max_ctx in [None, Some(2048), Some(20_000), Some(262_144)] {
                let input = FitInput {
                    weights_bytes: 7 * GB,
                    kv,
                    vocab_size: 262_144,
                    max_ctx,
                    ..Default::default()
                };
                for free in (8..18).map(|g| g * GB / 2 + 123_456_789) {
                    for ram in [0, 3 * GB, 40 * GB] {
                        let room = Room::graphics_card(free, ram);
                        for spare in [0, SPARE_BYTES, 3 * GB] {
                            let got = auto_ctx_with_margin(&input, room, spare);
                            let floor = max_ctx.map_or(AUTO_FLOOR_CTX, |m| m.min(AUTO_FLOOR_CTX));
                            let cap = max_ctx.map_or(AUTO_CAP_CTX, |m| m.min(AUTO_CAP_CTX));
                            let sizes = Sizes::new(floor, cap);
                            let all: Vec<u64> = (0..sizes.len()).map(|i| sizes.get(i)).collect();
                            let best = |which: fn((bool, bool)) -> bool| {
                                all.iter()
                                    .copied()
                                    .take_while(|&c| which(input.fits(c, room, spare)))
                                    .last()
                            };
                            let want = best(|f| f.0)
                                .map(|ctx| AutoCtx {
                                    ctx,
                                    verdict: Verdict::Green,
                                })
                                .or_else(|| {
                                    best(|f| f.1).map(|ctx| AutoCtx {
                                        ctx,
                                        verdict: Verdict::Yellow,
                                    })
                                });
                            assert_eq!(got, want, "{kv:?} {max_ctx:?} {free} {ram} {spare}");
                        }
                    }
                }
            }
        }
    }

    /// How many saved copies a load keeps: as many as fit half of what the computer has
    /// left after the reserve and the model's own part there, from 1 to 8; the most when
    /// that is unknown or the model keeps none.
    #[test]
    fn copies_take_half_of_what_is_left() {
        let copy = GB;
        let at = |spare: u64| copies(copy, Some(spare));
        assert_eq!(
            at(0),
            Copies {
                count: 1,
                bytes: GB,
                short: true
            }
        );
        // One copy fits but takes more than half: still one, never none.
        assert_eq!((at(GB).count, at(GB).short), (1, false));
        assert_eq!(at(3 * GB).count, 1);
        assert_eq!(at(4 * GB).count, 2);
        assert_eq!((at(17 * GB).count, at(17 * GB).bytes), (8, 8 * GB));
        assert_eq!(at(1000 * GB).count, MAX_CTX_CHECKPOINTS);
        assert_eq!(at(u64::MAX).count, MAX_CTX_CHECKPOINTS);
        // The computer's memory unknown: the most, and never short.
        assert_eq!(
            copies(copy, None),
            Copies {
                count: 8,
                bytes: 8 * GB,
                short: false
            }
        );
        // A model without copies.
        for spare in [None, Some(0), Some(GB)] {
            assert_eq!(
                copies(0, spare),
                Copies {
                    count: 8,
                    bytes: 0,
                    short: false
                }
            );
        }
        // Hostile sizes saturate.
        assert_eq!(copies(u64::MAX, Some(u64::MAX)).count, 1);
        assert_eq!(copies(u64::MAX, None).bytes, u64::MAX);
    }

    /// The saved copies never turn a verdict red or change the context (review M2): when
    /// not even one fits what is left, the verdict is yellow at most. The room they get:
    /// the computer's memory less the reserve, and less the model where it is there.
    #[test]
    fn copies_never_make_a_model_too_large() {
        let input = FitInput {
            ctx: 32_768,
            projector_bytes: 0,
            ..gemma4(Q8_0, gemma4_kv())
        };
        let card = |ram: Option<u64>| Target::Gpu {
            name: "GPU".into(),
            free_bytes: 15 * GIB,
            ram_free_bytes: ram,
        };
        // Gemma 4 Q8_0 at 32,768 tokens fits a 15 GB card (15,515,142,880 bytes with the
        // spare). 0.0.7 before the review: red with 4 GB of the computer's memory free,
        // because 8 copies (2.5 GiB) did not fit the 2 GB left; now 2 GB / 2 = 1 GB holds 2.
        let r = check(&input, &card(Some(4 * GB)));
        assert_eq!(r.verdict, Verdict::Green);
        assert_eq!((r.copies_bytes, r.copies_short), (2 * GEMMA4_COPY, false));
        assert_eq!(
            auto_ctx(&input, Room::graphics_card(15 * GIB, 4 * GB)),
            Some(AutoCtx {
                ctx: 32_768,
                verdict: Verdict::Green
            })
        );
        // 2.2 GB free: one copy (320 MiB) does not fit the 200 MB left: yellow, not red.
        let r = check(&input, &card(Some(2_200_000_000)));
        assert_eq!(r.verdict, Verdict::Yellow);
        assert_eq!((r.copies_bytes, r.copies_short), (GEMMA4_COPY, true));
        assert_eq!((r.suggested_ctx, r.partial_offload), (None, false));
        // Nothing free at all: the same.
        assert_eq!(check(&input, &card(Some(0))).verdict, Verdict::Yellow);
        // The computer's memory unknown (review m5): not checked, and said so.
        let r = check(&input, &card(None));
        assert_eq!(r.verdict, Verdict::Green);
        assert_eq!((r.copies_bytes, r.copies_short), (8 * GEMMA4_COPY, false));
        assert!(r.computer_unknown);
        assert!(!check(&input, &card(Some(0))).computer_unknown);
        // On the processor the model is in the same memory: what it needs, 14,515,142,880
        // bytes, is not counted twice; the copies get half of the rest.
        let r = check(
            &input,
            &Target::Cpu {
                ram_free_bytes: 20 * GB,
            },
        );
        assert_eq!(r.needed_bytes, input.needed_at(32_768));
        let rest = 18 * GB - input.needed_at(32_768);
        assert_eq!(r.copies_bytes, (rest / 2 / GEMMA4_COPY) * GEMMA4_COPY);
        assert_eq!((r.verdict, r.computer_bytes), (Verdict::Green, 0));
        // With part of the model on the processor (a 12 GB card), the part that does not
        // fit the card is the computer's too.
        let r = check(
            &input,
            &Target::Gpu {
                name: "GPU".into(),
                free_bytes: 12 * GB,
                ram_free_bytes: Some(32 * GB),
            },
        );
        assert!(r.partial_offload);
        let spill = input.needed_at(32_768) + SPARE_BYTES - 12 * GB;
        let left = 30 * GB - spill;
        assert_eq!(r.copies_bytes, (left / 2 / GEMMA4_COPY).min(8) * GEMMA4_COPY);
        // A model without copies takes nothing more.
        let r = check(&input_without_window(), &card(Some(0)));
        assert_eq!((r.copies_bytes, r.copies_short, r.verdict), (0, false, Verdict::Green));
    }

    /// The models whose copies are largest, on a 16 GB card with 16 GB of computer memory
    /// of which 8 GB is free (review M2): never too large because of the copies. Their
    /// conversation memory as `crate::kv` reads their headers (see its tests): OLMo 3 7B
    /// (24 short-memory layers of 32 heads of 128 values, a window of 4,096: a copy of 1.5
    /// GiB, 12 GiB for 8), Llama 4 Scout (36 chunked layers of 8 heads, 8,192 tokens: 1.125
    /// GiB) and Gemma 3 27B (52 short-memory layers of 16 heads, 1,024 tokens: 416 MiB).
    #[test]
    fn long_window_models_on_a_16_gb_card() {
        let olmo3 = KvSize {
            per_cell: 8 * 16_384,
            window_per_cell: 24 * 16_384,
            window: 4096,
            fixed: 0,
            checkpoint_per_cell: 24 * 16_384,
            copy_fixed: 0,
        };
        assert_eq!(olmo3.copy_bytes(32_768), 3 * GIB / 2);
        let input = FitInput {
            weights_bytes: 4_470_000_000,
            kv: olmo3,
            vocab_size: 100_278,
            ctx: 32_768,
            max_ctx: Some(65_536),
            ..Default::default()
        };
        let card = Target::Gpu {
            name: "GPU".into(),
            free_bytes: 15 * GIB,
            ram_free_bytes: Some(8 * GB),
        };
        // About 11 GB on the card; 6 GB left in the computer's memory: one copy (1.5 GiB),
        // where 0.0.7 before the review said red (12 GiB of copies, "not enough").
        let r = check(&input, &card);
        assert!(input.needed_at(32_768) < 11_100_000_000);
        assert_eq!(r.verdict, Verdict::Green);
        assert_eq!((r.copies_bytes, r.copies_short), (3 * GIB / 2, false));
        assert_eq!(auto_ctx(&input, card.room()).map(|a| a.ctx), Some(32_768));
        // Llama 4 Scout's and Gemma 3 27B's copies, at the same computer's 6 GB left.
        for (copy, count) in [(9 * GIB / 8, 2), (416 * 1024 * 1024, 6)] {
            let c = copies(copy, Some(6 * GB));
            assert_eq!((c.count, c.short), (count, false), "{copy}");
        }
        // Every amount free: never red, and Automatic never gives up, because of copies.
        for ram in (0..=16).map(|g| g * GB) {
            let card = Target::Gpu {
                name: "GPU".into(),
                free_bytes: 15 * GIB,
                ram_free_bytes: Some(ram),
            };
            let r = check(&input, &card);
            assert_ne!(r.verdict, Verdict::Red, "{ram}");
            assert_eq!(r.copies_short, ram < 2 * GB + 3 * GIB / 2, "{ram}");
            assert!(auto_ctx(&input, card.room()).is_some(), "{ram}");
        }
    }

    fn input_without_window() -> FitInput {
        input(4 * GB, 8192)
    }
}
