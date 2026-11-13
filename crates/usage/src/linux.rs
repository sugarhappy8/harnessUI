//! Linux (a development platform): the processor from `/proc/stat`, AMD graphics cards from
//! the amdgpu driver's sysfs files. NVIDIA and Intel cards are `NotReadHere`: reading them
//! would mean loading a vendor library into the process that holds the approval state and
//! the keychain secrets (item 6, section 5). That holds also when the model runs on such a
//! card and an AMD card is there as well: the line never shows a card the model does not
//! use in its place.
//!
//! The parsers below are compiled on every system so their tests run everywhere.

use crate::{CpuTimes, Focus, GraphicsSample, Reading, cap_percent};
use std::path::Path;

/// The first `cpu` line of `/proc/stat` (all processors together) as busy and total time.
/// Busy is user, nice, system, irq, softirq and steal; idle is idle and iowait (guest time
/// is already counted in user and nice). Kernels before 2.6 give only the first four
/// numbers; missing ones count as 0. `None` when the line is missing or a number does not
/// parse, or the sum does not fit. A trailing CR is ignored.
pub fn parse_proc_stat(text: &str) -> Option<CpuTimes> {
    let line = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .find(|l| l.split_whitespace().next() == Some("cpu"))?;
    let mut numbers = Vec::with_capacity(10);
    for field in line.split_whitespace().skip(1).take(8) {
        numbers.push(field.parse::<u64>().ok()?);
    }
    if numbers.len() < 4 {
        return None;
    }
    numbers.resize(8, 0);
    let (user, nice, system, idle, iowait, irq, softirq, steal) = (
        numbers[0], numbers[1], numbers[2], numbers[3], numbers[4], numbers[5], numbers[6],
        numbers[7],
    );
    let busy = [nice, system, irq, softirq, steal]
        .into_iter()
        .try_fold(user, u64::checked_add)?;
    let total = busy.checked_add(idle)?.checked_add(iowait)?;
    Some(CpuTimes { busy, total })
}

/// What the sysfs tree says about the graphics cards.
#[derive(Clone, Debug, PartialEq)]
pub enum LinuxGraphics {
    /// No `card<N>` entry at all.
    NoCard,
    /// Cards exist, but none is driven by amdgpu.
    NotReadable,
    /// The folder could not be listed.
    Failed,
    /// The amdgpu card with the most graphics memory.
    Amd(AmdCard),
}

/// One amdgpu card's figures.
#[derive(Clone, Debug, PartialEq)]
pub struct AmdCard {
    pub busy: Reading<f32>,
    pub used: Reading<u64>,
    pub total_bytes: Option<u64>,
    /// `device/product_name` when the board has one (raw; escaped when shown).
    pub product_name: Option<String>,
}

/// AMD's PCI vendor id, as `device/vendor` writes it.
const AMD_VENDOR: &str = "0x1002";

/// Reads the cards under `drm` (`/sys/class/drm` on a real system, a fixture folder in
/// tests). Only entries named `card<digits>` are cards (`card0-DP-1` is a connector,
/// `renderD128` a render node). Of the AMD cards with amdgpu's memory files, the one with
/// the most graphics memory is read (a discrete card beats an integrated one).
pub fn amdgpu_from_files(drm: &Path) -> LinuxGraphics {
    let entries = match std::fs::read_dir(drm) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LinuxGraphics::NoCard,
        Err(_) => return LinuxGraphics::Failed,
    };
    let mut cards: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| is_card_name(name))
        .collect();
    if cards.is_empty() {
        return LinuxGraphics::NoCard;
    }
    cards.sort();
    let mut best: Option<(u64, std::path::PathBuf)> = None;
    for card in &cards {
        let device = drm.join(card).join("device");
        if read_trimmed(&device.join("vendor")).as_deref() != Some(AMD_VENDOR) {
            continue;
        }
        // amdgpu writes the memory files; the older radeon driver does not.
        let Some(total) =
            read_trimmed(&device.join("mem_info_vram_total")).and_then(|t| t.parse::<u64>().ok())
        else {
            continue;
        };
        if best.as_ref().is_none_or(|(most, _)| total > *most) {
            best = Some((total, device));
        }
    }
    let Some((total, device)) = best else {
        return LinuxGraphics::NotReadable;
    };
    let busy = match read_trimmed(&device.join("gpu_busy_percent")) {
        None => Reading::gap(missing_or_failed(&device.join("gpu_busy_percent"))),
        Some(text) => text
            .parse::<f64>()
            .ok()
            .and_then(cap_percent)
            .map_or(Reading::gap(crate::Gap::ReadFailed), Reading::value),
    };
    let used = match read_trimmed(&device.join("mem_info_vram_used")) {
        None => Reading::gap(missing_or_failed(&device.join("mem_info_vram_used"))),
        Some(text) => text
            .parse::<u64>()
            .map_or(Reading::gap(crate::Gap::ReadFailed), crate::bytes_reading),
    };
    let product_name = read_trimmed(&device.join("product_name")).filter(|n| !n.is_empty());
    LinuxGraphics::Amd(AmdCard {
        busy,
        used,
        total_bytes: (total <= crate::MAX_BYTES).then_some(total),
        product_name,
    })
}

