//! Windows: the PDH counters Task Manager's figures come from, for every vendor (no vendor
//! library is loaded), and DXGI adapter enumeration without creating a device.
//!
//! - Graphics card busy: `\GPU Engine(*)\Utilization Percentage`, summed per engine over
//!   processes, then the busiest engine of the focus adapter (Task Manager's rule).
//! - Graphics memory used: `\GPU Adapter Memory(*)\Dedicated Usage` of the focus adapter;
//!   the total is DXGI's `DedicatedVideoMemory`.
//! - Processor busy: `\Processor Information(_Total)\% Processor Utility` capped at 100
//!   (what Task Manager shows), else `GetSystemTimes`.
//!
//! Counters are always added by their English names (`PdhAddEnglishCounterW`), so
//! non-English Windows works. Process ids in PDH instance names are dropped while parsing;
//! per-process values are summed per engine and never leave this crate. The counter set
//! names and their instance-name format come from Windows itself and community sources,
//! not from Microsoft documentation (unverified until the owner's check).
//!
//! The parsers below are compiled on every system so their tests run everywhere.

use crate::{Focus, Gap, Reading, bytes_reading, cap_percent};
use std::collections::BTreeMap;

/// An adapter's locally unique id as PDH writes it in instance names
/// (`luid_0x<HighPart>_0x<LowPart>`, 8 hex digits each).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Luid {
    pub high: u32,
    pub low: u32,
}

impl Luid {
    /// From DXGI's `LUID { LowPart, HighPart }` (the high part's bits, as PDH prints them).
    pub fn from_parts(high_part: i32, low_part: u32) -> Self {
        Luid {
            high: high_part.cast_unsigned(),
            low: low_part,
        }
    }
}

/// One engine of one adapter, from a `GPU Engine` instance name; the process id is dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineInstance {
    pub luid: Luid,
    pub phys: u32,
    pub eng: u32,
    /// "3D", "Copy", "VideoDecode", "Compute_0" … (at most 64 characters kept).
    pub engtype: String,
}

/// One adapter part from a `GPU Adapter Memory` instance name (`luid_0x…_0x…_phys_0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterInstance {
    pub luid: Luid,
    pub phys: u32,
}

/// Reads `prefix` then decimal digits; returns the number and the rest.
fn number_after<'a>(text: &'a str, prefix: &str) -> Option<(u32, &'a str)> {
    let rest = text.strip_prefix(prefix)?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let n = rest[..end].parse::<u32>().ok()?;
    Some((n, &rest[end..]))
}

/// Reads `luid_0x<1 to 8 hex>_0x<1 to 8 hex>`; returns the id and the rest.
fn luid_after(text: &str) -> Option<(Luid, &str)> {
    let hex = |s: &str| -> Option<(u32, usize)> {
        let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))?;
        let end = s.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(s.len());
        if end == 0 || end > 8 {
            return None;
        }
        Some((u32::from_str_radix(&s[..end], 16).ok()?, end + 2))
    };
    let rest = text.strip_prefix("luid_")?;
    let (high, used) = hex(rest)?;
    let rest = rest[used..].strip_prefix('_')?;
    let (low, used) = hex(rest)?;
    Some((Luid { high, low }, &rest[used..]))
}

/// Parses `pid_1234_luid_0x00000000_0x0000D1E5_phys_0_eng_0_engtype_3D`. The process id
/// is checked and dropped. A trailing CR is ignored. `None` for anything else.
pub fn parse_engine_instance(name: &str) -> Option<EngineInstance> {
    let name = name.trim_end_matches('\r');
    let rest = match number_after(name, "pid_") {
        Some((_pid, rest)) => rest.strip_prefix('_')?,
        None => name,
    };
    let (luid, rest) = luid_after(rest)?;
    let (phys, rest) = number_after(rest, "_phys_")?;
    let (eng, rest) = number_after(rest, "_eng_")?;
    let engtype = rest.strip_prefix("_engtype_")?;
    Some(EngineInstance {
        luid,
        phys,
        eng,
        engtype: engtype.chars().take(64).collect(),
    })
}

/// Parses `luid_0x00000000_0x0000D1E5_phys_0`. A trailing CR is ignored.
pub fn parse_adapter_instance(name: &str) -> Option<AdapterInstance> {
    let name = name.trim_end_matches('\r');
    let (luid, rest) = luid_after(name)?;
    let (phys, rest) = number_after(rest, "_phys_")?;
    rest.is_empty().then_some(AdapterInstance { luid, phys })
}

/// Whether a PDH instance's id is DXGI's `AdapterLuid`.
pub fn luid_matches(instance: Luid, high_part: i32, low_part: u32) -> bool {
    instance == Luid::from_parts(high_part, low_part)
}

/// Task Manager's rule: each engine's utilisation is the sum over processes; an adapter is
/// as busy as its busiest engine. Items whose name does not parse, or whose value is not a
/// finite number of at least 0, are left out. The result is not capped (see
/// [`crate::cap_percent`]).
pub fn busiest_engine_per_adapter<'a>(
    items: impl IntoIterator<Item = (&'a str, f64)>,
) -> BTreeMap<Luid, f64> {
    let mut per_engine: BTreeMap<(Luid, u32, u32), f64> = BTreeMap::new();
    for (name, value) in items {
        if !value.is_finite() || value < 0.0 {
            continue;
        }
        let Some(e) = parse_engine_instance(name) else {
            continue;
        };
        *per_engine.entry((e.luid, e.phys, e.eng)).or_insert(0.0) += value;
    }
    let mut per_adapter: BTreeMap<Luid, f64> = BTreeMap::new();
    for ((luid, _, _), sum) in per_engine {
        let busiest = per_adapter.entry(luid).or_insert(0.0);
        if sum > *busiest {
            *busiest = sum;
        }
    }
    per_adapter
}

/// Dedicated memory in use per adapter (the parts of linked adapters added up). Items whose
/// name does not parse or whose value is negative are left out; sums saturate.
pub fn memory_per_adapter<'a>(
    items: impl IntoIterator<Item = (&'a str, i64)>,
) -> BTreeMap<Luid, u64> {
    let mut out: BTreeMap<Luid, u64> = BTreeMap::new();
    for (name, value) in items {
        let (Some(a), Ok(bytes)) = (parse_adapter_instance(name), u64::try_from(value)) else {
            continue;
        };
        let sum = out.entry(a.luid).or_insert(0);
        *sum = sum.saturating_add(bytes);
    }
    out
}

