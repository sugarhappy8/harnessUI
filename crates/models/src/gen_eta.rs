//! How long a local run takes (studio-local-design.md L5, "Time estimate"), and the word a
//! model's fit gets on screen. No time is promised unless something was measured: a first
//! guess comes only from the catalog's `speed[]` points (the benchmark's), this computer's
//! own runs replace it after the first one, and with neither there is no estimate.

use serde::Serialize;

use crate::fit::Target;
use crate::gen_catalog::{GenEntry, GenKind, SpeedPoint};
use crate::gen_fit::{GenFit, GenFitResult, GenSize, tokens};

/// Video on Apple silicon takes this much longer than measured: a fanless Mac slows down
/// as it warms, and one cannot be told apart reliably, so every Mac gets it.
pub const APPLE_VIDEO_PAD: f64 = 1.3;
/// "Slow here" for a picture whose run takes more than 2 minutes.
pub const SLOW_PICTURE_MS: u64 = 120_000;
/// "Slow here" for a clip whose run takes more than 3 minutes.
pub const SLOW_CLIP_MS: u64 = 180_000;
/// 15 minutes: "very slow", the amber line, and Studio offers a cloud model.
pub const VERY_SLOW_MS: u64 = 900_000;

/// The drawing grows with the tokens to this power.
const TOKEN_POWER: f64 = 1.3;
/// A first guess spans 0.75× to 1.5× its figure.
const GUESS_RANGE: (f64, f64) = (0.75, 1.5);
/// A time measured here spans 0.9× to 1.15×.
const MEASURED_RANGE: (f64, f64) = (0.9, 1.15);
/// About how much of a Mac's memory macOS lets Metal use, to tell its size from the
/// working set.
const MAC_WORKING_SET_SHARE: f64 = 2.0 / 3.0;

/// How long a run takes, as a range in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Eta {
    pub low_ms: u64,
    pub high_ms: u64,
    /// From this computer's own runs, not the catalog's first guess.
    pub measured: bool,
}

/// What a run's speed depends on: the engine's backend and the device's memory, as the
/// catalog's speed points name them (`vulkan-8`, `metal-16`, `cpu`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceClass {
    /// `vulkan`, `metal` or `cpu`.
    pub backend: String,
    /// The device's memory in whole GB; 0 on the processor.
    pub memory_gb: u32,
}

/// A local run of this model on this device, as recorded when it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeasuredRun {
    pub size: GenSize,
    pub steps: u32,
    pub pictures: u32,
    /// The whole run, the load included.
    pub elapsed_ms: u64,
}

/// The word a model's fit gets on screen (Tasks L15, L16 and the webview).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FitWord {
    /// "Fits".
    Fits,
    /// "Slow here".
    Slow,
    /// "Fits after the chat model is unloaded".
    AfterUnload,
    /// Not on this computer.
    NotHere,
}

/// The class of the device a run uses: `backend` with the device's memory in whole GB (the
/// free graphics memory; on Apple silicon the Mac's memory, from the Metal working set,
/// which macOS sets to about two thirds of it), or `cpu` with 0 when the run is on the
/// processor (the backend `cpu`, or a computer that runs models there).
pub fn device_class(target: &Target, backend: &str) -> DeviceClass {
    let memory = match target {
        _ if backend == "cpu" => None,
        Target::Cpu { .. } => None,
        Target::Gpu { free_bytes, .. } => Some(*free_bytes as f64),
        Target::AppleUnified {
            working_set_bytes, ..
        } => Some(*working_set_bytes as f64 / MAC_WORKING_SET_SHARE),
    };
    match memory {
        Some(bytes) => DeviceClass {
            backend: backend.to_string(),
            memory_gb: (bytes / 1e9).round() as u32,
        },
        None => DeviceClass {
            backend: "cpu".to_string(),
            memory_gb: 0,
        },
    }
}

