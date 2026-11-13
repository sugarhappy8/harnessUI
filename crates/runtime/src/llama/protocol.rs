//! Adapters for what llama-server writes: the managed-child protocol on stdout and the
//! log lines on stderr (ARCHITECTURE.md 7.4).
//!
//! Both are internal to llama.cpp and can change in any release, so they are parsed here
//! and nowhere else, and the fixtures under `tests/fixtures/` were captured from the pinned
//! build. Unknown fields and lines are ignored.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::fit_params::escape_text;

/// Prefix of a managed-child state line on stdout.
pub const CHILD_STATE_PREFIX: &str = "cmd_child_to_router:state:";

/// A state reported by llama-server in managed-child mode.
#[derive(Clone, Debug, PartialEq)]
pub enum ChildState {
    /// The model is loading.
    Loading {
        /// Load stages (`text_model`, `spec_model`, `mmproj_model`).
        stages: Vec<String>,
        /// The stage in progress.
        current: Option<String>,
        /// Progress of the current stage, 0 to 1.
        progress: Option<f32>,
    },
    /// The model is loaded and the server accepts requests.
    Ready {
        /// The context the server allocated (`meta.n_ctx`).
        n_ctx: Option<u32>,
        /// The model id the server reports.
        model_id: Option<String>,
        /// Where the model went: the memory llama.cpp allocated per device, from the
        /// payload's `memory` (item 6, I6-7). `None` when the line carries no readable row.
        memory: Option<MemoryReport>,
    },
    /// The server unloaded the model after being idle.
    Sleeping,
    /// A state this adapter does not know.
    Unknown(String),
}

/// Parses one stdout line. Returns `None` for lines that are not state lines or whose JSON
/// does not parse.
pub fn parse_child_line(line: &str) -> Option<ChildState> {
    let json = line
        .trim_end_matches(['\r', '\n'])
        .strip_prefix(CHILD_STATE_PREFIX)?;
    let value: Value = serde_json::from_str(json).ok()?;
    let state = value.get("state")?.as_str()?;
    let payload = value.get("payload").unwrap_or(&Value::Null);
    Some(match state {
        "loading" => ChildState::Loading {
            stages: payload
                .get("stages")
                .and_then(Value::as_array)
                .map(|stages| stages.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default(),
            current: payload.get("current").and_then(Value::as_str).map(str::to_owned),
            #[allow(clippy::cast_possible_truncation)] // a 0..1 progress value
            progress: payload.get("value").and_then(Value::as_f64).map(|v| v.clamp(0.0, 1.0) as f32),
        },
        "ready" => ChildState::Ready {
            n_ctx: payload
                .pointer("/meta/n_ctx")
                .or_else(|| payload.pointer("/memory/n_ctx"))
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok()),
            model_id: payload.get("id").and_then(Value::as_str).map(str::to_owned),
            memory: payload.get("memory").and_then(ready_memory),
        },
        "sleeping" => ChildState::Sleeping,
        other => ChildState::Unknown(other.to_owned()),
    })
}

/// The kind of a row in a memory report (item 6, section 2.1). Read from llama.cpp's own
/// words explicitly ([`MemoryRowKind::from_llama`]): `"device"`, `"host"` and
/// `"buffer_type"`; never through a derived `Deserialize`, which would not know
/// `"buffer_type"`. Written to the webview as `"device"`, `"host"` and `"bufferType"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryRowKind {
    /// A graphics device (`Vulkan0`, `MTL0`, `CUDA0`).
    Device,
    /// Computer memory used by the processor (`Host`).
    Host,
    /// A buffer type of no model device, held in computer memory (repacked weights:
    /// `CPU_REPACK`, `AMX`).
    BufferType,
}

impl MemoryRowKind {
    /// The kind for llama.cpp's word, or `None` for a word this build does not know (the
    /// row is then skipped).
    pub fn from_llama(word: &str) -> Option<Self> {
        match word {
            "device" => Some(Self::Device),
            "host" => Some(Self::Host),
            "buffer_type" => Some(Self::BufferType),
            _ => None,
        }
    }
}

/// One row of a memory report, in bytes (item 6, section 2.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRow {
    /// Device, host or buffer type.
    pub kind: MemoryRowKind,
    /// llama.cpp's name for it: `Vulkan0`, `Host`, `CPU_REPACK`. Escaped and cut to
    /// [`MAX_ROW_NAME_CHARS`] characters (section 2.0).
    pub name: String,
    /// The device's own description (`AMD Radeon RX 9070 XT`), from fit-params' device
    /// rows; escaped and cut to [`MAX_ROW_DESCRIPTION_CHARS`]. Empty in the ready line,
    /// which carries none.
    pub description: String,
    /// The device's whole memory (device rows only).
    pub total_bytes: Option<u64>,
    /// The device's free memory at the moment of the run (device rows only).
    pub free_bytes: Option<u64>,
    /// The model's weights.
    pub model_bytes: u64,
    /// The conversation memory (KV cache).
    pub context_bytes: u64,
    /// The working space (compute buffers).
    pub compute_bytes: u64,
}

