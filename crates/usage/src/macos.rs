//! macOS: the graphics accelerator's `PerformanceStatistics` dictionary in the IORegistry
//! (only that one property is read, never all of an entry's properties, which costs much
//! more processor time when done often), and the kernel's processor tick counters
//! (`host_statistics(HOST_CPU_LOAD_INFO)`). On Apple silicon the graphics memory is the
//! computer's memory (`unified`), and its total is the probe's Metal working set.
//!
//! The `PerformanceStatistics` keys are not documented by Apple (unverified on the owner's
//! macOS; owner checklist). The parser below is compiled on every system so its tests run
//! everywhere.

use crate::{Gap, Reading, cap_percent};
use std::collections::BTreeMap;

/// The keys read from `PerformanceStatistics`, in the order they are tried.
pub const BUSY_KEYS: [&str; 2] = ["Device Utilization %", "GPU Activity(%)"];
/// Graphics memory in use (Apple silicon: the computer's memory the graphics part uses).
pub const MEMORY_KEY: &str = "In use system memory";

/// A number from the dictionary: CoreFoundation numbers are integers or floating point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StatNumber {
    Int(i64),
    Float(f64),
}

/// The graphics figures from a `PerformanceStatistics` map (only the keys above are ever
/// put into it).
#[derive(Clone, Debug, PartialEq)]
pub struct AppleStats {
    pub busy: Reading<f32>,
    pub used: Reading<u64>,
}

/// Busy from the first busy key present, memory from [`MEMORY_KEY`]. A missing key is
/// something the system does not give; a negative or fractional byte count, or one beyond
/// [`crate::MAX_BYTES`], is a failed read.
pub fn apple_from_stats(stats: &BTreeMap<String, StatNumber>) -> AppleStats {
    let busy = match BUSY_KEYS.iter().find_map(|k| stats.get(*k)) {
        None => Reading::gap(Gap::SystemGivesNone),
        Some(n) => {
            // i64 to f64 only for display (section 2.0).
            let percent = match *n {
                StatNumber::Int(v) => v as f64,
                StatNumber::Float(v) => v,
            };
            cap_percent(percent).map_or(Reading::gap(Gap::ReadFailed), Reading::value)
        }
    };
    let used = match stats.get(MEMORY_KEY) {
        None => Reading::gap(Gap::SystemGivesNone),
        Some(StatNumber::Int(v)) => {
            u64::try_from(*v).map_or(Reading::gap(Gap::ReadFailed), crate::bytes_reading)
        }
        Some(StatNumber::Float(_)) => Reading::gap(Gap::ReadFailed),
    };
    AppleStats { busy, used }
}

/// Processor busy and total ticks from `host_cpu_load_info`'s `cpu_ticks` (user, system,
/// idle, nice). The kernel's counters are 32-bit and wrap after weeks; a wrap shows as a
/// counter going backwards, which [`crate::processor_percent`] answers with "measure again".
pub fn cpu_times_from_ticks(user: u32, system: u32, idle: u32, nice: u32) -> crate::CpuTimes {
    let busy = u64::from(user)
        .saturating_add(u64::from(system))
        .saturating_add(u64::from(nice));
    crate::CpuTimes {
        busy,
        total: busy.saturating_add(u64::from(idle)),
    }
}

#[cfg(target_os = "macos")]
pub(crate) use sys::{MacSampler, logical_processors};