/// How long `pictures` pictures (1 for a clip) of `entry` at `size` and `steps` take on
/// `class`.
///
/// With `measured` runs (this model's on this device), their median scaled to these
/// settings as a speed point scales, as [0.9×, 1.15×] and `measured`; the load is once a
/// run whatever its size, so the point's load (at most the whole run) stays as it was and
/// only the rest of each run is scaled. Otherwise from the
/// speed point of the same backend nearest in memory: load + per picture (steps × seconds
/// a step × (tokens / the point's tokens)^1.3 + decode × the pixel-frames ratio), as
/// [0.75×, 1.5×]. Video on Apple silicon (`metal`) grows by [`APPLE_VIDEO_PAD`]. No point of
/// the same backend and no measured run: `None`.
pub fn estimate(
    entry: &GenEntry,
    size: GenSize,
    steps: u32,
    pictures: u32,
    class: &DeviceClass,
    measured: &[MeasuredRun],
) -> Option<Eta> {
    let point = nearest_point(entry, class);
    let scale = Scale { entry, point };
    let (load, work) = scale.seconds(size, steps, pictures);
    let runs = measured
        .iter()
        .filter_map(|m| {
            let (then_load, then_work) = scale.seconds(m.size, m.steps, m.pictures);
            let elapsed = m.elapsed_ms as f64;
            let fixed = (then_load * 1000.0).min(elapsed);
            (then_work > 0.0 && m.elapsed_ms > 0)
                .then(|| fixed + (elapsed - fixed) * work / then_work)
        })
        .collect();
    let (ms, (low, high), measured) = match (median(runs), point) {
        (Some(ms), _) => (ms, MEASURED_RANGE, true),
        (None, Some(_)) => ((load + work) * 1000.0, GUESS_RANGE, false),
        (None, None) => return None,
    };
    let pad = if class.backend == "metal" && entry.kind == GenKind::Video {
        APPLE_VIDEO_PAD
    } else {
        1.0
    };
    Some(Eta {
        low_ms: (ms * pad * low).round() as u64,
        high_ms: (ms * pad * high).round() as u64,
        measured,
    })
}

/// How a run's time grows with its settings: from a speed point, its load once plus, per
/// picture, its steps scaled by tokens^1.3 and its decode by pixel-frames; with no point,
/// in units of steps at a size alone (and no load), which only a run measured here turns
/// into time.
struct Scale<'a> {
    entry: &'a GenEntry,
    point: Option<&'a SpeedPoint>,
}

impl Scale<'_> {
    /// (the load, the pictures' steps and decoding), in seconds.
    fn seconds(&self, size: GenSize, steps: u32, pictures: u32) -> (f64, f64) {
        let pictures = f64::from(pictures.max(1));
        let steps = f64::from(steps);
        let tokens_here = tokens(self.entry, size) as f64;
        match self.point {
            Some(p) => {
                let at = GenSize {
                    width: p.width,
                    height: p.height,
                    frames: p.frames.max(1),
                };
                let tokens_ratio = tokens_here / tokens(self.entry, at) as f64;
                let pixel_ratio = size.pixel_frames() as f64 / at.pixel_frames() as f64;
                let drawing = steps * p.seconds_per_step * tokens_ratio.powf(TOKEN_POWER);
                (
                    p.load_seconds,
                    pictures * (drawing + p.decode_seconds * pixel_ratio),
                )
            }
            None => (0.0, pictures * steps * tokens_here.powf(TOKEN_POWER)),
        }
    }
}

/// The usable speed point of the class's backend nearest in memory (of two as near, the
/// smaller device's).
fn nearest_point<'a>(entry: &'a GenEntry, class: &DeviceClass) -> Option<&'a SpeedPoint> {
    entry
        .speed
        .iter()
        .filter(|p| usable(p))
        .filter_map(|p| {
            let (backend, gb) = parse_class(&p.device_class)?;
            (backend == class.backend).then_some((gb.abs_diff(class.memory_gb), gb, p))
        })
        .min_by_key(|&(distance, gb, _)| (distance, gb))
        .map(|(_, _, p)| p)
}

/// `<backend>-<GB>` (`vulkan-8`), or `cpu`.
fn parse_class(class: &str) -> Option<(&str, u32)> {
    if class == "cpu" {
        return Some(("cpu", 0));
    }
    let (backend, gb) = class.rsplit_once('-')?;
    Some((backend, gb.parse().ok()?))
}