/// `card` followed by one or more digits and nothing else.
fn is_card_name(name: &str) -> bool {
    name.strip_prefix("card")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// A small sysfs file's text without surrounding white space (a trailing CR included), or
/// `None` when it cannot be read. Files larger than 4 KiB are not sysfs values.
fn read_trimmed(path: &Path) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut text = String::new();
    file.take(4096).read_to_string(&mut text).ok()?;
    Some(text.trim().to_string())
}

/// A file that does not exist is something the system does not give; one that exists but
/// cannot be read is a failed read.
fn missing_or_failed(path: &Path) -> crate::Gap {
    if path.exists() {
        crate::Gap::ReadFailed
    } else {
        crate::Gap::SystemGivesNone
    }
}

/// Whether the probe's description names a graphics card of a maker this reader does not
/// read (NVIDIA or Intel).
fn names_an_unread_maker(description: &str) -> bool {
    let d = description.to_ascii_lowercase();
    ["nvidia", "geforce", "quadro", "intel"]
        .iter()
        .any(|maker| d.contains(maker))
}

/// The graphics reading for the usage line from what sysfs says.
///
/// - The focus (the model's card, from the probe) names an NVIDIA or Intel card: that card,
///   under the probe's name and total, with both figures `NotReadHere`, even when an AMD
///   card is there too (section 5: the focus device is the model's device).
/// - An AMD card is read: the name is the board's own when it has one, else the probe's
///   description when that names an AMD card, else "AMD graphics card".
pub fn graphics_reading(found: LinuxGraphics, focus: &Focus) -> Reading<GraphicsSample> {
    use crate::Gap;
    let not_read_here = || {
        Reading::value(GraphicsSample {
            name: crate::escape_text(
                focus.description.as_deref().unwrap_or("Graphics card"),
                crate::NAME_CAP,
            ),
            busy_percent: Reading::gap(Gap::NotReadHere),
            memory_used_bytes: Reading::gap(Gap::NotReadHere),
            memory_total_bytes: focus.total_hint_bytes,
        })
    };
    match found {
        LinuxGraphics::NoCard => Reading::gap(Gap::NoGraphicsCard),
        LinuxGraphics::Failed => Reading::gap(Gap::ReadFailed),
        LinuxGraphics::NotReadable => not_read_here(),
        LinuxGraphics::Amd(_)
            if focus
                .description
                .as_deref()
                .is_some_and(names_an_unread_maker) =>
        {
            not_read_here()
        }
        LinuxGraphics::Amd(card) => {
            let probe_name = focus.description.as_deref().filter(|d| {
                let d = d.to_ascii_lowercase();
                d.contains("amd") || d.contains("radeon")
            });
            let name = card
                .product_name
                .as_deref()
                .or(probe_name)
                .unwrap_or("AMD graphics card");
            Reading::value(GraphicsSample {
                name: crate::escape_text(name, crate::NAME_CAP),
                busy_percent: card.busy,
                memory_used_bytes: card.used,
                memory_total_bytes: card.total_bytes,
            })
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use sys::{LinuxSampler, logical_processors};

#[cfg(target_os = "linux")]
mod sys {
    use super::{amdgpu_from_files, graphics_reading, parse_proc_stat};
    use crate::{Focus, Gap, LogOnce, Rate, Reading, Sample, Sampler, rate_reading};
    use std::path::PathBuf;
    use std::time::Instant;

    /// The logical processors the system has online.
    pub(crate) fn logical_processors() -> Option<u32> {
        // SAFETY: sysconf has no preconditions; it only returns a number.
        let n = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
        u32::try_from(n).ok()
    }

    pub(crate) struct LinuxSampler {
        proc_stat: PathBuf,
        drm: PathBuf,
        rate: Rate<crate::CpuTimes>,
        threads: u32,
        log: LogOnce,
    }

    impl LinuxSampler {
        pub(crate) fn new() -> Self {
            LinuxSampler {
                proc_stat: PathBuf::from("/proc/stat"),
                drm: PathBuf::from("/sys/class/drm"),
                rate: Rate::new(),
                threads: crate::logical_processors(),
                log: LogOnce::default(),
            }
        }

        fn processor(&mut self, at: Instant) -> Reading<f32> {
            let text = match std::fs::read_to_string(&self.proc_stat) {
                Ok(text) => text,
                Err(e) => {
                    self.rate.reset();
                    if e.kind() == std::io::ErrorKind::NotFound {
                        return Reading::gap(Gap::SystemGivesNone);
                    }
                    self.log.failed("processor", "/proc/stat could not be read");
                    return Reading::gap(Gap::ReadFailed);
                }
            };
            match parse_proc_stat(&text) {
                Some(times) => rate_reading(&mut self.rate, at, times),
                None => {
                    self.rate.reset();
                    self.log
                        .failed("processor", "/proc/stat has no readable cpu line");
                    Reading::gap(Gap::ReadFailed)
                }
            }
        }
    }

    impl Sampler for LinuxSampler {
        fn sample(&mut self, focus: &Focus) -> Sample {
            let processor_percent = self.processor(Instant::now());
            let found = amdgpu_from_files(&self.drm);
            if found == super::LinuxGraphics::Failed {
                self.log
                    .failed("graphics", "/sys/class/drm could not be listed");
            }
            Sample {
                graphics: graphics_reading(found, focus),
                processor_percent,
                threads: self.threads,
                unified: false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Gap, processor_percent};
    use std::path::PathBuf;

    fn fixture(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn sysfs(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/sysfs")
            .join(name)
    }

    #[test]
    fn proc_stat_pair_gives_a_percentage() {
        let a = parse_proc_stat(&fixture("proc_stat_a.txt")).unwrap();
        let b = parse_proc_stat(&fixture("proc_stat_b.txt")).unwrap();
        // a: user 10000 nice 500 system 2000 idle 80000 iowait 1000 irq 100 softirq 300 steal 100
        assert_eq!(
            a,
            CpuTimes {
                busy: 13_000,
                total: 94_000
            }
        );
        // b: busy +1500, idle +3500, iowait +0 → 1500 of 5000 = 30 %
        assert_eq!(
            b,
            CpuTimes {
                busy: 14_500,
                total: 99_000
            }
        );
        assert_eq!(processor_percent(a, b), Some(30.0));
    }

    #[test]
    fn proc_stat_with_crlf_endings() {
        let text = fixture("proc_stat_crlf.txt");
        assert!(
            text.contains("\r\n"),
            "the fixture keeps its CRLF endings (.gitattributes)"
        );
        assert_eq!(
            parse_proc_stat(&text),
            Some(CpuTimes {
                busy: 13_000,
                total: 94_000
            })
        );
    }

    #[test]
    fn proc_stat_counters_that_wrap_start_again() {
        let a = parse_proc_stat(&fixture("proc_stat_wrap_a.txt")).unwrap();
        let b = parse_proc_stat(&fixture("proc_stat_wrap_b.txt")).unwrap();
        assert!(b.total < a.total);
        assert_eq!(
            processor_percent(a, b),
            None,
            "measure again, never a wrapped difference"
        );
    }

    #[test]
    fn proc_stat_old_and_broken_lines() {
        // Four numbers (kernels before 2.6).
        assert_eq!(
            parse_proc_stat("cpu  10 0 10 80\ncpu0 10 0 10 80\n"),
            Some(CpuTimes {
                busy: 20,
                total: 100
            })
        );
        // The per-processor lines are never taken for the total.
        assert_eq!(parse_proc_stat("cpu0 10 0 10 80\n"), None);
        assert_eq!(parse_proc_stat("cpu  10 0 x 80\n"), None);
        assert_eq!(parse_proc_stat("cpu  10 0 10\n"), None);
        assert_eq!(parse_proc_stat("cpu  -10 0 10 80\n"), None);
        assert_eq!(parse_proc_stat(""), None);
        // A sum that does not fit is refused, never wrapped.
        let huge = format!("cpu  {} {} 0 0\n", u64::MAX, u64::MAX);
        assert_eq!(parse_proc_stat(&huge), None);
        let huge = format!("cpu  {} 0 0 {}\n", u64::MAX, 1);
        assert_eq!(parse_proc_stat(&huge), None);
    }

    #[test]
    fn amd_card_beats_the_integrated_one() {
        let found = amdgpu_from_files(&sysfs("amd_and_igpu"));
        assert_eq!(
            found,
            LinuxGraphics::Amd(AmdCard {
                busy: Reading::value(63.0),
                used: Reading::value(4_294_967_296),
                total_bytes: Some(17_095_983_104),
                product_name: None,
            })
        );
        let shown = graphics_reading(found, &Focus::default());
        let Reading::Value { value } = shown else {
            panic!("{shown:?}")
        };
        assert_eq!(value.name, "AMD graphics card");
        assert_eq!(value.memory_total_bytes, Some(17_095_983_104));
    }

    #[test]
    fn missing_busy_file_is_half_a_reading() {
        let found = amdgpu_from_files(&sysfs("no_busy"));
        let LinuxGraphics::Amd(card) = &found else {
            panic!("{found:?}")
        };
        assert_eq!(card.busy, Reading::gap(Gap::SystemGivesNone));
        assert_eq!(card.used, Reading::value(1_073_741_824));
        assert_eq!(card.product_name.as_deref(), Some("AMD Radeon RX 7900 XTX"));
        // The CRLF ending of the busy-less card's files is ignored.
        assert_eq!(card.total_bytes, Some(25_753_026_560));
    }

    #[test]
    fn unparsable_files_are_read_failures_and_percent_is_capped() {
        let found = amdgpu_from_files(&sysfs("garbage"));
        let LinuxGraphics::Amd(card) = &found else {
            panic!("{found:?}")
        };
        assert_eq!(card.busy, Reading::value(100.0), "250 % is shown as 100 %");
        assert_eq!(card.used, Reading::gap(Gap::ReadFailed));
    }

    #[test]
    fn other_vendors_are_not_read_here() {
        let found = amdgpu_from_files(&sysfs("nvidia_only"));
        assert_eq!(found, LinuxGraphics::NotReadable);
        let focus = Focus {
            description: Some("NVIDIA GeForce RTX 4090".into()),
            total_hint_bytes: Some(25_757_220_864),
        };
        let shown = graphics_reading(found, &focus);
        let Reading::Value { value } = shown else {
            panic!("{shown:?}")
        };
        assert_eq!(value.name, "NVIDIA GeForce RTX 4090");
        assert_eq!(value.busy_percent, Reading::gap(Gap::NotReadHere));
        assert_eq!(value.memory_used_bytes, Reading::gap(Gap::NotReadHere));
        assert_eq!(value.memory_total_bytes, Some(25_757_220_864));
    }

    #[test]
    fn an_amd_card_the_model_does_not_use_is_not_shown_as_the_focus() {
        // The model runs on an NVIDIA card (the probe's focus); an AMD integrated card is
        // also there. The usage line describes the model's card, which is not read here,
        // never the AMD card's figures under any name.
        let found = amdgpu_from_files(&sysfs("nvidia_and_amd_apu"));
        assert!(
            matches!(found, LinuxGraphics::Amd(_)),
            "the AMD card is found: {found:?}"
        );
        let not_read = |description: &str| {
            let focus = Focus {
                description: Some(description.into()),
                total_hint_bytes: Some(25_757_220_864),
            };
            let shown = graphics_reading(found.clone(), &focus);
            let Reading::Value { value } = shown else {
                panic!("{shown:?}")
            };
            assert_eq!(value.name, description);
            assert_eq!(value.busy_percent, Reading::gap(Gap::NotReadHere));
            assert_eq!(value.memory_used_bytes, Reading::gap(Gap::NotReadHere));
            assert_eq!(value.memory_total_bytes, Some(25_757_220_864));
        };
        not_read("NVIDIA GeForce RTX 4090");
        not_read("Intel(R) Arc(tm) A770 Graphics (DG2)");
        // An AMD focus, or none, reads the AMD card ("Instinct" is not "Intel").
        let amd = |d: &str| Focus {
            description: Some(d.into()),
            total_hint_bytes: None,
        };
        for focus in [
            Focus::default(),
            amd("AMD Radeon(TM) Graphics (RADV RAPHAEL_MENDOCINO)"),
            amd("AMD Instinct MI300X"),
        ] {
            let shown = graphics_reading(found.clone(), &focus);
            let Reading::Value { value } = shown else {
                panic!("{shown:?}")
            };
            assert_eq!(value.busy_percent, Reading::value(40.0));
            assert_eq!(value.memory_used_bytes, Reading::value(268_435_456));
            assert_eq!(value.memory_total_bytes, Some(536_870_912));
        }
    }

    #[test]
    fn no_cards_means_no_graphics_card() {
        assert_eq!(amdgpu_from_files(&sysfs("no_cards")), LinuxGraphics::NoCard);
        assert_eq!(
            amdgpu_from_files(&sysfs("this folder does not exist")),
            LinuxGraphics::NoCard
        );
        assert_eq!(
            graphics_reading(LinuxGraphics::NoCard, &Focus::default()),
            Reading::gap(Gap::NoGraphicsCard)
        );
        assert_eq!(
            graphics_reading(LinuxGraphics::Failed, &Focus::default()),
            Reading::gap(Gap::ReadFailed)
        );
    }

    #[test]
    fn card_names() {
        assert!(is_card_name("card0"));
        assert!(is_card_name("card12"));
        assert!(!is_card_name("card"));
        assert!(!is_card_name("card0-DP-1"));
        assert!(!is_card_name("renderD128"));
        assert!(!is_card_name("version"));
    }

    #[test]
    fn names_come_from_the_board_then_an_amd_probe_name() {
        let card = AmdCard {
            busy: Reading::Measuring,
            used: Reading::Measuring,
            total_bytes: None,
            product_name: None,
        };
        let name = |focus: &Focus, product: Option<&str>| {
            let mut card = card.clone();
            card.product_name = product.map(str::to_string);
            match graphics_reading(LinuxGraphics::Amd(card), focus) {
                Reading::Value { value } => value.name,
                other => panic!("{other:?}"),
            }
        };
        let amd = Focus {
            description: Some("AMD Radeon RX 7900 XTX (RADV NAVI31)".into()),
            total_hint_bytes: None,
        };
        // A focus that names no maker (Mesa's software renderer); an NVIDIA or Intel focus
        // is not read at all (an_amd_card_the_model_does_not_use_is_not_shown_as_the_focus).
        let other = Focus {
            description: Some("llvmpipe (LLVM 17.0.6, 256 bits)".into()),
            total_hint_bytes: None,
        };
        assert_eq!(name(&amd, None), "AMD Radeon RX 7900 XTX (RADV NAVI31)");
        assert_eq!(
            name(&other, None),
            "AMD graphics card",
            "never another device's name"
        );
        assert_eq!(name(&other, Some("Radeon Pro W7800")), "Radeon Pro W7800");
        assert_eq!(
            name(&Focus::default(), Some("Evil\u{202E}Card\n")),
            "Evil[U+202E]Card"
        );
        let long = "R".repeat(500);
        assert_eq!(
            name(&Focus::default(), Some(&long)).chars().count(),
            crate::NAME_CAP
        );
    }
}
