//! The first-tier fit check (ARCHITECTURE.md 11): will this model fit before it is
//! downloaded? Weights + KV cache + projector + a stated compute allowance, compared
//! with the memory the selected backend can use.

use serde::Serialize;

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
    /// A discrete or integrated GPU with its own free memory figure.
    Gpu {
        name: String,
        free_bytes: u64,
        ram_free_bytes: u64,
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
}

/// What the estimate is made of.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FitInput {
    /// Sum of the model's file sizes.
    pub weights_bytes: u64,
    /// The projector's size, when images will be used.
    pub projector_bytes: u64,
    /// From [`crate::summary::ModelSummary::kv_bytes_per_token`].
    pub kv_bytes_per_token: u64,
    pub vocab_size: u64,
    /// Context the user wants.
    pub ctx: u64,
    /// The model's trained context; suggestions never exceed it.
    pub max_ctx: Option<u64>,
}

impl FitInput {
    /// Output-logits buffer (`vocab × ubatch × 4 bytes`) plus a fixed allowance.
    pub fn compute_bytes(&self) -> u64 {
        COMPUTE_BASE_BYTES + self.vocab_size.saturating_mul(UBATCH).saturating_mul(4)
    }
    pub fn needed_at(&self, ctx: u64) -> u64 {
        self.weights_bytes
            .saturating_add(self.projector_bytes)
            .saturating_add(self.kv_bytes_per_token.saturating_mul(ctx))
            .saturating_add(self.compute_bytes())
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
    pub needed_bytes: u64,
    pub available_bytes: u64,
    /// A smaller context that fits, for yellow verdicts.
    pub suggested_ctx: Option<u64>,
    /// Some layers would run on the CPU (yellow, discrete GPU only).
    pub partial_offload: bool,
    /// One sentence for the UI.
    pub sentence: String,
}

/// Decimal gigabytes with one decimal, the unit download sizes are shown in.
pub fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// The largest context (a multiple of 1024, at least [`MIN_USEFUL_CTX`]) that leaves
/// [`SPARE_BYTES`] free in `available`.
fn largest_ctx(input: &FitInput, available: u64) -> Option<u64> {
    let fixed = input.needed_at(0).saturating_add(SPARE_BYTES);
    if fixed >= available || input.kv_bytes_per_token == 0 {
        return None;
    }
    let tokens = (available - fixed) / input.kv_bytes_per_token;
    let mut ctx = tokens / 1024 * 1024;
    if let Some(max) = input.max_ctx {
        ctx = ctx.min(max);
    }
    ctx = ctx.min(input.ctx);
    (ctx >= MIN_USEFUL_CTX).then_some(ctx)
}

pub fn check(input: &FitInput, target: &Target) -> FitResult {
    let needed = input.needed_at(input.ctx);
    let available = target.primary_bytes();
    let place = target.place();
    let base = |verdict, suggested_ctx, partial_offload, sentence| FitResult {
        verdict,
        needed_bytes: needed,
        available_bytes: available,
        suggested_ctx,
        partial_offload,
        sentence,
    };

    if needed.saturating_add(SPARE_BYTES) <= available {
        let free_word = if matches!(target, Target::AppleUnified { .. }) {
            ""
        } else {
            " free"
        };
        return base(
            Verdict::Green,
            None,
            false,
            format!(
                "Needs about {}; {place} {}{free_word}. Fits.",
                gb(needed),
                gb(available)
            ),
        );
    }

    if let Some(ctx) = largest_ctx(input, available) {
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

    if let Target::Gpu { ram_free_bytes, .. } = *target {
        let combined = available.saturating_add(ram_free_bytes.saturating_sub(RAM_RESERVE_BYTES));
        if needed.saturating_add(SPARE_BYTES) <= combined {
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
            kv_bytes_per_token: 131_072, // 8B-class model, f16 cache
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
                ram_free_bytes: 0,
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
                ram_free_bytes: 0,
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
                ram_free_bytes: 0,
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
                ram_free_bytes: 32 * GB,
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
                ram_free_bytes: 0,
            },
        );
        // 65,536 tokens need ~8.6 GB of KV; the suggestion is capped by memory, not above training.
        if let Some(ctx) = r.suggested_ctx {
            assert!(ctx <= 32_768);
        }
    }

    #[test]
    fn example_sentence_shape() {
        let i = FitInput {
            weights_bytes: 11_000_000_000,
            projector_bytes: 0,
            kv_bytes_per_token: 0,
            vocab_size: 0,
            ctx: 4096,
            max_ctx: None,
        };
        let r = check(
            &i,
            &Target::Gpu {
                name: "RX".into(),
                free_bytes: 15_200_000_000,
                ram_free_bytes: 0,
            },
        );
        assert_eq!(
            r.sentence,
            "Needs about 11.3 GB; your GPU has 15.2 GB free. Fits."
        );
    }
}
