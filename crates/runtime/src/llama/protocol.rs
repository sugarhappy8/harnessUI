//! Adapters for what llama-server writes: the managed-child protocol on stdout and the
//! log lines on stderr (ARCHITECTURE.md 7.4).
//!
//! Both are internal to llama.cpp and can change in any release, so they are parsed here
//! and nowhere else, and the fixtures under `tests/fixtures/` were captured from the pinned
//! build. Unknown fields and lines are ignored.

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
        },
        "sleeping" => ChildState::Sleeping,
        other => ChildState::Unknown(other.to_owned()),
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
        assert_eq!(
            states[4],
            ChildState::Ready {
                n_ctx: Some(4096),
                model_id: Some("Qwen3-0.6B-Q8_0.gguf".into())
            }
        );
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
                model_id: None
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
}