#[cfg(target_os = "macos")]
mod sys {
    use super::{BUSY_KEYS, MEMORY_KEY, StatNumber, apple_from_stats, cpu_times_from_ticks};
    use crate::{Focus, Gap, GraphicsSample, LogOnce, Rate, Reading, Sample, Sampler};
    use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType};
    use std::collections::BTreeMap;
    use std::ffi::{c_char, c_void};
    use std::ptr::NonNull;
    use std::time::Instant;

    #[allow(non_camel_case_types)]
    type io_object_t = libc::mach_port_t;
    #[allow(non_camel_case_types)]
    type io_iterator_t = io_object_t;

    /// `kIOMainPortDefault`: the default main port (`MACH_PORT_NULL`).
    const MAIN_PORT_DEFAULT: libc::mach_port_t = 0;
    const KERN_SUCCESS: libc::kern_return_t = 0;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        /// Returns a matching dictionary with a +1 reference (or null).
        fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        /// Consumes one reference of `matching`, whatever it returns.
        fn IOServiceGetMatchingServices(
            main_port: libc::mach_port_t,
            matching: *mut c_void,
            existing: *mut io_iterator_t,
        ) -> libc::kern_return_t;
        /// The next entry (released by the caller with `IOObjectRelease`), or 0 at the end.
        fn IOIteratorNext(iterator: io_iterator_t) -> io_object_t;
        /// One property of an entry, with a +1 reference (the Create rule), or null.
        fn IORegistryEntryCreateCFProperty(
            entry: io_object_t,
            key: *const CFString,
            allocator: *const c_void,
            options: u32,
        ) -> *mut CFType;
        fn IOObjectRelease(object: io_object_t) -> libc::kern_return_t;
    }

    unsafe extern "C" {
        /// libSystem; not in the libc crate.
        fn mach_port_deallocate(
            task: libc::mach_port_t,
            name: libc::mach_port_t,
        ) -> libc::kern_return_t;
    }

    /// An IOKit object (an iterator or a registry entry), never 0, released exactly once.
    struct IoObject(io_object_t);

    impl IoObject {
        fn new(object: io_object_t) -> Option<Self> {
            (object != 0).then_some(IoObject(object))
        }
    }

    impl Drop for IoObject {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a non-zero object this wrapper owns one reference of
            // (from IOServiceGetMatchingServices or IOIteratorNext); it is released once.
            unsafe { IOObjectRelease(self.0) };
        }
    }

    /// The host port, deallocated once (every `mach_host_self` call adds a reference).
    struct HostPort(libc::mach_port_t);

    impl HostPort {
        #[allow(deprecated)] // libc points to the mach2 crate; the call is libSystem's.
        fn new() -> Option<Self> {
            // SAFETY: mach_host_self has no preconditions; it returns a send right that
            // Drop deallocates.
            let port = unsafe { libc::mach_host_self() };
            (port != 0).then_some(HostPort(port))
        }
    }

    impl Drop for HostPort {
        #[allow(deprecated)] // as above, for mach_task_self
        fn drop(&mut self) {
            // SAFETY: `self.0` is the send right `mach_host_self` gave this wrapper;
            // deallocating it once in this task returns that reference.
            unsafe { mach_port_deallocate(libc::mach_task_self(), self.0) };
        }
    }

    /// The logical processors (`hw.logicalcpu`).
    pub(crate) fn logical_processors() -> Option<u32> {
        let mut value: libc::c_int = 0;
        let mut size = std::mem::size_of::<libc::c_int>();
        // SAFETY: the name is NUL-terminated; `value` and `size` describe a writable int,
        // and no new value is set.
        let rc = unsafe {
            libc::sysctlbyname(
                c"hw.logicalcpu".as_ptr(),
                (&mut value as *mut libc::c_int).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 || size != std::mem::size_of::<libc::c_int>() {
            return None;
        }
        u32::try_from(value).ok()
    }

    /// What is known about the graphics accelerator, kept for the reader thread's life.
    enum Accelerator {
        /// Not looked for yet, or the kept entry stopped answering.
        Unknown,
        /// No `IOAccelerator` entry at all.
        Missing,
        /// Entries exist, but none has the figures.
        WithoutFigures,
        /// The entry whose `PerformanceStatistics` has a busy key.
        Entry(IoObject),
    }

    pub(crate) struct MacSampler {
        host: Option<HostPort>,
        accelerator: Accelerator,
        rate: Rate<crate::CpuTimes>,
        threads: u32,
        log: LogOnce,
    }

    impl MacSampler {
        pub(crate) fn new() -> Self {
            MacSampler {
                host: HostPort::new(),
                accelerator: Accelerator::Unknown,
                rate: Rate::new(),
                threads: crate::logical_processors(),
                log: LogOnce::default(),
            }
        }

        fn processor(&mut self, at: Instant) -> Reading<f32> {
            let Some(host) = &self.host else {
                self.log.failed("processor", "mach_host_self failed");
                return Reading::gap(Gap::ReadFailed);
            };
            // SAFETY: host_cpu_load_info is a plain C struct of integers; zero is valid.
            let mut info: libc::host_cpu_load_info = unsafe { std::mem::zeroed() };
            let mut count: libc::mach_msg_type_number_t = libc::HOST_CPU_LOAD_INFO_COUNT;
            // SAFETY: `info` and `count` describe a buffer of HOST_CPU_LOAD_INFO_COUNT
            // integers, as host_statistics requires for HOST_CPU_LOAD_INFO; the host port
            // is a live send right.
            let rc = unsafe {
                libc::host_statistics(
                    host.0,
                    libc::HOST_CPU_LOAD_INFO,
                    (&mut info as *mut libc::host_cpu_load_info).cast(),
                    &mut count,
                )
            };
            if rc != KERN_SUCCESS || count != libc::HOST_CPU_LOAD_INFO_COUNT {
                self.rate.reset();
                self.log.failed("processor", "host_statistics failed");
                return Reading::gap(Gap::ReadFailed);
            }
            let tick = |state: libc::c_int| {
                usize::try_from(state)
                    .ok()
                    .and_then(|i| info.cpu_ticks.get(i))
                    .copied()
                    .unwrap_or(0)
            };
            let times = cpu_times_from_ticks(
                tick(libc::CPU_STATE_USER),
                tick(libc::CPU_STATE_SYSTEM),
                tick(libc::CPU_STATE_IDLE),
                tick(libc::CPU_STATE_NICE),
            );
            crate::rate_reading(&mut self.rate, at, times)
        }

        /// Looks for the first `IOAccelerator` whose `PerformanceStatistics` has a busy key,
        /// and returns its figures with it. `Err` when IOKit's matching fails.
        fn find_accelerator() -> Result<(Accelerator, BTreeMap<String, StatNumber>), ()> {
            // SAFETY: the name is a NUL-terminated C string. The returned dictionary has a
            // +1 reference that IOServiceGetMatchingServices consumes below (so it is not
            // wrapped in CFRetained, which would release it a second time).
            let matching = unsafe { IOServiceMatching(c"IOAccelerator".as_ptr()) };
            if matching.is_null() {
                return Err(());
            }
            let mut iterator: io_iterator_t = 0;
            // SAFETY: `matching` is a live matching dictionary whose reference this call
            // consumes; `iterator` is a writable io_iterator_t.
            let rc =
                unsafe { IOServiceGetMatchingServices(MAIN_PORT_DEFAULT, matching, &mut iterator) };
            if rc != KERN_SUCCESS {
                return Err(());
            }
            let Some(iterator) = IoObject::new(iterator) else {
                return Ok((Accelerator::Missing, BTreeMap::new()));
            };
            let mut found = Accelerator::Missing;
            // SAFETY (each call): `iterator` is a live iterator this function owns.
            while let Some(entry) = IoObject::new(unsafe { IOIteratorNext(iterator.0) }) {
                found = Accelerator::WithoutFigures;
                if let Some(stats) = read_stats(&entry)
                    && BUSY_KEYS.iter().any(|k| stats.contains_key(*k))
                {
                    return Ok((Accelerator::Entry(entry), stats));
                }
            }
            Ok((found, BTreeMap::new()))
        }

        fn graphics(&mut self, focus: &Focus) -> Reading<GraphicsSample> {
            let kept = match &self.accelerator {
                Accelerator::Entry(entry) => read_stats(entry),
                _ => None,
            };
            let stats = match (kept, &self.accelerator) {
                (Some(stats), _) => stats,
                (None, Accelerator::Missing) => return Reading::gap(Gap::NoGraphicsCard),
                (None, Accelerator::WithoutFigures) => BTreeMap::new(),
                (None, Accelerator::Unknown | Accelerator::Entry(_)) => {
                    match Self::find_accelerator() {
                        Err(()) => {
                            self.accelerator = Accelerator::Unknown;
                            self.log.failed("graphics", "IOKit matching failed");
                            return Reading::gap(Gap::ReadFailed);
                        }
                        Ok((found, stats)) => {
                            let missing = matches!(found, Accelerator::Missing);
                            self.accelerator = found;
                            if missing {
                                return Reading::gap(Gap::NoGraphicsCard);
                            }
                            stats
                        }
                    }
                }
            };
            let figures = apple_from_stats(&stats);
            let fallback = if cfg!(target_arch = "aarch64") {
                "Apple graphics"
            } else {
                "Graphics card"
            };
            Reading::value(GraphicsSample {
                name: crate::escape_text(
                    focus.description.as_deref().unwrap_or(fallback),
                    crate::NAME_CAP,
                ),
                busy_percent: figures.busy,
                // Intel Macs: "In use system memory" is not a graphics card's own memory.
                memory_used_bytes: if cfg!(target_arch = "aarch64") {
                    figures.used
                } else {
                    Reading::gap(Gap::NotReadHere)
                },
                memory_total_bytes: focus.total_hint_bytes,
            })
        }
    }

    /// The busy and memory keys of an entry's `PerformanceStatistics`, or `None` when the
    /// entry has no such dictionary.
    fn read_stats(entry: &IoObject) -> Option<BTreeMap<String, StatNumber>> {
        let key = CFString::from_static_str("PerformanceStatistics");
        // SAFETY: `entry` is a live registry entry; `key` is a live CFString; a null
        // allocator means the default one. The result follows the Create rule (+1).
        let raw = unsafe { IORegistryEntryCreateCFProperty(entry.0, &*key, std::ptr::null(), 0) };
        // SAFETY: `raw` is non-null here and carries the +1 reference of the Create rule,
        // which CFRetained releases exactly once.
        let property: CFRetained<CFType> = unsafe { CFRetained::from_raw(NonNull::new(raw)?) };
        let dict: CFRetained<CFDictionary> = property.downcast::<CFDictionary>().ok()?;
        let mut stats = BTreeMap::new();
        for name in BUSY_KEYS.iter().chain(std::iter::once(&MEMORY_KEY)) {
            if let Some(n) = number(&dict, name) {
                stats.insert((*name).to_string(), n);
            }
        }
        Some(stats)
    }

    /// One number from the dictionary, or `None` when the key is missing or not a number.
    fn number(dict: &CFDictionary, name: &'static str) -> Option<StatNumber> {
        let key = CFString::from_static_str(name);
        // SAFETY: `key` is a live CFString, a valid key for a dictionary with CFType keys.
        // The value follows the Get rule: borrowed from `dict`, which outlives the borrow
        // below and is never changed.
        let value = unsafe { dict.value((&*key as *const CFString).cast()) };
        // SAFETY: a non-null value of a CoreFoundation dictionary is a CoreFoundation
        // object; the reference does not outlive `dict`.
        let value: &CFType = unsafe { value.cast::<CFType>().as_ref() }?;
        let n = value.downcast_ref::<CFNumber>()?;
        n.as_i64()
            .map(StatNumber::Int)
            .or_else(|| n.as_f64().map(StatNumber::Float))
    }

    impl Sampler for MacSampler {
        fn sample(&mut self, focus: &Focus) -> Sample {
            let processor_percent = self.processor(Instant::now());
            Sample {
                graphics: self.graphics(focus),
                processor_percent,
                threads: self.threads,
                unified: cfg!(target_arch = "aarch64"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processor_percent;
    use std::path::PathBuf;

    /// A fixture of `key<TAB>value` lines; values with a dot are floating point.
    fn stats(name: &str) -> BTreeMap<String, StatNumber> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let text = std::fs::read_to_string(&path).unwrap();
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let (k, v) = l.trim_end_matches('\r').split_once('\t').unwrap();
                let n = if v.contains('.') {
                    StatNumber::Float(v.parse().unwrap())
                } else {
                    StatNumber::Int(v.parse().unwrap())
                };
                (k.to_string(), n)
            })
            .collect()
    }

    #[test]
    fn apple_silicon_statistics() {
        let figures = apple_from_stats(&stats("apple_stats_m2.txt"));
        assert_eq!(figures.busy, Reading::value(37.0));
        assert_eq!(figures.used, Reading::value(2_147_483_648));
    }

    #[test]
    fn statistics_without_the_keys() {
        let figures = apple_from_stats(&stats("apple_stats_no_keys.txt"));
        assert_eq!(figures.busy, Reading::gap(Gap::SystemGivesNone));
        assert_eq!(figures.used, Reading::gap(Gap::SystemGivesNone));
        assert_eq!(
            apple_from_stats(&BTreeMap::new()),
            AppleStats {
                busy: Reading::gap(Gap::SystemGivesNone),
                used: Reading::gap(Gap::SystemGivesNone),
            }
        );
    }

    #[test]
    fn statistics_with_odd_values() {
        let mut map = BTreeMap::new();
        map.insert("GPU Activity(%)".to_string(), StatNumber::Float(12.5));
        map.insert(MEMORY_KEY.to_string(), StatNumber::Int(-1));
        let figures = apple_from_stats(&map);
        assert_eq!(figures.busy, Reading::value(12.5), "the second busy key");
        assert_eq!(figures.used, Reading::gap(Gap::ReadFailed));

        map.insert("Device Utilization %".to_string(), StatNumber::Int(250));
        map.insert(MEMORY_KEY.to_string(), StatNumber::Float(1.5));
        let figures = apple_from_stats(&map);
        assert_eq!(
            figures.busy,
            Reading::value(100.0),
            "the first key wins, capped"
        );
        assert_eq!(figures.used, Reading::gap(Gap::ReadFailed));

        map.insert(
            "Device Utilization %".to_string(),
            StatNumber::Float(f64::NAN),
        );
        map.insert(MEMORY_KEY.to_string(), StatNumber::Int(i64::MAX));
        let figures = apple_from_stats(&map);
        assert_eq!(figures.busy, Reading::gap(Gap::ReadFailed));
        assert_eq!(figures.used, Reading::gap(Gap::ReadFailed), "beyond 1 PiB");
    }

    #[test]
    fn processor_ticks() {
        let a = cpu_times_from_ticks(1000, 500, 8000, 0);
        let b = cpu_times_from_ticks(1300, 600, 8600, 0);
        assert_eq!(
            a,
            crate::CpuTimes {
                busy: 1500,
                total: 9500
            }
        );
        assert_eq!(processor_percent(a, b), Some(40.0));
        // The 32-bit counters wrapped: measure again.
        let wrapped = cpu_times_from_ticks(5, 1, 7, 0);
        assert_eq!(processor_percent(b, wrapped), None);
        let max = cpu_times_from_ticks(u32::MAX, u32::MAX, u32::MAX, u32::MAX);
        assert_eq!(max.total, 4 * u64::from(u32::MAX));
    }
}