/// Items exist, but not one name is in the format `parse` knows.
fn no_name_parses<S: AsRef<str>, V, T>(
    items: &[(S, V)],
    parse: impl Fn(&str) -> Option<T>,
) -> bool {
    !items.is_empty() && items.iter().all(|(name, _)| parse(name.as_ref()).is_none())
}

/// How busy one adapter is, from the `GPU Engine` items (instance name, value) by Task
/// Manager's rule ([`busiest_engine_per_adapter`]), capped at 100.
///
/// An adapter with no engine instance is idle (no process uses it) only when PDH lists it
/// among the `GPU Adapter Memory` items, which name every adapter whether or not a process
/// uses it: then 0 %. Otherwise a failure to match never looks like an idle card: engine
/// items whose names are all in a format this crate does not know give `ReadFailed`; an
/// adapter PDH does not list (or a memory counter that gave nothing) gives
/// `SystemGivesNone`.
pub fn adapter_busy<S: AsRef<str>, M: AsRef<str>>(
    engine_items: &[(S, f64)],
    memory_items: &[(M, i64)],
    luid: Luid,
) -> Reading<f32> {
    let per = busiest_engine_per_adapter(engine_items.iter().map(|(n, v)| (n.as_ref(), *v)));
    if let Some(busiest) = per.get(&luid) {
        return cap_percent(*busiest).map_or(Reading::gap(Gap::ReadFailed), Reading::value);
    }
    if no_name_parses(engine_items, parse_engine_instance) {
        return Reading::gap(Gap::ReadFailed);
    }
    let listed = memory_items
        .iter()
        .any(|(name, _)| parse_adapter_instance(name.as_ref()).is_some_and(|a| a.luid == luid));
    if listed {
        Reading::value(0.0)
    } else {
        Reading::gap(Gap::SystemGivesNone)
    }
}

/// One adapter's dedicated memory in use, from the `GPU Adapter Memory` items (the parts of
/// linked adapters added up, [`memory_per_adapter`]). Items whose names are all in a format
/// this crate does not know give `ReadFailed`; an adapter PDH does not list gives
/// `SystemGivesNone`; more than 1 PiB gives `ReadFailed`.
pub fn adapter_memory<M: AsRef<str>>(memory_items: &[(M, i64)], luid: Luid) -> Reading<u64> {
    let per = memory_per_adapter(memory_items.iter().map(|(n, v)| (n.as_ref(), *v)));
    if let Some(bytes) = per.get(&luid) {
        return bytes_reading(*bytes);
    }
    if no_name_parses(memory_items, parse_adapter_instance) {
        Reading::gap(Gap::ReadFailed)
    } else {
        Reading::gap(Gap::SystemGivesNone)
    }
}

/// A display adapter as DXGI lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterInfo {
    /// DXGI's description as written (escaped only when shown).
    pub description: String,
    /// `DXGI_ADAPTER_DESC1.Flags`.
    pub flags: u32,
    pub dedicated_bytes: u64,
    pub luid: Luid,
}

/// `DXGI_ADAPTER_FLAG_SOFTWARE`.
pub const ADAPTER_FLAG_SOFTWARE: u32 = 2;

/// Adapters that are not graphics cards: software adapters, and names that
/// `src-tauri/src/gpu.rs` `is_virtual` treats as virtual (remote sessions, virtual
/// machines, the fallback driver Windows uses when a card has no driver, virtual monitors).
/// The list is copied from gpu.rs; keep the two the same.
pub fn is_software_adapter(flags: u32, description: &str) -> bool {
    if flags & ADAPTER_FLAG_SOFTWARE != 0 {
        return true;
    }
    let n = description.to_ascii_lowercase();
    [
        "microsoft basic display",
        "microsoft basic render",
        "microsoft remote display",
        "microsoft hyper-v",
        "hyper-v video",
        "vmware",
        "virtualbox",
        "parallels",
        "qxl",
        "virtio",
        "citrix",
        "parsec",
        "virtual display",
        "virtual monitor",
        "indirect display",
        "displaylink",
        "spacedesk",
        "meta virtual",
        "oculus virtual",
        "teamviewer",
        "anydesk",
        "splashtop",
    ]
    .iter()
    .any(|needle| n.contains(needle))
}

/// The adapter the usage line reads: the one whose description equals the probe's (the RX
/// 9070 XT, not the Ryzen's integrated "AMD Radeon(TM) Graphics"; case and surrounding
/// spaces ignored), else the one with the most dedicated memory (the first of equals).
/// Software and virtual adapters are never picked. `None`: no graphics card.
pub fn pick_adapter<'a>(adapters: &'a [AdapterInfo], focus: &Focus) -> Option<&'a AdapterInfo> {
    let real = || {
        adapters
            .iter()
            .filter(|a| !is_software_adapter(a.flags, &a.description))
    };
    if let Some(want) = focus
        .description
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
        && let Some(a) = real().find(|a| a.description.trim().eq_ignore_ascii_case(want))
    {
        return Some(a);
    }
    real().fold(None, |best: Option<&AdapterInfo>, a| match best {
        Some(b) if b.dedicated_bytes >= a.dedicated_bytes => Some(b),
        _ => Some(a),
    })
}

/// DXGI's fixed-size description: up to the first NUL, invalid UTF-16 replaced.
pub fn description_from_wide(wide: &[u16]) -> String {
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

/// Processor times from `GetSystemTimes` (100 ns units): kernel time includes idle time.
/// `None` when the numbers do not add up.
pub fn system_times(idle: u64, kernel: u64, user: u64) -> Option<crate::CpuTimes> {
    let total = kernel.checked_add(user)?;
    let busy = total.checked_sub(idle)?;
    Some(crate::CpuTimes { busy, total })
}

/// A `FILETIME`'s two halves as one number.
pub fn filetime(low: u32, high: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

/// What one call of `PdhGetFormattedCounterArrayW` said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    /// `ERROR_SUCCESS`, with the item count.
    Done { items: u32 },
    /// `PDH_MORE_DATA`, with the buffer size PDH asks for, in bytes.
    MoreData { bytes: u32 },
    /// Any other status.
    Failed(u32),
}

/// Why a counter array could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrayError {
    Status(u32),
    /// The instances kept changing between the two calls.
    KeptChanging,
    /// PDH asked for more than [`MAX_ARRAY_BYTES`].
    TooLarge,
}

