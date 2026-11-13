//! What this computer's graphics card and processor are doing right now (item 6, live
//! usage; `docs/dev/0.0.6/item6-spec.md` sections 2.3 and 5).
//!
//! Read on this computer only, and only when asked: the app's reader thread
//! (`src-tauri/src/usage_cmd.rs`) owns one [`Sampler`] and asks it for a [`Sample`] while
//! the window is visible. Nothing here polls by itself, starts a process, loads a driver
//! library or sends anything anywhere.
//!
//! - Windows ([`windows`]): the PDH counters Task Manager's figures come from (always added
//!   by their English names, so non-English Windows works) and DXGI adapter enumeration
//!   without creating a device.
//! - macOS ([`macos`]): the graphics accelerator's `PerformanceStatistics` in the IORegistry
//!   and the kernel's processor tick counters.
//! - Linux ([`linux`], a development platform): `/proc/stat` and the amdgpu driver's sysfs
//!   files. Other graphics cards on Linux are not read (no vendor library is loaded).
//!
//! Computer memory is not read here: the app fills it from
//! `models::hardware::system_memory()`, so this crate does not depend on `models`.
//!
//! Every parser is a pure function kept apart from the system calls, so its tests run on
//! every system against the fixtures in `tests/fixtures`. Numbers from the system are
//! combined with checked or saturating arithmetic (section 2.0); names from the system go
//! through [`escape_text`].

use serde::Serialize;
use std::time::{Duration, Instant};

pub mod linux;
pub mod macos;
pub mod windows;

/// Why a figure is not available.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Gap {
    /// No graphics device at all (the usage line then leaves graphics out).
    NoGraphicsCard,
    /// The system has no counter or key for this figure.
    SystemGivesNone,
    /// A graphics card this system's reader does not handle (NVIDIA or Intel on Linux).
    NotReadHere,
    /// A call failed, or the app's reader is stuck.
    ReadFailed,
}

/// One figure: a value, "still measuring" (a rate needs two samples), or the reason it is
/// not available. JSON: `{"kind":"value","value":…}`, `{"kind":"measuring"}`,
/// `{"kind":"notAvailable","why":"readFailed"}`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Reading<T> {
    Value { value: T },
    Measuring,
    NotAvailable { why: Gap },
}

impl<T> Reading<T> {
    /// A value.
    pub fn value(value: T) -> Self {
        Reading::Value { value }
    }

    /// Not available, for this reason.
    pub fn gap(why: Gap) -> Self {
        Reading::NotAvailable { why }
    }

    /// The value, when there is one.
    pub fn get(&self) -> Option<&T> {
        match self {
            Reading::Value { value } => Some(value),
            _ => None,
        }
    }

    /// The same reading with the value changed by `f`.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Reading<U> {
        match self {
            Reading::Value { value } => Reading::Value { value: f(value) },
            Reading::Measuring => Reading::Measuring,
            Reading::NotAvailable { why } => Reading::NotAvailable { why },
        }
    }
}

/// The graphics device the app cares about: the loaded model's device, else the one a
/// load would use. Both facts come from the app's device probe.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Focus {
    /// The probe's description ("AMD Radeon RX 9070 XT"). Windows matches the adapter by it.
    pub description: Option<String>,
    /// The probe's total memory for the device. On a Mac this is the most Apple graphics may
    /// use (the Metal working set) and is the graphics memory total.
    pub total_hint_bytes: Option<u64>,
}

/// The graphics card's figures. Busy and memory are separate readings, so half a reading is
/// shown as half.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphicsSample {
    /// The card's name, escaped and capped at [`NAME_CAP`] characters (section 2.0).
    pub name: String,
    pub busy_percent: Reading<f32>,
    pub memory_used_bytes: Reading<u64>,
    pub memory_total_bytes: Option<u64>,
}

/// One reading of this computer.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    /// `NotAvailable { NoGraphicsCard }` when no graphics device is known.
    pub graphics: Reading<GraphicsSample>,
    /// `Measuring` on the first sample and after a gap of more than [`RESTART_AFTER`].
    pub processor_percent: Reading<f32>,
    /// Logical processors ("threads").
    pub threads: u32,
    /// Apple silicon: graphics memory is computer memory.
    pub unified: bool,
}