/// Where the model went or would go: llama.cpp's memory figures per device (item 6).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryReport {
    /// The context the figures are for, when reported.
    pub n_ctx: Option<u32>,
    /// At most [`MAX_MEMORY_ROWS`] rows.
    pub rows: Vec<MemoryRow>,
}

/// Most rows read from one memory report.
pub const MAX_MEMORY_ROWS: usize = 16;

/// Longest row name kept, in characters (section 2.0).
pub const MAX_ROW_NAME_CHARS: usize = 64;

/// Longest device description kept, in characters (section 2.0).
pub const MAX_ROW_DESCRIPTION_CHARS: usize = 80;

/// Largest byte value a ready-line row may carry (2^50, one pebibyte); a row with a larger
/// one is dropped (section 2.0).
pub const MAX_READY_ROW_BYTES: u64 = 1 << 50;

/// The ready line's `memory` object, in bytes:
/// `{"n_ctx":4096,"devices":[{"kind":"host","name":"Host","model":…,"context":…,
/// "compute":…},{"kind":"device","name":"Vulkan0","total":…,"free":…,…}]}`.
///
/// Kinds are mapped explicitly and unknown kinds skipped; a missing number counts as 0
/// (`total` and `free` stay unknown); a value that is not a whole number from 0 to
/// [`MAX_READY_ROW_BYTES`] drops its row; names are escaped and cut (section 2.0). At most
/// [`MAX_MEMORY_ROWS`] rows are kept. `None` when no row could be read.
fn ready_memory(memory: &Value) -> Option<MemoryReport> {
    let devices = memory.get("devices")?.as_array()?;
    let rows: Vec<MemoryRow> = devices
        .iter()
        .filter_map(ready_row)
        .take(MAX_MEMORY_ROWS)
        .collect();
    if rows.is_empty() {
        return None;
    }
    Some(MemoryReport {
        n_ctx: memory
            .get("n_ctx")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok()),
        rows,
    })
}

/// One row of the ready line's `devices`, or `None` to skip it.
fn ready_row(row: &Value) -> Option<MemoryRow> {
    let kind = MemoryRowKind::from_llama(row.get("kind")?.as_str()?)?;
    // A present value must be a whole number in range; an absent one is unknown.
    let bytes = |key: &str| -> Result<Option<u64>, ()> {
        match row.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => match value.as_u64() {
                Some(n) if n <= MAX_READY_ROW_BYTES => Ok(Some(n)),
                _ => Err(()),
            },
        }
    };
    let device = kind == MemoryRowKind::Device;
    let total = bytes("total").ok()?;
    let free = bytes("free").ok()?;
    Some(MemoryRow {
        kind,
        name: escape_text(
            row.get("name").and_then(Value::as_str).unwrap_or_default(),
            MAX_ROW_NAME_CHARS,
        ),
        description: String::new(),
        total_bytes: if device { total } else { None },
        free_bytes: if device { free } else { None },
        model_bytes: bytes("model").ok()?.unwrap_or(0),
        context_bytes: bytes("context").ok()?.unwrap_or(0),
        compute_bytes: bytes("compute").ok()?.unwrap_or(0),
    })
}

/// A recognised stderr log line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum LogEvent {
    /// `load_model: loading model '<path>'`.
    LoadingModel {
        /// The path as printed.
        path: String,
    },
    /// `model loaded`.
    ModelLoaded,
    /// `listening on http://127.0.0.1:<port>`.
    Listening {
        /// The URL as printed.
        url: String,
    },
    /// `offloaded N/M layers to GPU`.
    Offloaded {
        /// Layers placed on the GPU.
        layers: u32,
        /// Layers in the model.
        total: u32,
    },
}