/// A point with a size and times that are numbers of seconds.
fn usable(p: &SpeedPoint) -> bool {
    p.width > 0
        && p.height > 0
        && [p.load_seconds, p.seconds_per_step, p.decode_seconds]
            .iter()
            .all(|s| s.is_finite() && *s >= 0.0)
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    match n {
        0 => None,
        _ if n % 2 == 1 => Some(values[n / 2]),
        _ => Some((values[n / 2 - 1] + values[n / 2]) / 2.0),
    }
}

/// The middle of the range.
fn midpoint(eta: &Eta) -> u64 {
    ((u128::from(eta.low_ms) + u128::from(eta.high_ms)) / 2) as u64
}

/// "Slow here": the range's middle above [`SLOW_PICTURE_MS`] for a picture model, above
/// [`SLOW_CLIP_MS`] for a video model.
pub fn is_slow(entry: &GenEntry, eta: &Eta) -> bool {
    let limit = match entry.kind {
        GenKind::Video => SLOW_CLIP_MS,
        GenKind::Image | GenKind::ImageEdit => SLOW_PICTURE_MS,
    };
    midpoint(eta) > limit
}

/// The time left in a run at `step` of `steps` in the current picture, with
/// `pictures_left` pictures to finish (the current one included; 0 when none is left), at
/// the live `sec_per_step` and with `tail_ms` after each picture's steps (decoding and
/// saving).
pub fn left_ms(step: u32, steps: u32, pictures_left: u32, sec_per_step: f64, tail_ms: u64) -> u64 {
    if pictures_left == 0 {
        return 0;
    }
    let step_ms = sec_per_step.max(0.0) * 1000.0;
    let tail = tail_ms as f64;
    let this_one = f64::from(steps.saturating_sub(step)) * step_ms + tail;
    let later = f64::from(pictures_left - 1) * (f64::from(steps) * step_ms + tail);
    (this_one + later).round() as u64
}

/// A time as people read it: to the nearest 5 s under a minute, 30 s under 10 minutes,
/// whole minutes above.
pub fn round_for_display(ms: u64) -> u64 {
    let step = match ms {
        0..60_000 => 5_000,
        60_000..600_000 => 30_000,
        _ => 60_000,
    };
    ms.saturating_add(step / 2) / step * step
}

/// The fit a run will have, and whether the chat model must be unloaded for it: `fit` when
/// it is Green or one part at a time (it runs beside the chat model); else `after_unload`
/// when that is not Red; else `fit`.
pub fn running_fit(r: &GenFitResult) -> (GenFit, bool) {
    if r.fit.on_device() {
        return (r.fit, false);
    }
    match r.after_unload {
        Some(after) if after != GenFit::Red => (after, true),
        _ => (r.fit, false),
    }
}

/// In this order, so a job that fits once the chat model is unloaded is never called red or
/// slow because of the chat model:
/// 1. `fit` Green or one part at a time → Fits, or Slow for a picture on the processor or
///    when `is_slow(eta)`;
/// 2. `after_unload` Green or one part at a time → AfterUnload;
/// 3. partly on the processor, now or after an unload → Slow;
/// 4. otherwise NotHere.
pub fn fit_word(entry: &GenEntry, r: &GenFitResult, eta: Option<&Eta>) -> FitWord {
    if r.fit.on_device() {
        let picture_on_processor = r.processor_only && entry.kind != GenKind::Video;
        return if picture_on_processor || eta.is_some_and(|e| is_slow(entry, e)) {
            FitWord::Slow
        } else {
            FitWord::Fits
        };
    }
    if r.after_unload.is_some_and(GenFit::on_device) {
        return FitWord::AfterUnload;
    }
    if r.fit == GenFit::PartlyOnProcessor || r.after_unload == Some(GenFit::PartlyOnProcessor) {
        return FitWord::Slow;
    }
    FitWord::NotHere
}

/// Very slow, so Studio offers a cloud model: NotHere, or the range's middle above
/// [`VERY_SLOW_MS`].
pub fn very_slow(word: FitWord, eta: Option<&Eta>) -> bool {
    word == FitWord::NotHere || over_15_min(word, eta)
}