/// Retries after `PDH_MORE_DATA` on the second call (the instances changed in between).
pub const MORE_DATA_RETRIES: u32 = 3;
/// The largest counter array buffer accepted (many processes on many engines fit easily).
pub const MAX_ARRAY_BYTES: u32 = 32 << 20;

/// PDH's two-call rule: one call for the size (no buffer), then one with a buffer of that
/// size; on `PDH_MORE_DATA` from the second call, start again, at most
/// [`MORE_DATA_RETRIES`] more times. Returns the buffer (8-byte aligned, as the items need)
/// and the item count PDH wrote; items past that count are never read.
pub fn two_calls(
    mut call: impl FnMut(Option<&mut [u64]>) -> Call,
) -> Result<(Vec<u64>, u32), ArrayError> {
    for _ in 0..=MORE_DATA_RETRIES {
        let bytes = match call(None) {
            Call::Done { .. } => return Ok((Vec::new(), 0)),
            Call::MoreData { bytes } => bytes,
            Call::Failed(status) => return Err(ArrayError::Status(status)),
        };
        if bytes > MAX_ARRAY_BYTES {
            return Err(ArrayError::TooLarge);
        }
        let words = usize::try_from(bytes.div_ceil(8)).map_err(|_| ArrayError::TooLarge)?;
        let mut buffer = vec![0u64; words.max(1)];
        match call(Some(&mut buffer)) {
            Call::Done { items } => return Ok((buffer, items)),
            Call::MoreData { .. } => continue,
            Call::Failed(status) => return Err(ArrayError::Status(status)),
        }
    }
    Err(ArrayError::KeptChanging)
}

/// The UTF-16 text at `offset_units` (in 16-bit units) inside `units`, up to its NUL; `None`
/// when it runs past the end. PDH points item names into the buffer it filled; this never
/// reads past that buffer, whatever the pointer says.
pub fn wide_at(units: &[u16], offset_units: usize) -> Option<String> {
    let tail = units.get(offset_units..)?;
    let end = tail.iter().position(|&c| c == 0)?;
    String::from_utf16(&tail[..end]).ok()
}

#[cfg(windows)]
pub(crate) use sys::{WindowsSampler, logical_processors};