/// Reads this computer. Lives on one thread at a time; rates (busy percentages) are the
/// difference to the previous call.
pub trait Sampler {
    fn sample(&mut self, focus: &Focus) -> Sample;
}

/// The reader for this system. Its state (counter handles, the previous counters) lives
/// until it is dropped; nothing runs between calls.
pub fn system_sampler() -> Box<dyn Sampler + Send> {
    #[cfg(windows)]
    {
        Box::new(windows::WindowsSampler::new())
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacSampler::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxSampler::new())
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        Box::new(Unsupported {
            threads: logical_processors(),
        })
    }
}

/// Any other system: only the thread count is known.
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
struct Unsupported {
    threads: u32,
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
impl Sampler for Unsupported {
    fn sample(&mut self, _focus: &Focus) -> Sample {
        Sample {
            graphics: Reading::gap(Gap::NotReadHere),
            processor_percent: Reading::gap(Gap::NotReadHere),
            threads: self.threads,
            unified: false,
        }
    }
}

/// The logical processors of this computer (what the usage menu and Fine control call
/// "threads"), at least 1.
pub fn logical_processors() -> u32 {
    #[cfg(windows)]
    let counted = windows::logical_processors();
    #[cfg(target_os = "macos")]
    let counted = macos::logical_processors();
    #[cfg(target_os = "linux")]
    let counted = linux::logical_processors();
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    let counted: Option<u32> = None;
    counted.filter(|n| *n > 0).unwrap_or_else(|| {
        std::thread::available_parallelism()
            .ok()
            .and_then(|n| u32::try_from(n.get()).ok())
            .unwrap_or(1)
    })
}

/// Names from the system (graphics cards) are capped at this many characters (section 2.0).
pub const NAME_CAP: usize = 80;

/// Byte amounts above this (1 PiB) are not believed: the figure becomes `ReadFailed`.
pub const MAX_BYTES: u64 = 1 << 50;

/// A rate is measured against the previous sample only when it is at most this old; after a
/// longer gap the next sample says `Measuring` and starts again.
pub const RESTART_AFTER: Duration = Duration::from_secs(10);

/// Text from the system as the app shows it (section 2.0, the rule of
/// `models::hf::uploader_text`): trimmed; every control character (Unicode Cc),
/// bidirectional mark, override or isolate, and zero-width or invisible character written
/// as `[U+XXXX]`; other runs of white space become one space; then cut to `cap` characters
/// with `…` at the end when it was cut. An escape is never cut in half.
pub fn escape_text(text: &str, cap: usize) -> String {
    let mut pieces: Vec<String> = Vec::new();
    let mut space = false;
    for c in text.trim().chars() {
        if is_hidden(c) {
            if space {
                pieces.push(" ".into());
                space = false;
            }
            pieces.push(format!("[U+{:04X}]", u32::from(c)));
        } else if c.is_whitespace() {
            space = true;
        } else {
            if space {
                pieces.push(" ".into());
                space = false;
            }
            pieces.push(c.to_string());
        }
    }
    let len = |p: &String| p.chars().count();
    if pieces.iter().map(len).sum::<usize>() <= cap {
        return pieces.concat();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for p in &pieces {
        let n = len(p);
        if used.saturating_add(n) > cap.saturating_sub(1) {
            break;
        }
        out.push_str(p);
        used = used.saturating_add(n);
    }
    out.truncate(out.trim_end().len());
    out.push('…');
    out
}

/// Characters that are never shown as themselves: control characters (Cc), bidirectional
/// marks, overrides and isolates, line and paragraph separators, and zero-width or
/// invisible characters. The list of 2.0 plus `models::hf`'s `is_invisible_or_control`.
fn is_hidden(c: char) -> bool {
    c.is_control()
        || matches!(u32::from(c),
            0x00AD                  // soft hyphen
            | 0x061C                // Arabic letter mark
            | 0x115F | 0x1160       // Hangul fillers
            | 0x17B4 | 0x17B5
            | 0x180B..=0x180F       // Mongolian variation selectors and vowel separator
            | 0x200B..=0x200F       // zero-width space and joiners, LRM, RLM
            | 0x2028 | 0x2029       // line and paragraph separators
            | 0x202A..=0x202E       // bidi embeddings and overrides
            | 0x2060..=0x206F       // word joiner, invisible operators, bidi isolates
            | 0x3164                // Hangul filler
            | 0xFE00..=0xFE0F       // variation selectors
            | 0xFEFF                // zero-width no-break space
            | 0xFFA0
            | 0xFFF9..=0xFFFB       // interlinear annotation
            | 0x1D173..=0x1D17A
            | 0xE0000..=0xE007F     // tag characters
            | 0xE0100..=0xE01EF)
}

/// A percentage as the app shows it: 0 to 100. Values above 100 (Windows' "% Processor
/// Utility" with turbo, a sum of processes on one engine) become 100, as Task Manager
/// shows; a value that is not a number is no reading.
pub fn cap_percent(value: f64) -> Option<f32> {
    if !value.is_finite() {
        return None;
    }
    // f64 to f32 only for display (section 2.0).
    Some(value.clamp(0.0, 100.0) as f32)
}

/// Processor time counters since start: time spent busy, and all time (busy plus idle).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

/// How busy the processor was between two readings of its counters, or `None` when no time
/// passed or a counter went backwards (it wrapped or was reset: measure again from `now`).
pub fn processor_percent(prev: CpuTimes, now: CpuTimes) -> Option<f32> {
    let total = now.total.checked_sub(prev.total)?;
    let busy = now.busy.checked_sub(prev.busy)?;
    if total == 0 {
        return None;
    }
    // u64 to f64 only for display (section 2.0).
    cap_percent(busy as f64 / total as f64 * 100.0)
}

/// The previous reading of a counter, kept for the next rate (section 5: the first sample
/// reports `Measuring`, and a gap of more than [`RESTART_AFTER`] starts again).
#[derive(Debug)]
pub(crate) struct Rate<T> {
    last: Option<(Instant, T)>,
}

impl<T: Copy> Rate<T> {
    pub(crate) const fn new() -> Self {
        Rate { last: None }
    }

    /// Keeps `now` for the next call and returns the previous reading when it is recent
    /// enough to measure a rate against.
    pub(crate) fn step(&mut self, at: Instant, now: T) -> Option<T> {
        let (then, value) = self.last.replace((at, now))?;
        let gap = at.checked_duration_since(then)?;
        (gap <= RESTART_AFTER).then_some(value)
    }

    /// Forgets the previous reading (a failed read): the next rate starts again.
    pub(crate) fn reset(&mut self) {
        self.last = None;
    }
}

/// A percentage reading from two counter readings: `Measuring` until there is a recent
/// previous one, or when no time passed between them.
pub(crate) fn rate_reading(rate: &mut Rate<CpuTimes>, at: Instant, now: CpuTimes) -> Reading<f32> {
    match rate.step(at, now) {
        Some(prev) => processor_percent(prev, now).map_or(Reading::Measuring, Reading::value),
        None => Reading::Measuring,
    }
}

/// A byte amount from the system as a reading: amounts above [`MAX_BYTES`] are not believed.
pub(crate) fn bytes_reading(bytes: u64) -> Reading<u64> {
    if bytes > MAX_BYTES {
        Reading::gap(Gap::ReadFailed)
    } else {
        Reading::value(bytes)
    }
}

/// Logs a reader failure once per sampler (one sampler lives for one reader thread).
#[derive(Debug, Default)]
pub(crate) struct LogOnce {
    done: bool,
}

impl LogOnce {
    pub(crate) fn failed(&mut self, what: &str, detail: &str) {
        if !self.done {
            self.done = true;
            tracing::warn!(reader = what, detail, "computer usage: a reading failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_serialise_as_the_webview_expects() {
        let value: Reading<f32> = Reading::value(12.5);
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            r#"{"kind":"value","value":12.5}"#
        );
        let measuring: Reading<f32> = Reading::Measuring;
        assert_eq!(
            serde_json::to_string(&measuring).unwrap(),
            r#"{"kind":"measuring"}"#
        );
        for (why, word) in [
            (Gap::NoGraphicsCard, "noGraphicsCard"),
            (Gap::SystemGivesNone, "systemGivesNone"),
            (Gap::NotReadHere, "notReadHere"),
            (Gap::ReadFailed, "readFailed"),
        ] {
            let r: Reading<u64> = Reading::gap(why);
            assert_eq!(
                serde_json::to_string(&r).unwrap(),
                format!(r#"{{"kind":"notAvailable","why":"{word}"}}"#)
            );
        }
        let sample = Sample {
            graphics: Reading::value(GraphicsSample {
                name: "AMD Radeon RX 9070 XT".into(),
                busy_percent: Reading::Measuring,
                memory_used_bytes: Reading::value(1024),
                memory_total_bytes: None,
            }),
            processor_percent: Reading::gap(Gap::ReadFailed),
            threads: 16,
            unified: false,
        };
        assert_eq!(
            serde_json::to_value(&sample).unwrap(),
            serde_json::json!({
                "graphics": {"kind": "value", "value": {
                    "name": "AMD Radeon RX 9070 XT",
                    "busyPercent": {"kind": "measuring"},
                    "memoryUsedBytes": {"kind": "value", "value": 1024},
                    "memoryTotalBytes": null
                }},
                "processorPercent": {"kind": "notAvailable", "why": "readFailed"},
                "threads": 16,
                "unified": false
            })
        );
    }

    #[test]
    fn reading_helpers() {
        assert_eq!(Reading::value(3u64).get(), Some(&3));
        assert_eq!(Reading::<u64>::Measuring.get(), None);
        assert_eq!(Reading::value(3u64).map(|v| v * 2), Reading::value(6u64));
        assert_eq!(
            Reading::<u64>::gap(Gap::NotReadHere).map(|v| v * 2),
            Reading::<u64>::gap(Gap::NotReadHere)
        );
    }

    #[test]
    fn escape_text_rules() {
        assert_eq!(
            escape_text("  AMD Radeon RX 9070 XT  ", NAME_CAP),
            "AMD Radeon RX 9070 XT"
        );
        assert_eq!(escape_text("a  \u{3000} b", NAME_CAP), "a b");
        // Control characters are written out, line breaks included (2.0), after trimming.
        assert_eq!(escape_text("a\nb\tc\r\n", NAME_CAP), "a[U+000A]b[U+0009]c");
        assert_eq!(escape_text("\u{7}bell", NAME_CAP), "[U+0007]bell");
        // Bidirectional marks, overrides and isolates.
        for c in [
            '\u{061C}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202E}', '\u{2066}', '\u{2069}',
        ] {
            assert_eq!(
                escape_text(&format!("x{c}y"), NAME_CAP),
                format!("x[U+{:04X}]y", u32::from(c))
            );
        }
        // Zero-width and invisible characters.
        for c in [
            '\u{200B}', '\u{200D}', '\u{2060}', '\u{2064}', '\u{FEFF}', '\u{2028}',
        ] {
            assert_eq!(
                escape_text(&format!("x{c}y"), NAME_CAP),
                format!("x[U+{:04X}]y", u32::from(c))
            );
        }
        assert_eq!(
            escape_text("<b>card</b>", NAME_CAP),
            "<b>card</b>",
            "text, never markup"
        );
        assert_eq!(escape_text("", NAME_CAP), "");
        assert_eq!(escape_text(" \n ", NAME_CAP), "");
    }

    #[test]
    fn hostile_device_names_are_escaped_and_capped() {
        let hostile = format!(
            "Radeon\u{202E}TX 0709\u{202C} {}\u{200B}{}",
            "A".repeat(100),
            "\u{0}".repeat(50)
        );
        let shown = escape_text(&hostile, NAME_CAP);
        assert!(shown.chars().count() <= NAME_CAP);
        assert!(shown.ends_with('…'));
        assert!(shown.starts_with("Radeon[U+202E]TX 0709[U+202C] AAAA"));
        assert!(!shown.chars().any(is_hidden));
        // Counted in characters; an escape is never cut in half.
        let long = "é".repeat(200);
        let cut = escape_text(&long, NAME_CAP);
        assert_eq!(cut.chars().count(), NAME_CAP);
        let cut = escape_text(&format!("{}\u{202E}tail", "a".repeat(75)), NAME_CAP);
        assert_eq!(cut, format!("{}…", "a".repeat(75)));
        assert_eq!(escape_text(&"a".repeat(80), NAME_CAP), "a".repeat(80));
        assert_eq!(escape_text("abc", 0), "…");
    }

    #[test]
    fn cap_percent_rules() {
        assert_eq!(cap_percent(12.5), Some(12.5));
        assert_eq!(cap_percent(-3.0), Some(0.0));
        assert_eq!(cap_percent(130.0), Some(100.0));
        assert_eq!(cap_percent(f64::NAN), None);
        assert_eq!(cap_percent(f64::INFINITY), None);
        assert_eq!(cap_percent(f64::MAX), Some(100.0));
    }

    #[test]
    fn processor_percent_from_counters() {
        let a = CpuTimes {
            busy: 100,
            total: 1000,
        };
        let b = CpuTimes {
            busy: 150,
            total: 1200,
        };
        assert_eq!(processor_percent(a, b), Some(25.0));
        // No time passed, a counter went backwards: measure again.
        assert_eq!(processor_percent(a, a), None);
        assert_eq!(processor_percent(b, a), None);
        assert_eq!(
            processor_percent(
                a,
                CpuTimes {
                    busy: 90,
                    total: 1200
                }
            ),
            None
        );
        // Busy can never be more than all time; if a system says so, show 100.
        assert_eq!(
            processor_percent(
                a,
                CpuTimes {
                    busy: 600,
                    total: 1100
                }
            ),
            Some(100.0)
        );
        // Extremes never panic.
        assert_eq!(
            processor_percent(
                CpuTimes::default(),
                CpuTimes {
                    busy: u64::MAX,
                    total: u64::MAX
                }
            ),
            Some(100.0)
        );
    }

    #[test]
    fn rates_start_again_after_a_gap() {
        let t0 = Instant::now();
        let mut rate: Rate<u32> = Rate::new();
        assert_eq!(
            rate.step(t0, 1),
            None,
            "the first reading has nothing to compare with"
        );
        assert_eq!(rate.step(t0 + Duration::from_millis(1500), 2), Some(1));
        assert_eq!(
            rate.step(t0 + Duration::from_secs(11), 3),
            Some(2),
            "exactly 9.5 s"
        );
        assert_eq!(
            rate.step(t0 + Duration::from_secs(22), 4),
            None,
            "11 s: start again"
        );
        assert_eq!(rate.step(t0 + Duration::from_secs(23), 5), Some(4));
        rate.reset();
        assert_eq!(rate.step(t0 + Duration::from_secs(24), 6), None);
    }

    #[test]
    fn rate_reading_measures_from_the_second_reading() {
        let t0 = Instant::now();
        let mut rate = Rate::new();
        let a = CpuTimes {
            busy: 0,
            total: 100,
        };
        let b = CpuTimes {
            busy: 50,
            total: 200,
        };
        assert_eq!(rate_reading(&mut rate, t0, a), Reading::Measuring);
        assert_eq!(
            rate_reading(&mut rate, t0 + Duration::from_secs(1), b),
            Reading::value(50.0)
        );
        // The same counters again (no time passed): still measuring, not 0 %.
        assert_eq!(
            rate_reading(&mut rate, t0 + Duration::from_secs(2), b),
            Reading::Measuring
        );
    }

    #[test]
    fn byte_amounts_beyond_belief_are_read_failures() {
        assert_eq!(bytes_reading(0), Reading::value(0));
        assert_eq!(bytes_reading(MAX_BYTES), Reading::value(MAX_BYTES));
        assert_eq!(bytes_reading(MAX_BYTES + 1), Reading::gap(Gap::ReadFailed));
        assert_eq!(bytes_reading(u64::MAX), Reading::gap(Gap::ReadFailed));
    }

    #[test]
    fn threads_are_counted() {
        assert!(logical_processors() >= 1);
    }
}