/// The range's middle above [`VERY_SLOW_MS`]; never for NotHere.
pub fn over_15_min(word: FitWord, eta: Option<&Eta>) -> bool {
    word != FitWord::NotHere && eta.is_some_and(|e| midpoint(e) > VERY_SLOW_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gen_fit::gen_fit;
    use crate::gen_fit::tests::{GB, apple, clip, cpu, gpu, klein, sq, wan22, zimage};

    fn class(backend: &str, memory_gb: u32) -> DeviceClass {
        DeviceClass {
            backend: backend.into(),
            memory_gb,
        }
    }

    /// `entry` with a speed point at its default size.
    fn with_speed(
        mut entry: GenEntry,
        device_class: &str,
        load_seconds: f64,
        seconds_per_step: f64,
        decode_seconds: f64,
    ) -> GenEntry {
        entry.speed.push(SpeedPoint {
            device_class: device_class.into(),
            width: entry.defaults.width,
            height: entry.defaults.height,
            frames: entry.defaults.frames.unwrap_or(1),
            load_seconds,
            seconds_per_step,
            decode_seconds,
            measured_on: "2026-11-18, a test".into(),
        });
        entry
    }

    fn run(size: GenSize, steps: u32, pictures: u32, elapsed_ms: u64) -> MeasuredRun {
        MeasuredRun {
            size,
            steps,
            pictures,
            elapsed_ms,
        }
    }

    fn eta(low_ms: u64, high_ms: u64, measured: bool) -> Eta {
        Eta {
            low_ms,
            high_ms,
            measured,
        }
    }

    #[test]
    fn no_measured_point_and_no_run_here_means_no_time() {
        assert_eq!(
            estimate(&klein(), sq(1024), 4, 1, &class("vulkan", 8), &[]),
            None
        );
    }

    #[test]
    fn the_nearest_class_of_the_same_backend_is_used() {
        // load 6 s, 2 s a step, decode 1 s, at 1024²
        let e = with_speed(klein(), "vulkan-8", 6.0, 2.0, 1.0);
        assert_eq!(
            estimate(&e, sq(1024), 4, 1, &class("vulkan", 12), &[]).unwrap(),
            Eta {
                low_ms: 11_250,
                high_ms: 22_500,
                measured: false
            }
        );
        assert_eq!(estimate(&e, sq(1024), 4, 1, &class("metal", 16), &[]), None);
        // A 24 GB point is nearer a 20 GB card (3 + 4 × 1 + 0.5 = 7.5 s); 12 GB stays with 8.
        let e = with_speed(e, "vulkan-24", 3.0, 1.0, 0.5);
        let at = |gb| estimate(&e, sq(1024), 4, 1, &class("vulkan", gb), &[]);
        assert_eq!(at(20), Some(eta(5_625, 11_250, false)));
        assert_eq!(at(12), Some(eta(11_250, 22_500, false)));
        // Each picture after the first skips the load: 6 + 2 × (8 + 1) = 24 s.
        assert_eq!(
            estimate(&e, sq(1024), 4, 2, &class("vulkan", 8), &[]),
            Some(eta(18_000, 36_000, false))
        );
    }

    #[test]
    fn this_computers_own_runs_replace_the_guess() {
        let e = with_speed(klein(), "vulkan-8", 6.0, 2.0, 1.0);
        let runs = [run(sq(1024), 4, 1, 30_000); 3];
        assert_eq!(
            estimate(&e, sq(1024), 4, 1, &class("vulkan", 8), &runs),
            Some(eta(27_000, 34_500, true))
        );
        // The median, not the mean; and runs count without any speed point of this backend.
        let runs = [
            run(sq(1024), 4, 1, 20_000),
            run(sq(1024), 4, 1, 90_000),
            run(sq(1024), 4, 1, 30_000),
        ];
        assert_eq!(
            estimate(&klein(), sq(1024), 4, 1, &class("vulkan", 8), &runs),
            Some(eta(27_000, 34_500, true))
        );
    }

    #[test]
    fn bigger_settings_cost_time_by_tokens_to_the_power_1_3() {
        // One run here at 1024², 4 steps, as long as the point says (6 + 8 + 1 = 15 s).
        let e = with_speed(klein(), "vulkan-8", 6.0, 2.0, 1.0);
        let runs = [run(sq(1024), 4, 1, 15_000)];
        let wide = clip(2048, 1024, 1);
        let got = estimate(&e, wide, 4, 1, &class("vulkan", 8), &runs).unwrap();
        // Twice the tokens: the load stays, the step part × 2^1.3, the decode × 2.
        let ms = (6.0 + 8.0 * 2f64.powf(1.3) + 2.0) * 1000.0;
        assert_eq!(
            got,
            eta((ms * 0.9).round() as u64, (ms * 1.15).round() as u64, true)
        );
        // With no speed point, the whole run scales as steps do.
        let got = estimate(&klein(), wide, 4, 1, &class("vulkan", 8), &runs).unwrap();
        let ms = 15_000.0 * 2f64.powf(1.3);
        assert_eq!(got.high_ms, (ms * 1.15).round() as u64);
        // Twice the steps, from the guess: 6 + 16 + 1 = 23 s.
        assert_eq!(
            estimate(&e, sq(1024), 8, 1, &class("vulkan", 8), &[]),
            Some(eta(17_250, 34_500, false))
        );
    }

    #[test]
    fn the_load_is_never_scaled_down_with_the_size() {
        // A 6 s load and a run of 30 s here at 1024²: 6 s of it is the load, and only the
        // other 24 s scale down to 512².
        let e = with_speed(klein(), "vulkan-8", 6.0, 2.0, 1.0);
        let runs = [run(sq(1024), 4, 1, 30_000)];
        let got = estimate(&e, sq(512), 4, 1, &class("vulkan", 8), &runs).unwrap();
        assert!(got.low_ms >= 6_000, "{got:?}");
        // 6 + 24 × (8 × 0.25^1.3 + 0.25) / 9 = 10.19 s.
        let ms = (6.0 + 24.0 * (8.0 * 0.25f64.powf(1.3) + 0.25) / 9.0) * 1000.0;
        assert_eq!(
            got,
            eta((ms * 0.9).round() as u64, (ms * 1.15).round() as u64, true)
        );
        // From a point far slower than this computer (170 s at 1024²), scaling the whole run
        // would give 5.9 s, under the load alone.
        let e = with_speed(klein(), "vulkan-8", 6.0, 40.0, 4.0);
        let got = estimate(&e, sq(512), 4, 1, &class("vulkan", 8), &runs).unwrap();
        assert!(got.low_ms >= 6_000, "{got:?}");
        // A run shorter than the point's load is all load, at any size.
        let quick = [run(sq(1024), 4, 1, 5_000)];
        assert_eq!(
            estimate(&e, sq(512), 4, 1, &class("vulkan", 8), &quick),
            Some(eta(4_500, 5_750, true))
        );
    }

    #[test]
    fn apple_video_gets_the_pad() {
        // 10 + 4 × 20 + 15 = 105 s at the point's own size.
        let unpadded = 157_500;
        let e = with_speed(wan22(), "metal-16", 10.0, 20.0, 15.0);
        let got = estimate(&e, clip(832, 480, 49), 4, 1, &class("metal", 16), &[]).unwrap();
        assert_eq!(got.high_ms, (1.3 * unpadded as f64).round() as u64);
        assert_eq!(got, eta(102_375, 204_750, false));
        // Measured here too.
        let runs = [run(clip(832, 480, 49), 4, 1, 100_000)];
        let got = estimate(&e, clip(832, 480, 49), 4, 1, &class("metal", 16), &runs).unwrap();
        assert_eq!(got, eta(117_000, 149_500, true));
        // Not on a graphics card, and not for a picture on a Mac.
        let e = with_speed(wan22(), "vulkan-8", 10.0, 20.0, 15.0);
        let got = estimate(&e, clip(832, 480, 49), 4, 1, &class("vulkan", 8), &[]).unwrap();
        assert_eq!(got.high_ms, unpadded);
        let e = with_speed(klein(), "metal-16", 6.0, 2.0, 1.0);
        let got = estimate(&e, sq(1024), 4, 1, &class("metal", 16), &[]).unwrap();
        assert_eq!(got.high_ms, 22_500);
    }

    #[test]
    fn the_device_class_is_the_backend_and_its_memory() {
        assert_eq!(device_class(&gpu(8, 32), "vulkan"), class("vulkan", 8));
        // A run on the processor of a computer with a card is a processor run.
        assert_eq!(device_class(&gpu(8, 32), "cpu"), class("cpu", 0));
        assert_eq!(device_class(&cpu(16), "cpu"), class("cpu", 0));
        // A 16 GB Mac gives Metal about two thirds of its memory.
        let mac = Target::AppleUnified {
            working_set_bytes: 10_700_000_000,
            ram_free_bytes: Some(8 * GB),
        };
        assert_eq!(device_class(&mac, "metal"), class("metal", 16));
        assert_eq!(device_class(&apple(16, 24), "metal"), class("metal", 24));
    }

    #[test]
    fn times_are_rounded_for_people() {
        assert_eq!(round_for_display(12_300), 10_000);
        assert_eq!(round_for_display(430_000), 420_000);
        assert_eq!(round_for_display(820_000), 840_000);
        assert_eq!(round_for_display(58_000), 60_000);
    }

    #[test]
    fn time_left_follows_the_live_speed() {
        assert_eq!(left_ms(3, 4, 1, 2.0, 1_000), 3_000);
        // One more picture after this one: 4 steps and its tail.
        assert_eq!(left_ms(3, 4, 2, 2.0, 1_000), 12_000);
        assert_eq!(left_ms(0, 4, 0, 2.0, 1_000), 0);
    }

    #[test]
    fn slow_is_judged_by_the_middle_of_the_range() {
        let picture = eta(100_000, 140_000, false);
        assert!(!is_slow(&klein(), &picture));
        assert!(is_slow(&klein(), &eta(100_000, 140_002, false)));
        // A clip has three minutes.
        assert!(!is_slow(&wan22(), &eta(150_000, 210_000, true)));
        assert!(is_slow(&wan22(), &eta(150_000, 210_002, true)));
    }

    #[test]
    fn the_word_after_an_unload_comes_before_red_and_slow() {
        let r = gen_fit(&zimage(), sq(1024), &gpu(3, 32), Some(5 * GB)); // partly on the processor now, one part at a time after
        assert_eq!(fit_word(&zimage(), &r, None), FitWord::AfterUnload);
        assert_eq!(running_fit(&r), (GenFit::OnePartAtATime, true));
        let beside = gen_fit(&klein(), sq(1024), &gpu(8, 32), Some(GB)); // green beside a small chat model: no unload
        assert_eq!(
            (fit_word(&klein(), &beside, None), running_fit(&beside)),
            (FitWord::Fits, (GenFit::Green, false))
        );
        let red = gen_fit(&zimage(), sq(1024), &gpu(1, 3), Some(GB)); // red even after the unload
        assert_eq!(
            (fit_word(&zimage(), &red, None), running_fit(&red).1),
            (FitWord::NotHere, false)
        );
        assert_eq!(
            fit_word(&klein(), &gen_fit(&klein(), sq(512), &cpu(32), None), None),
            FitWord::Slow
        ); // a picture on the processor
        // Slow because of the time, and slow with part on the processor after an unload.
        let slow = eta(150_000, 300_000, false);
        assert_eq!(fit_word(&klein(), &beside, Some(&slow)), FitWord::Slow);
        let r = gen_fit(&zimage(), sq(1024), &gpu(1, 3), Some(3 * GB));
        assert_eq!(fit_word(&zimage(), &r, None), FitWord::Slow);
        assert_eq!(running_fit(&r), (GenFit::PartlyOnProcessor, true));
        assert_eq!(
            serde_json::to_value(FitWord::AfterUnload).unwrap(),
            serde_json::json!("afterUnload")
        );
    }

    #[test]
    fn very_slow_starts_after_15_minutes() {
        let twenty = Eta {
            low_ms: 900_000,
            high_ms: 1_500_000,
            measured: true,
        };
        assert!(
            very_slow(FitWord::Fits, Some(&twenty)) && over_15_min(FitWord::Fits, Some(&twenty))
        );
        assert!(very_slow(FitWord::NotHere, None) && !over_15_min(FitWord::NotHere, Some(&twenty)));
        assert!(!very_slow(FitWord::Slow, None));
        // The middle of 10 to 20 minutes is exactly 15: not above it.
        let fifteen = eta(600_000, 1_200_000, false);
        assert!(!very_slow(FitWord::Slow, Some(&fifteen)));
        assert_eq!(
            serde_json::to_value(twenty).unwrap(),
            serde_json::json!({"lowMs": 900_000, "highMs": 1_500_000, "measured": true})
        );
    }
}