#[cfg(windows)]
mod sys {
    use super::{
        AdapterInfo, ArrayError, Call, Luid, adapter_busy, adapter_memory, description_from_wide,
        filetime, pick_adapter, system_times, two_calls, wide_at,
    };
    use crate::{
        CpuTimes, Focus, Gap, GraphicsSample, LogOnce, Rate, Reading, Sample, Sampler, cap_percent,
        rate_reading,
    };
    use ::windows::Win32::Foundation::FILETIME;
    use ::windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ERROR_NOT_FOUND, IDXGIFactory1,
    };
    use ::windows::Win32::System::Performance::{
        PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_NO_COUNTER, PDH_CSTATUS_NO_INSTANCE,
        PDH_CSTATUS_NO_OBJECT, PDH_CSTATUS_VALID_DATA, PDH_FMT, PDH_FMT_COUNTERVALUE,
        PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY,
        PDH_MORE_DATA, PDH_NO_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
        PdhGetFormattedCounterArrayW, PdhGetFormattedCounterValue, PdhOpenQueryW,
    };
    use ::windows::Win32::System::Threading::{
        ALL_PROCESSOR_GROUPS, GetActiveProcessorCount, GetSystemTimes,
    };
    use ::windows::core::{PCWSTR, w};
    use std::time::Instant;

    /// `PDH_FMT_NOCAP100` from pdh.h (not in the windows crate's metadata): values above
    /// 100 are kept, so a sum of processes or turbo utility is capped once, by this crate.
    const PDH_FMT_NOCAP100: PDH_FMT = PDH_FMT(0x0000_8000);
    const ERROR_SUCCESS: u32 = 0;

    /// The logical processors in every processor group.
    pub(crate) fn logical_processors() -> Option<u32> {
        // SAFETY: GetActiveProcessorCount has no preconditions; 0 means it failed.
        let n = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
        (n > 0).then_some(n)
    }

    /// A PDH query, closed exactly once (which also removes its counters).
    struct Query(PDH_HQUERY);

    impl Drop for Query {
        fn drop(&mut self) {
            // SAFETY: `self.0` is the open query PdhOpenQueryW gave this wrapper.
            unsafe { PdhCloseQuery(self.0) };
        }
    }

    /// A counter of the query, or why it is not there.
    #[derive(Clone, Copy)]
    enum Counter {
        Added(PDH_HCOUNTER),
        /// The counter set does not exist on this system.
        Missing,
        /// Adding it failed for another reason.
        Failed,
    }

    impl Counter {
        /// SAFETY: `query` must be an open query.
        unsafe fn add(query: PDH_HQUERY, path: PCWSTR) -> Self {
            let mut counter = PDH_HCOUNTER::default();
            // SAFETY: `query` is open (the caller's promise); `path` is a NUL-terminated
            // constant; `counter` is writable.
            let status = unsafe { PdhAddEnglishCounterW(query, path, 0, &mut counter) };
            match status {
                ERROR_SUCCESS => Counter::Added(counter),
                // The counter set, the counter or the instance does not exist here.
                PDH_CSTATUS_NO_OBJECT | PDH_CSTATUS_NO_COUNTER | PDH_CSTATUS_NO_INSTANCE => {
                    Counter::Missing
                }
                _ => Counter::Failed,
            }
        }
    }

    pub(crate) struct WindowsSampler {
        // The counters belong to `query`; closing it removes them.
        engines: Counter,
        adapter_memory: Counter,
        processor: Counter,
        query: Option<Query>,
        /// The time of the last collection (PDH keeps the counters themselves).
        collected: Rate<()>,
        /// `GetSystemTimes`, read every sample so it can stand in for the processor counter
        /// at once.
        times: Rate<CpuTimes>,
        /// DXGI's adapters, listed once per reader thread.
        adapters: Option<Result<Vec<AdapterInfo>, ()>>,
        threads: u32,
        log: LogOnce,
    }

    // SAFETY: PDH query and counter handles are not tied to the thread that created them;
    // the sampler is used by one thread at a time (`&mut self`) and owns its handles. No COM
    // object is kept (DXGI's factory is dropped right after listing the adapters).
    unsafe impl Send for WindowsSampler {}

    impl WindowsSampler {
        pub(crate) fn new() -> Self {
            let mut query = PDH_HQUERY::default();
            // SAFETY: a null data source means real-time data; `query` is writable.
            let status = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) };
            let query = (status == ERROR_SUCCESS).then_some(Query(query));
            let (engines, adapter_memory, processor) = match &query {
                // SAFETY: the query was just opened and is still open.
                Some(q) => unsafe {
                    (
                        Counter::add(q.0, w!("\\GPU Engine(*)\\Utilization Percentage")),
                        Counter::add(q.0, w!("\\GPU Adapter Memory(*)\\Dedicated Usage")),
                        Counter::add(
                            q.0,
                            w!("\\Processor Information(_Total)\\% Processor Utility"),
                        ),
                    )
                },
                None => (Counter::Failed, Counter::Failed, Counter::Missing),
            };
            WindowsSampler {
                engines,
                adapter_memory,
                processor,
                query,
                collected: Rate::new(),
                times: Rate::new(),
                adapters: None,
                threads: crate::logical_processors(),
                log: LogOnce::default(),
            }
        }

        /// Collects every counter once; `Some(ready)` on success, where `ready` says a rate
        /// can be formatted (a recent previous collection exists).
        fn collect(&mut self, at: Instant) -> Option<bool> {
            let query = self.query.as_ref()?;
            // SAFETY: the query is open.
            let status = unsafe { PdhCollectQueryData(query.0) };
            if status != ERROR_SUCCESS {
                self.collected.reset();
                self.log
                    .failed("pdh", &format!("collecting failed: 0x{status:08X}"));
                return None;
            }
            Some(self.collected.step(at, ()).is_some())
        }

        fn adapters(&mut self) -> Result<&[AdapterInfo], ()> {
            if self.adapters.is_none() {
                let listed = list_adapters();
                if let Err(e) = &listed {
                    self.log
                        .failed("dxgi", &format!("listing adapters failed: {e}"));
                }
                self.adapters = Some(listed.map_err(|_| ()));
            }
            match &self.adapters {
                Some(Ok(list)) => Ok(list),
                _ => Err(()),
            }
        }

        fn graphics(&mut self, focus: &Focus, collected: Option<bool>) -> Reading<GraphicsSample> {
            let picked = match self.adapters() {
                Err(()) => return Reading::gap(Gap::ReadFailed),
                Ok(list) => pick_adapter(list, focus).cloned(),
            };
            let Some(adapter) = picked else {
                return Reading::gap(Gap::NoGraphicsCard);
            };
            // The memory items first: they also say which adapters PDH knows, so an adapter
            // without engine instances reads as idle only when PDH lists it.
            let memory = match (self.adapter_memory, collected) {
                (Counter::Missing, _) => Err(Gap::SystemGivesNone),
                (Counter::Failed, _) | (Counter::Added(_), None) => Err(Gap::ReadFailed),
                (Counter::Added(c), Some(_)) => read_array(c, PDH_FMT_LARGE, |v| {
                    // SAFETY: PDH wrote the 64-bit integer member, the format asked for.
                    unsafe { v.Anonymous.largeValue }
                })
                .map_err(|e| {
                    self.log.failed("pdh", &format!("GPU memory: {e:?}"));
                    Gap::ReadFailed
                }),
            };
            let used = match &memory {
                Ok(items) => {
                    let used = adapter_memory(items, adapter.luid);
                    if let Reading::NotAvailable { why } = &used {
                        self.log
                            .failed("pdh", &format!("GPU memory: the adapter gave {why:?}"));
                    }
                    used
                }
                Err(gap) => Reading::gap(*gap),
            };
            let listed: &[(String, i64)] = memory.as_deref().unwrap_or(&[]);
            let busy = match (self.engines, collected) {
                (Counter::Missing, _) => Reading::gap(Gap::SystemGivesNone),
                (Counter::Failed, _) | (Counter::Added(_), None) => Reading::gap(Gap::ReadFailed),
                (Counter::Added(_), Some(false)) => Reading::Measuring,
                (Counter::Added(c), Some(true)) => {
                    match read_array(c, PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100.0), |v| {
                        // SAFETY: PDH wrote the double member, the format asked for.
                        unsafe { v.Anonymous.doubleValue }
                    }) {
                        Ok(items) => {
                            let busy = adapter_busy(&items, listed, adapter.luid);
                            if let Reading::NotAvailable { why } = &busy {
                                self.log.failed(
                                    "pdh",
                                    &format!("GPU engines: the adapter gave {why:?}"),
                                );
                            }
                            busy
                        }
                        Err(e) => {
                            self.log.failed("pdh", &format!("GPU engines: {e:?}"));
                            Reading::gap(Gap::ReadFailed)
                        }
                    }
                }
            };
            let total = Some(adapter.dedicated_bytes)
                .filter(|b| *b > 0 && *b <= crate::MAX_BYTES)
                .or(focus.total_hint_bytes);
            Reading::value(GraphicsSample {
                name: crate::escape_text(&adapter.description, crate::NAME_CAP),
                busy_percent: busy,
                memory_used_bytes: used,
                memory_total_bytes: total,
            })
        }

        /// Task Manager's figure (`% Processor Utility`) when PDH has it. `GetSystemTimes` is
        /// read every time as well, so it can step in at once when the counter is missing,
        /// a collection fails, or the counter fails (then for the rest of this reader).
        fn processor(&mut self, at: Instant, collected: Option<bool>) -> Reading<f32> {
            let fallback = self.system_times(at);
            let (Counter::Added(c), Some(ready)) = (self.processor, collected) else {
                return fallback;
            };
            if !ready {
                return Reading::Measuring;
            }
            let mut value = PDH_FMT_COUNTERVALUE::default();
            // SAFETY: `c` is a counter of the open query; `value` is writable; the type
            // out-parameter is not wanted.
            let status = unsafe {
                PdhGetFormattedCounterValue(
                    c,
                    PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100.0),
                    None,
                    &mut value,
                )
            };
            if status != ERROR_SUCCESS || !valid(value.CStatus) {
                self.log.failed(
                    "pdh",
                    &format!("processor utility: 0x{status:08X} 0x{:08X}", value.CStatus),
                );
                self.processor = Counter::Failed;
                return fallback;
            }
            // SAFETY: PDH wrote the double member, the format asked for.
            let percent = unsafe { value.Anonymous.doubleValue };
            cap_percent(percent).map_or(fallback, Reading::value)
        }

        /// How busy the processor was since the last call, from `GetSystemTimes`.
        fn system_times(&mut self, at: Instant) -> Reading<f32> {
            let (mut idle, mut kernel, mut user) = (
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
            );
            // SAFETY: the three FILETIMEs are writable.
            let ok = unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) };
            let times = ok.ok().and_then(|()| {
                system_times(
                    filetime(idle.dwLowDateTime, idle.dwHighDateTime),
                    filetime(kernel.dwLowDateTime, kernel.dwHighDateTime),
                    filetime(user.dwLowDateTime, user.dwHighDateTime),
                )
            });
            match times {
                Some(times) => rate_reading(&mut self.times, at, times),
                None => {
                    self.times.reset();
                    self.log.failed("processor", "GetSystemTimes failed");
                    Reading::gap(Gap::ReadFailed)
                }
            }
        }
    }

    impl Sampler for WindowsSampler {
        fn sample(&mut self, focus: &Focus) -> Sample {
            let at = Instant::now();
            let collected = self.collect(at);
            Sample {
                graphics: self.graphics(focus, collected),
                processor_percent: self.processor(at, collected),
                threads: self.threads,
                unified: false,
            }
        }
    }

    fn valid(status: u32) -> bool {
        status == PDH_CSTATUS_VALID_DATA || status == PDH_CSTATUS_NEW_DATA
    }

    /// Every valid item of a wildcard counter as (instance name, value).
    fn read_array<T>(
        counter: PDH_HCOUNTER,
        format: PDH_FMT,
        value: impl Fn(&PDH_FMT_COUNTERVALUE) -> T,
    ) -> Result<Vec<(String, T)>, ArrayError> {
        let read = two_calls(|buffer| {
            let mut count: u32 = 0;
            let (mut bytes, items) = match buffer {
                None => (0u32, None),
                Some(b) => (
                    u32::try_from(std::mem::size_of_val(b)).unwrap_or(u32::MAX),
                    Some(b.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>()),
                ),
            };
            // SAFETY: `counter` belongs to an open query; `bytes` is the size of the buffer
            // `items` points to (or 0 with no buffer, to ask for the size); PDH writes at
            // most `bytes` bytes and the item count.
            let status = unsafe {
                PdhGetFormattedCounterArrayW(counter, format, &mut bytes, &mut count, items)
            };
            match status {
                ERROR_SUCCESS => Call::Done { items: count },
                PDH_MORE_DATA => Call::MoreData { bytes },
                // No instance at all (no process uses a graphics card): an empty list.
                PDH_NO_DATA | PDH_CSTATUS_NO_INSTANCE => Call::Done { items: 0 },
                s => Call::Failed(s),
            }
        });
        let (buffer, count) = read?;
        let count = usize::try_from(count).map_err(|_| ArrayError::TooLarge)?;
        let item_size = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let buffer_bytes = std::mem::size_of_val(buffer.as_slice());
        if count
            .checked_mul(item_size)
            .is_none_or(|n| n > buffer_bytes)
        {
            return Err(ArrayError::TooLarge);
        }
        let base = buffer.as_ptr();
        // SAFETY: the buffer is 8-byte aligned (u64), PDH wrote `count` items at its start,
        // and `count * item_size` fits in it (checked above).
        let items: &[PDH_FMT_COUNTERVALUE_ITEM_W] =
            unsafe { std::slice::from_raw_parts(base.cast(), count) };
        // SAFETY: the same buffer seen as 16-bit units; u64 alignment satisfies u16's.
        let units: &[u16] =
            unsafe { std::slice::from_raw_parts(base.cast::<u16>(), buffer_bytes / 2) };
        let mut out = Vec::with_capacity(count);
        for item in items {
            if !valid(item.FmtValue.CStatus) {
                continue;
            }
            // The name pointer must point into this buffer, at a whole 16-bit unit.
            let offset = item.szName.0.addr().checked_sub(base.addr());
            let Some(name) = offset
                .filter(|o| o % 2 == 0)
                .and_then(|o| wide_at(units, o / 2))
            else {
                continue;
            };
            out.push((name, value(&item.FmtValue)));
        }
        Ok(out)
    }

    /// DXGI's adapters, without creating a device.
    fn list_adapters() -> ::windows::core::Result<Vec<AdapterInfo>> {
        // SAFETY: CreateDXGIFactory1 has no preconditions; it returns an owned interface.
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }?;
        let mut out = Vec::new();
        for index in 0u32..64 {
            // SAFETY: `factory` is live; an index past the last adapter gives
            // DXGI_ERROR_NOT_FOUND.
            let adapter = match unsafe { factory.EnumAdapters1(index) } {
                Ok(adapter) => adapter,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(e) => return Err(e),
            };
            // SAFETY: `adapter` is live; GetDesc1 only fills the returned structure.
            let desc = unsafe { adapter.GetDesc1() }?;
            out.push(AdapterInfo {
                description: description_from_wide(&desc.Description),
                flags: desc.Flags,
                dedicated_bytes: u64::try_from(desc.DedicatedVideoMemory).unwrap_or(u64::MAX),
                luid: Luid::from_parts(desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// `instance<TAB>value` lines (blank lines and `#` comments skipped).
    fn pairs(text: &str) -> Vec<(String, String)> {
        text.lines()
            .map(|l| l.trim_end_matches('\r'))
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|l| {
                let (a, b) = l.split_once('\t').unwrap_or((l, ""));
                (a.to_string(), b.to_string())
            })
            .collect()
    }

    const DGPU: Luid = Luid {
        high: 0,
        low: 0x0000_E2F0,
    };
    const IGPU: Luid = Luid {
        high: 1,
        low: 0x0000_D1E5,
    };

    #[test]
    fn engine_instance_names() {
        assert_eq!(
            parse_engine_instance("pid_1234_luid_0x00000000_0x0000D1E5_phys_0_eng_0_engtype_3D"),
            Some(EngineInstance {
                luid: Luid {
                    high: 0,
                    low: 0xD1E5
                },
                phys: 0,
                eng: 0,
                engtype: "3D".into(),
            })
        );
        let e = parse_engine_instance(
            "pid_77_luid_0xFFFFFFFF_0x8000ABCD_phys_1_eng_12_engtype_Video Encode\r",
        )
        .unwrap();
        assert_eq!(
            e.luid,
            Luid {
                high: 0xFFFF_FFFF,
                low: 0x8000_ABCD
            }
        );
        assert_eq!((e.phys, e.eng, e.engtype.as_str()), (1, 12, "Video Encode"));
        // The process id never comes out: the struct has no field for it.
        assert_eq!(
            parse_engine_instance("luid_0x00000000_0x0000D1E5_phys_0_eng_3_engtype_Copy")
                .map(|e| e.eng),
            Some(3)
        );
        for bad in [
            "",
            "pid_1234",
            "pid__luid_0x0_0x1_phys_0_eng_0_engtype_3D",
            "pid_1234_luid_0x0000000g_0x0000D1E5_phys_0_eng_0_engtype_3D",
            "pid_1234_luid_0x000000000_0x0000D1E5_phys_0_eng_0_engtype_3D",
            "pid_1234_luid_0x00000000_0x0000D1E5_phys_0_eng_0",
            "pid_1234_luid_0x00000000_0x0000D1E5_phys_x_eng_0_engtype_3D",
            "pid_99999999999_luid_0x00000000_0x0000D1E5_phys_0_eng_0_engtype_3D",
            "_Total",
        ] {
            assert_eq!(parse_engine_instance(bad), None, "{bad:?}");
        }
        let long = format!(
            "pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_{}",
            "x".repeat(10_000)
        );
        assert_eq!(parse_engine_instance(&long).unwrap().engtype.len(), 64);
    }

    #[test]
    fn adapter_instance_names() {
        assert_eq!(
            parse_adapter_instance("luid_0x00000001_0x0000D1E5_phys_0"),
            Some(AdapterInstance {
                luid: IGPU,
                phys: 0
            })
        );
        assert_eq!(
            parse_adapter_instance("luid_0x00000000_0x0000E2F0_phys_1\r"),
            Some(AdapterInstance {
                luid: DGPU,
                phys: 1
            })
        );
        assert_eq!(
            parse_adapter_instance("luid_0x00000000_0x0000E2F0_phys_1_x"),
            None
        );
        assert_eq!(parse_adapter_instance("luid_0x00000000_0x0000E2F0"), None);
        assert_eq!(parse_adapter_instance("_Total"), None);
    }

    #[test]
    fn luids_match_dxgi_parts() {
        assert!(luid_matches(DGPU, 0, 0xE2F0));
        assert!(luid_matches(IGPU, 1, 0xD1E5));
        assert!(!luid_matches(IGPU, 0, 0xD1E5), "the high part counts");
        assert!(!luid_matches(DGPU, 0, 0xE2F1));
        // A negative high part is printed as its bits.
        let e = parse_adapter_instance("luid_0xFFFFFFFF_0x00000001_phys_0").unwrap();
        assert!(luid_matches(e.luid, -1, 1));
    }

    #[test]
    fn busiest_engine_of_each_adapter() {
        let rows = pairs(&fixture("pdh_gpu_engines.txt"));
        let items: Vec<(&str, f64)> = rows
            .iter()
            .map(|(n, v)| (n.as_str(), v.parse::<f64>().unwrap_or(f64::NAN)))
            .collect();
        let per = busiest_engine_per_adapter(items);
        // The discrete card: engine 0 is 10.5 + 30.25 + 12 = 52.75 over three processes,
        // engine 1 is one process at 63: the busiest engine wins, not the sum of engines.
        assert_eq!(per.get(&DGPU), Some(&63.0));
        // The integrated card (high part 1): engine 0 is 20 + 25.
        assert_eq!(per.get(&IGPU), Some(&45.0));
        assert_eq!(per.len(), 2, "malformed names and values are left out");
    }

    #[test]
    fn many_processes_on_one_engine_are_capped_once() {
        let text = fixture("pdh_gpu_engines_many.txt");
        let rows = pairs(&text);
        assert!(rows.len() >= 200);
        let items: Vec<(&str, f64)> = rows
            .iter()
            .map(|(n, v)| (n.as_str(), v.parse::<f64>().unwrap()))
            .collect();
        let per = busiest_engine_per_adapter(items);
        let busiest = per[&DGPU];
        assert!(busiest > 100.0, "the raw sum is kept: {busiest}");
        assert_eq!(crate::cap_percent(busiest), Some(100.0));
    }

    #[test]
    fn dedicated_memory_per_adapter() {
        let rows = pairs(&fixture("pdh_adapter_memory.txt"));
        let items: Vec<(&str, i64)> = rows
            .iter()
            .filter_map(|(n, v)| Some((n.as_str(), v.parse::<i64>().ok()?)))
            .collect();
        let per = memory_per_adapter(items);
        // Two parts of a linked adapter are added up.
        assert_eq!(per.get(&DGPU), Some(&(12_884_901_888 + 1_073_741_824)));
        assert_eq!(per.get(&IGPU), Some(&536_870_912));
        assert_eq!(per.len(), 2, "negative values and bad names are left out");
        // Sums saturate.
        let per = memory_per_adapter([
            ("luid_0x0_0x1_phys_0", i64::MAX),
            ("luid_0x0_0x1_phys_1", i64::MAX),
            ("luid_0x0_0x1_phys_2", i64::MAX),
        ]);
        assert_eq!(per[&Luid { high: 0, low: 1 }], u64::MAX);
    }

    #[test]
    fn an_adapter_is_idle_only_when_pdh_lists_it() {
        let engine_rows = pairs(&fixture("pdh_gpu_engines.txt"));
        let engines: Vec<(&str, f64)> = engine_rows
            .iter()
            .map(|(n, v)| (n.as_str(), v.parse::<f64>().unwrap_or(f64::NAN)))
            .collect();
        let memory_rows = pairs(&fixture("pdh_adapter_memory.txt"));
        let memory: Vec<(&str, i64)> = memory_rows
            .iter()
            .filter_map(|(n, v)| Some((n.as_str(), v.parse::<i64>().ok()?)))
            .collect();
        assert_eq!(adapter_busy(&engines, &memory, DGPU), Reading::value(63.0));
        assert_eq!(adapter_busy(&engines, &memory, IGPU), Reading::value(45.0));
        // PDH lists the adapter in GPU Adapter Memory, but no process uses it: idle.
        let listed_only = Luid {
            high: 0,
            low: 0x1111,
        };
        assert_eq!(
            adapter_busy(&engines, &memory, listed_only),
            Reading::value(0.0)
        );
        assert_eq!(
            adapter_busy(&[] as &[(&str, f64)], &memory, DGPU),
            Reading::value(0.0),
            "no engine instance at all, the adapter listed: idle"
        );
        // DXGI's adapter is in neither list (the LUID halves read the other way round, or
        // the memory counter is missing): not available, never a plausible 0 %.
        let swapped = Luid {
            high: 0x0000_E2F0,
            low: 0,
        };
        assert_eq!(
            adapter_busy(&engines, &memory, swapped),
            Reading::gap(Gap::SystemGivesNone)
        );
        assert_eq!(
            adapter_memory(&memory, swapped),
            Reading::gap(Gap::SystemGivesNone)
        );
        assert_eq!(
            adapter_busy(&engines, &[] as &[(&str, i64)], listed_only),
            Reading::gap(Gap::SystemGivesNone),
            "the memory counter gave nothing"
        );
        // Instance names in a format this crate does not know: a failed read, even when
        // the memory list names the adapter.
        let unknown = [
            (
                "pid_1234_luid_00000000_0000E2F0_phys_0_eng_0_engtype_3D",
                80.0,
            ),
            ("adapter 0 engine 0", 10.0),
        ];
        assert_eq!(
            adapter_busy(&unknown, &memory, DGPU),
            Reading::gap(Gap::ReadFailed)
        );
        let unknown_memory = [("luid_00000000_0000E2F0_phys_0", 5_i64)];
        assert_eq!(
            adapter_memory(&unknown_memory, DGPU),
            Reading::gap(Gap::ReadFailed)
        );
        assert_eq!(
            adapter_busy(&engines, &unknown_memory, listed_only),
            Reading::gap(Gap::SystemGivesNone)
        );
        // The memory reading itself: linked parts added, 1 PiB the limit.
        assert_eq!(
            adapter_memory(&memory, DGPU),
            Reading::value(12_884_901_888 + 1_073_741_824)
        );
        assert_eq!(
            adapter_memory(&[] as &[(&str, i64)], DGPU),
            Reading::gap(Gap::SystemGivesNone)
        );
        assert_eq!(
            adapter_memory(
                &[("luid_0x0_0x1_phys_0", i64::MAX)],
                Luid { high: 0, low: 1 }
            ),
            Reading::gap(Gap::ReadFailed)
        );
        // Sums of engines that overflow to infinity are no reading.
        let huge = [
            ("luid_0x0_0x1_phys_0_eng_0_engtype_3D", f64::MAX),
            ("pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D", f64::MAX),
        ];
        assert_eq!(
            adapter_busy(&huge, &[] as &[(&str, i64)], Luid { high: 0, low: 1 }),
            Reading::gap(Gap::ReadFailed)
        );
    }

    fn adapters() -> Vec<AdapterInfo> {
        pairs(&fixture("dxgi_adapters.txt"))
            .into_iter()
            .map(|(description, rest)| {
                let f: Vec<&str> = rest.split('\t').collect();
                AdapterInfo {
                    description,
                    flags: f[0].parse().unwrap(),
                    dedicated_bytes: f[1].parse().unwrap(),
                    luid: Luid {
                        high: u32::from_str_radix(f[2].trim_start_matches("0x"), 16).unwrap(),
                        low: u32::from_str_radix(f[3].trim_start_matches("0x"), 16).unwrap(),
                    },
                }
            })
            .collect()
    }

    fn focus(description: &str) -> Focus {
        Focus {
            description: Some(description.into()),
            total_hint_bytes: None,
        }
    }

    #[test]
    fn the_probe_description_picks_the_adapter() {
        let list = adapters();
        assert_eq!(list.len(), 6);
        let pick = |f: &Focus| pick_adapter(&list, f).map(|a| a.description.clone());
        assert_eq!(
            pick(&focus("AMD Radeon RX 9070 XT")).as_deref(),
            Some("AMD Radeon RX 9070 XT")
        );
        assert_eq!(
            pick(&focus("  amd radeon(tm) graphics ")).as_deref(),
            Some("AMD Radeon(TM) Graphics"),
            "the integrated card when the probe names it"
        );
        // No description, or one that matches nothing: the most dedicated memory.
        assert_eq!(
            pick(&Focus::default()).as_deref(),
            Some("AMD Radeon RX 9070 XT")
        );
        assert_eq!(
            pick(&focus("NVIDIA GeForce RTX 4090")).as_deref(),
            Some("AMD Radeon RX 9070 XT")
        );
        assert_eq!(pick(&focus("")).as_deref(), Some("AMD Radeon RX 9070 XT"));
        // Never a software adapter or a basic render driver, even by name.
        assert_eq!(
            pick(&focus("Microsoft Basic Render Driver")).as_deref(),
            Some("AMD Radeon RX 9070 XT")
        );
        assert_eq!(
            pick(&focus("Software Rasterizer With Lots Of Memory")).as_deref(),
            Some("AMD Radeon RX 9070 XT")
        );
    }

    #[test]
    fn only_software_adapters_mean_no_graphics_card() {
        let list: Vec<AdapterInfo> = adapters()
            .into_iter()
            .filter(|a| is_software_adapter(a.flags, &a.description))
            .collect();
        assert_eq!(
            list.len(),
            4,
            "two flagged, the basic render driver by name, Parsec"
        );
        assert_eq!(pick_adapter(&list, &Focus::default()), None);
        assert_eq!(pick_adapter(&[], &Focus::default()), None);
    }

    #[test]
    fn equal_memory_keeps_the_first() {
        let a = |d: &str, low| AdapterInfo {
            description: d.into(),
            flags: 0,
            dedicated_bytes: 8 << 30,
            luid: Luid { high: 0, low },
        };
        let list = [a("First", 1), a("Second", 2)];
        assert_eq!(
            pick_adapter(&list, &Focus::default()).unwrap().description,
            "First"
        );
    }

    #[test]
    fn software_adapters() {
        assert!(is_software_adapter(ADAPTER_FLAG_SOFTWARE, "Anything"));
        assert!(is_software_adapter(0, "Microsoft Basic Render Driver"));
        assert!(is_software_adapter(0, "Microsoft Hyper-V Video"));
        assert!(is_software_adapter(0, "Parsec Virtual Display Adapter"));
        assert!(!is_software_adapter(0, "AMD Radeon RX 9070 XT"));
        assert!(
            !is_software_adapter(1, "AMD Radeon RX 9070 XT"),
            "remote flag only"
        );
    }

    #[test]
    fn descriptions_from_dxgi() {
        let mut wide = [0u16; 128];
        for (i, c) in "AMD Radeon RX 9070 XT".encode_utf16().enumerate() {
            wide[i] = c;
        }
        assert_eq!(description_from_wide(&wide), "AMD Radeon RX 9070 XT");
        // No NUL at all: the whole array; invalid UTF-16 replaced.
        let all = [0x0041u16; 128];
        assert_eq!(description_from_wide(&all).len(), 128);
        assert_eq!(description_from_wide(&[0xD800, 0x0041, 0]), "\u{FFFD}A");
        // Hostile names are escaped and capped when shown.
        let hostile: Vec<u16> = format!("Card\u{202E}{}", "B".repeat(120))
            .encode_utf16()
            .collect();
        let shown = crate::escape_text(&description_from_wide(&hostile), crate::NAME_CAP);
        assert!(shown.starts_with("Card[U+202E]B"));
        assert_eq!(shown.chars().count(), crate::NAME_CAP);
    }

    #[test]
    fn system_times_add_up() {
        // Kernel time includes idle time.
        assert_eq!(
            system_times(800, 1000, 200),
            Some(crate::CpuTimes {
                busy: 400,
                total: 1200
            })
        );
        assert_eq!(system_times(5000, 1000, 200), None, "idle above all time");
        assert_eq!(system_times(0, u64::MAX, 1), None);
        assert_eq!(filetime(0xFFFF_FFFF, 1), 0x1_FFFF_FFFF);
        assert_eq!(filetime(u32::MAX, u32::MAX), u64::MAX);
    }

    /// A fake `PdhGetFormattedCounterArrayW`: `sizes` are the buffer sizes the instance
    /// list needs over time; the list grows between the size call and the data call (a
    /// process started) until the last size is reached.
    struct Changing {
        sizes: Vec<u32>,
        round: usize,
        calls: u32,
    }

    impl Changing {
        fn needed(&self, round: usize) -> u32 {
            self.sizes[round.min(self.sizes.len() - 1)]
        }

        fn call(&mut self, buffer: Option<&mut [u64]>) -> Call {
            self.calls += 1;
            match buffer {
                None => Call::MoreData {
                    bytes: self.needed(self.round),
                },
                Some(b) => {
                    let needed = self.needed(self.round + 1);
                    self.round += 1;
                    if std::mem::size_of_val(b) < needed as usize {
                        Call::MoreData { bytes: needed }
                    } else {
                        b[0] = 0xAB;
                        Call::Done { items: needed / 24 }
                    }
                }
            }
        }
    }

    #[test]
    fn instances_that_change_between_calls_are_read_again() {
        // The instance lists of two snapshots (the second has more processes).
        let first = pairs(&fixture("pdh_gpu_engines.txt")).len() as u32;
        let second = pairs(&fixture("pdh_gpu_engines_many.txt")).len() as u32;
        let mut fake = Changing {
            sizes: vec![first * 24, second * 24],
            round: 0,
            calls: 0,
        };
        let (buffer, items) = two_calls(|b| fake.call(b)).unwrap();
        assert_eq!(items, second);
        assert_eq!(buffer[0], 0xAB);
        assert!(std::mem::size_of_val(buffer.as_slice()) >= (second * 24) as usize);
        assert_eq!(fake.calls, 4, "size, fill (too small), size, fill");
    }

    #[test]
    fn instances_that_keep_changing_give_up_after_three_retries() {
        let mut calls = 0;
        let result = two_calls(|b| {
            calls += 1;
            match b {
                None => Call::MoreData { bytes: 64 },
                Some(_) => Call::MoreData { bytes: 128 },
            }
        });
        assert_eq!(result, Err(ArrayError::KeptChanging));
        assert_eq!(calls, 8, "the first try and three retries, two calls each");
    }

    #[test]
    fn two_call_rule_edges() {
        // No instances: PDH answers the size call with success.
        assert_eq!(two_calls(|_| Call::Done { items: 0 }), Ok((Vec::new(), 0)));
        // A failure on either call is reported.
        assert_eq!(two_calls(|_| Call::Failed(7)), Err(ArrayError::Status(7)));
        let mut n = 0;
        assert_eq!(
            two_calls(|b| {
                n += 1;
                if b.is_none() {
                    Call::MoreData { bytes: 10 }
                } else {
                    Call::Failed(9)
                }
            }),
            Err(ArrayError::Status(9))
        );
        // A size beyond the cap is refused before allocating.
        assert_eq!(
            two_calls(|_| Call::MoreData { bytes: u32::MAX }),
            Err(ArrayError::TooLarge)
        );
        // The buffer always holds at least the bytes asked for, 8-byte aligned.
        let (buffer, items) = two_calls(|b| match b {
            None => Call::MoreData { bytes: 13 },
            Some(b) => {
                assert!(std::mem::size_of_val(b) >= 13);
                Call::Done { items: 1 }
            }
        })
        .unwrap();
        assert_eq!((buffer.len(), items), (2, 1));
    }

    #[test]
    fn names_are_never_read_past_the_buffer() {
        let units: Vec<u16> = "ab\0cd".encode_utf16().collect();
        assert_eq!(wide_at(&units, 0).as_deref(), Some("ab"));
        assert_eq!(wide_at(&units, 1).as_deref(), Some("b"));
        assert_eq!(wide_at(&units, 3), None, "no NUL before the end");
        assert_eq!(wide_at(&units, 5), None);
        assert_eq!(wide_at(&units, usize::MAX), None);
        assert_eq!(wide_at(&[0xD800, 0], 0), None, "invalid UTF-16 is left out");
    }
}