/// Recognises the stderr lines the runtime cares about.
pub fn scan_log_line(line: &str) -> Option<LogEvent> {
    let line = line.trim_end();
    if let Some(at) = line.find("loading model '") {
        let rest = &line[at + "loading model '".len()..];
        let path = rest.rfind('\'').map_or(rest, |end| &rest[..end]);
        return Some(LogEvent::LoadingModel {
            path: path.to_owned(),
        });
    }
    if line.ends_with(": model loaded") || line == "model loaded" {
        return Some(LogEvent::ModelLoaded);
    }
    if let Some(at) = line.find("listening on ") {
        let url = line[at + "listening on ".len()..]
            .split_whitespace()
            .next()?;
        return Some(LogEvent::Listening {
            url: url.to_owned(),
        });
    }
    if let Some(at) = line.find("offloaded ") {
        let rest = &line[at + "offloaded ".len()..];
        let (counts, tail) = rest.split_once(' ')?;
        if !tail.starts_with("layers to GPU") {
            return None;
        }
        let (layers, total) = counts.split_once('/')?;
        return Some(LogEvent::Offloaded {
            layers: layers.parse().ok()?,
            total: total.parse().ok()?,
        });
    }
    None
}

/// What the log has said so far about the current launch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogInfo {
    /// The model path from `loading model`.
    pub model_path: Option<String>,
    /// Whether `model loaded` was seen.
    pub model_loaded: bool,
    /// The URL from `listening on`.
    pub listening_url: Option<String>,
    /// `(layers, total)` from `offloaded N/M layers to GPU`.
    pub offloaded: Option<(u32, u32)>,
}

impl LogInfo {
    /// Folds one event into the summary.
    pub fn apply(&mut self, event: &LogEvent) {
        match event {
            LogEvent::LoadingModel { path } => self.model_path = Some(path.clone()),
            LogEvent::ModelLoaded => self.model_loaded = true,
            LogEvent::Listening { url } => self.listening_url = Some(url.clone()),
            LogEvent::Offloaded { layers, total } => self.offloaded = Some((*layers, *total)),
        }
    }

    /// Scans a line and folds any event in; returns the event.
    pub fn scan(&mut self, line: &str) -> Option<LogEvent> {
        let event = scan_log_line(line)?;
        self.apply(&event);
        Some(event)
    }

    /// True when a GPU was expected but the log reported every layer on the CPU, which is
    /// what a silent fallback to CPU looks like (ARCHITECTURE.md 7.4).
    pub fn fell_back_to_cpu(&self) -> bool {
        matches!(self.offloaded, Some((0, total)) if total > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANAGED_STDOUT: &str = include_str!("../../tests/fixtures/managed_stdout.txt");
    const STDERR_CPU: &str = include_str!("../../tests/fixtures/server_stderr_cpu.txt");

    #[test]
    fn parses_the_captured_managed_stdout() {
        let states: Vec<ChildState> = MANAGED_STDOUT
            .lines()
            .filter_map(parse_child_line)
            .collect();
        assert_eq!(states.len(), 5);
        assert_eq!(
            states[0],
            ChildState::Loading {
                stages: vec!["text_model".into()],
                current: Some("text_model".into()),
                progress: Some(0.0)
            }
        );
        match &states[2] {
            ChildState::Loading {
                progress: Some(p), ..
            } => assert!((p - 0.568).abs() < 0.001),
            other => panic!("unexpected {other:?}"),
        }
        match &states[4] {
            ChildState::Ready {
                n_ctx,
                model_id,
                memory,
            } => {
                assert_eq!(*n_ctx, Some(4096));
                assert_eq!(model_id.as_deref(), Some("Qwen3-0.6B-Q8_0.gguf"));
                assert!(memory.is_some(), "the ready line carries the memory report");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn tolerates_unknown_states_fields_and_crlf() {
        assert_eq!(
            parse_child_line("cmd_child_to_router:state:{\"state\":\"sleeping\",\"payload\":{}}\r"),
            Some(ChildState::Sleeping)
        );
        assert_eq!(
            parse_child_line("cmd_child_to_router:state:{\"state\":\"warming\",\"extra\":1}"),
            Some(ChildState::Unknown("warming".into()))
        );
        assert_eq!(
            parse_child_line("cmd_child_to_router:state:{\"state\":\"loading\"}"),
            Some(ChildState::Loading {
                stages: vec![],
                current: None,
                progress: None
            })
        );
        assert_eq!(
            parse_child_line(
                "cmd_child_to_router:state:{\"state\":\"ready\",\"payload\":{\"memory\":{\"n_ctx\":2048}}}"
            ),
            Some(ChildState::Ready {
                n_ctx: Some(2048),
                model_id: None,
                memory: None,
            })
        );
        assert_eq!(
            parse_child_line(
                "cmd_child_to_router:state:{\"state\":\"loading\",\"payload\":{\"value\":7.5}}"
            ),
            Some(ChildState::Loading {
                stages: vec![],
                current: None,
                progress: Some(1.0)
            })
        );
    }

    #[test]
    fn rejects_other_lines() {
        assert_eq!(parse_child_line(""), None);
        assert_eq!(parse_child_line("hello"), None);
        assert_eq!(parse_child_line("cmd_child_to_router:state:not json"), None);
        assert_eq!(
            parse_child_line("cmd_child_to_router:state:{\"payload\":{}}"),
            None
        );
        assert_eq!(
            parse_child_line(" cmd_child_to_router:state:{\"state\":\"ready\"}"),
            None
        );
    }

    #[test]
    fn scans_the_captured_stderr() {
        let mut info = LogInfo::default();
        let events: Vec<LogEvent> = STDERR_CPU
            .lines()
            .filter_map(|line| info.scan(line))
            .collect();
        assert_eq!(
            events,
            vec![
                LogEvent::LoadingModel {
                    path: "Qwen3-0.6B-Q8_0.gguf".into()
                },
                LogEvent::ModelLoaded,
                LogEvent::Listening {
                    url: "http://127.0.0.1:18080".into()
                },
            ]
        );
        assert_eq!(info.model_path.as_deref(), Some("Qwen3-0.6B-Q8_0.gguf"));
        assert!(info.model_loaded);
        assert_eq!(info.offloaded, None);
        assert!(!info.fell_back_to_cpu());
    }

    #[test]
    fn scans_offload_lines() {
        assert_eq!(
            scan_log_line("0.01.000.000 I load_tensors: offloaded 29/29 layers to GPU"),
            Some(LogEvent::Offloaded {
                layers: 29,
                total: 29
            })
        );
        let mut info = LogInfo::default();
        info.scan("load_tensors: offloaded 0/37 layers to GPU");
        assert!(info.fell_back_to_cpu());
        assert_eq!(scan_log_line("offloaded x/29 layers to GPU"), None);
        assert_eq!(scan_log_line("offloaded 3/29 tensors"), None);
    }

    #[test]
    fn model_loaded_does_not_match_loading_or_loader_lines() {
        assert_eq!(scan_log_line("llama_model_loader: loaded meta data"), None);
        assert_eq!(
            scan_log_line("E srv    load_model: failed to load model, 'x'"),
            None
        );
        assert_eq!(
            scan_log_line("load_model: loading model 'C:\\Users\\o'brien\\m.gguf'"),
            Some(LogEvent::LoadingModel {
                path: "C:\\Users\\o'brien\\m.gguf".into()
            })
        );
    }

    const READY_VULKAN: &str = include_str!("../../tests/fixtures/managed_ready_memory_vulkan.txt");

    /// The memory report of the ready line of `text`.
    fn ready_memory_of(text: &str) -> Option<MemoryReport> {
        text.lines()
            .filter_map(parse_child_line)
            .find_map(|state| match state {
                ChildState::Ready { memory, .. } => Some(memory),
                _ => None,
            })
            .expect("a ready line")
    }

    /// A ready line with `devices` set to `devices` (JSON text, put on one line).
    fn ready_with(devices: &str) -> String {
        let devices = devices.replace('\n', " ");
        format!(
            "{CHILD_STATE_PREFIX}{{\"state\":\"ready\",\"payload\":{{\"memory\":{{\"n_ctx\":2048,\"devices\":{devices}}}}}}}"
        )
    }

    /// Where the model went, from the ready line (item 6, I6-7): the captured processor
    /// shape (Host plus the repacked `AMX` weights, in bytes) and the Vulkan shape (a
    /// device row with total and free, no description).
    #[test]
    fn ready_line_memory_is_read() {
        let cpu = ready_memory_of(MANAGED_STDOUT).expect("the processor report");
        assert_eq!(
            cpu,
            MemoryReport {
                n_ctx: Some(4096),
                rows: vec![
                    MemoryRow {
                        kind: MemoryRowKind::Host,
                        name: "Host".into(),
                        description: String::new(),
                        total_bytes: None,
                        free_bytes: None,
                        model_bytes: 165_568_512,
                        context_bytes: 469_762_048,
                        compute_bytes: 33_572_896,
                    },
                    MemoryRow {
                        kind: MemoryRowKind::BufferType,
                        name: "AMX".into(),
                        description: String::new(),
                        total_bytes: None,
                        free_bytes: None,
                        model_bytes: 707_731_456,
                        context_bytes: 0,
                        compute_bytes: 0,
                    },
                ],
            }
        );
        let vulkan = ready_memory_of(READY_VULKAN).expect("the Vulkan report");
        assert_eq!(vulkan.n_ctx, Some(16384));
        assert_eq!(vulkan.rows.len(), 3);
        assert_eq!(
            vulkan.rows[0],
            MemoryRow {
                kind: MemoryRowKind::Device,
                name: "Vulkan0".into(),
                description: String::new(),
                total_bytes: Some(17_095_983_104),
                free_bytes: Some(16_148_070_400),
                model_bytes: 10_630_070_272,
                context_bytes: 1_288_699_904,
                compute_bytes: 644_874_240,
            }
        );
        assert_eq!(vulkan.rows[1].kind, MemoryRowKind::Host);
        assert_eq!(vulkan.rows[2].kind, MemoryRowKind::BufferType);
        assert_eq!(vulkan.rows[2].name, "Vulkan_Host");

        // The webview's words for the kinds.
        assert_eq!(
            serde_json::to_value(&vulkan.rows[2]).expect("json")["kind"],
            "bufferType"
        );
        assert_eq!(
            serde_json::to_value(&vulkan.rows[0]).expect("json")["totalBytes"],
            17_095_983_104_u64
        );
    }

    /// Rows that break the rules of section 2.0 are dropped one by one; a line with none
    /// left carries no report.
    #[test]
    fn ready_line_memory_rows_follow_the_number_rules() {
        let limit = MAX_READY_ROW_BYTES;
        let line = ready_with(&format!(
            r#"[
                {{"kind":"host","name":"Host","model":{limit},"context":1,"compute":2}},
                {{"kind":"host","name":"Big","model":{over}}},
                {{"kind":"device","name":"Vulkan0","total":{over},"free":1,"model":1}},
                {{"kind":"device","name":"Neg","model":-1}},
                {{"kind":"device","name":"Frac","context":1.5}},
                {{"kind":"device","name":"Text","compute":"12"}},
                {{"kind":"gpu","name":"Unknown kind","model":1}},
                {{"name":"No kind","model":1}},
                {{"kind":"buffer_type","name":"Missing numbers"}},
                {{"kind":"host","name":"Host total","total":5,"free":6,"model":7}}
            ]"#,
            over = limit + 1,
        ));
        let report = ready_memory_of(&line).expect("a report");
        let names: Vec<&str> = report.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Host", "Missing numbers", "Host total"]);
        assert_eq!(report.rows[0].model_bytes, limit);
        assert_eq!(
            (
                report.rows[1].model_bytes,
                report.rows[1].context_bytes,
                report.rows[1].compute_bytes
            ),
            (0, 0, 0),
            "missing numbers count as 0"
        );
        assert_eq!(
            (report.rows[2].total_bytes, report.rows[2].free_bytes),
            (None, None),
            "total and free belong to device rows only"
        );

        // Nothing readable: no report, and the rest of the line still reads.
        let none = ready_with(r#"[{"kind":"gpu","model":1}]"#);
        assert_eq!(ready_memory_of(&none), None);
        assert_eq!(ready_memory_of(&ready_with("[]")), None);
        assert_eq!(ready_memory_of(&ready_with(r#""devices""#)), None);
        match parse_child_line(&none) {
            Some(ChildState::Ready { n_ctx, .. }) => assert_eq!(n_ctx, Some(2048)),
            other => panic!("unexpected {other:?}"),
        }
    }

    /// Hostile names are escaped and cut; at most 16 rows are kept.
    #[test]
    fn ready_line_memory_names_are_escaped_and_rows_capped() {
        let long = "V".repeat(200);
        let rows: Vec<String> = (0..20)
            .map(|i| format!(r#"{{"kind":"host","name":"Row{i}","model":{i}}}"#))
            .collect();
        let line = ready_with(&format!(
            r#"[{{"kind":"device","name":"Vul\u202ekan\u0000{long}","model":1}},{}]"#,
            rows.join(",")
        ));
        let report = ready_memory_of(&line).expect("a report");
        assert_eq!(report.rows.len(), MAX_MEMORY_ROWS);
        let name = &report.rows[0].name;
        assert!(name.starts_with("Vul[U+202E]kan[U+0000]VVV"), "{name}");
        assert_eq!(name.chars().count(), MAX_ROW_NAME_CHARS);
        assert!(!name.contains('\u{202e}') && !name.contains('\0'));
        assert_eq!(report.rows[15].name, "Row14");

        // Tag characters and the other invisible characters of
        // `models::hf::uploader_text`'s set are escaped too.
        let line = ready_with(
            "[{\"kind\":\"device\",\"name\":\"MTL\u{e0041}0\u{ad}\u{fe0f}\u{180e}\u{206a}\",\"model\":1}]",
        );
        let report = ready_memory_of(&line).expect("a report");
        assert_eq!(
            report.rows[0].name,
            "MTL[U+E0041]0[U+00AD][U+FE0F][U+180E][U+206A]"
        );
    }
}
