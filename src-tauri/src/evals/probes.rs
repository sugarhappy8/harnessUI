//! The capability check: three short probes, each under a few request settings, and the
//! choice of the settings that worked best.
//!
//! - Tool calling: read a named file with `read_file` and reply with a code word from it.
//!   The word is new for every run, so only a model that read the file can know it.
//! - Editing: make one specified line change with `edit_file`. Verified by the file's
//!   content.
//! - Error recovery: the obvious text to replace appears twice, so a first edit that names
//!   only that text fails with the tool's explanation; the probe passes when the file ends
//!   up right. Verified by the file's content, never by the wording of any error.
//!
//! The check has the file tools only (no terminal). Its verifications read the run's
//! folder after the model finished; nothing the model says decides a result, except
//! that the tool-calling probe's answer must contain the word it could only have read.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::harness::{self, Arrival, AuditFn, Model, Record, Settings, Spec};
use super::workspace::Workspace;

/// The three probes, in the order they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Probe {
    ToolCalling,
    Editing,
    ErrorRecovery,
}

pub const PROBES: [Probe; 3] = [Probe::ToolCalling, Probe::Editing, Probe::ErrorRecovery];

impl Probe {
    pub fn title(self) -> &'static str {
        match self {
            Probe::ToolCalling => "Tool calling",
            Probe::Editing => "Editing",
            Probe::ErrorRecovery => "Error recovery",
        }
    }
}

/// Model calls one probe may take.
const STEP_CAP: u32 = 8;
/// How long one probe may run.
const TIME_LIMIT: Duration = Duration::from_secs(180);

/// Words for the tool-calling probe.
const WORDS: &[&str] = &[
    "heron", "lantern", "pebble", "walnut", "harbor", "meadow", "copper", "falcon", "thistle",
    "orchid", "glacier", "saffron", "juniper", "quartz", "maple", "otter",
];

pub const EDIT_BEFORE: &str = "name = demo\ncolor = blue\nsize = 3\n";
pub const EDIT_AFTER: &str = "name = demo\ncolor = green\nsize = 3\n";
pub const RECOVERY_BEFORE: &str = "[client]\nlimit = 10\nretries = 2\n\n[server]\nlimit = 10\nretries = 2\n";
pub const RECOVERY_AFTER: &str = "[client]\nlimit = 10\nretries = 2\n\n[server]\nlimit = 20\nretries = 2\n";

/// One probe's outcome under one variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub probe: Probe,
    pub passed: bool,
    /// How the model's tool calls arrived: native calls, or read from its text.
    pub arrival: Arrival,
    pub tool_failures: u32,
    pub failed_edits: u32,
    pub blocked: u32,
    pub steps: u32,
    pub tokens: u64,
    pub time_ms: u64,
    /// How the run ended (`completed`, `timeLimit`, …).
    pub status: String,
    /// One line on why it failed, or how it passed.
    pub note: String,
}

/// One variant's results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantResult {
    pub label: String,
    pub settings: Settings,
    pub passed: u32,
    pub tool_failures: u32,
    pub probes: Vec<ProbeResult>,
}

/// The settings variants for a model: always the model's defaults and a low temperature;
/// thinking off as well when the model can think.
pub fn variants(can_think: bool) -> Vec<(String, Settings)> {
    let mut out = vec![("Default".to_owned(), Settings::default())];
    if can_think {
        out.push((
            "Thinking off".to_owned(),
            Settings {
                thinking_off: true,
                temperature: None,
            },
        ));
    }
    out.push((
        "Temperature 0.2".to_owned(),
        Settings {
            thinking_off: false,
            temperature: Some(0.2),
        },
    ));
    if can_think {
        out.push((
            "Thinking off, temperature 0.2".to_owned(),
            Settings {
                thinking_off: true,
                temperature: Some(0.2),
            },
        ));
    }
    out
}

/// What a probe's run starts from, and how it is judged.
pub struct Setup {
    pub files: Vec<(&'static str, String)>,
    pub prompt: String,
    /// The tool-calling probe's word.
    pub word: Option<String>,
}

pub fn setup(probe: Probe) -> Setup {
    match probe {
        Probe::ToolCalling => {
            let pick = uuid::Uuid::new_v4().as_bytes()[0] as usize % WORDS.len();
            let word = WORDS[pick].to_owned();
            Setup {
                files: vec![(
                    "note.txt",
                    format!(
                        "Notes for a short check.\nThe code word is {word}.\nNothing else in this file matters.\n"
                    ),
                )],
                prompt: "Use the read_file tool to read the file note.txt in the project folder. Then reply with the code word written in it, and nothing else.".into(),
                word: Some(word),
            }
        }
        Probe::Editing => Setup {
            files: vec![("settings.txt", EDIT_BEFORE.to_owned())],
            prompt: "In the file settings.txt, change the line `color = blue` to `color = green` with the edit_file tool. Change nothing else.".into(),
            word: None,
        },
        Probe::ErrorRecovery => Setup {
            files: vec![("limits.txt", RECOVERY_BEFORE.to_owned())],
            prompt: "In the file limits.txt, change `limit = 10` to `limit = 20` in the [server] section only, with the edit_file tool. The [client] section must stay exactly as it is.".into(),
            word: None,
        },
    }
}

fn lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// Whether `answer` holds `word` as a whole word, in any letter case.
fn says_word(answer: &str, word: &str) -> bool {
    answer
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w.eq_ignore_ascii_case(word))
}

/// Judges a finished probe run by what is in its folder (and, for tool calling, the word).
pub fn judge(probe: Probe, setup: &Setup, record: &Record, ws: &Workspace) -> (bool, String) {
    match probe {
        Probe::ToolCalling => {
            let word = setup.word.as_deref().unwrap_or_default();
            if record.succeeded("read_file") == 0 {
                (false, "The file was not read with read_file.".into())
            } else if !says_word(&record.answer, word) {
                (false, "The reply did not contain the word from the file.".into())
            } else {
                (true, "Read the file and reported the word.".into())
            }
        }
        Probe::Editing | Probe::ErrorRecovery => {
            let (file, expected) = if probe == Probe::Editing {
                ("settings.txt", EDIT_AFTER)
            } else {
                ("limits.txt", RECOVERY_AFTER)
            };
            let content = ws.read(file).map(|t| lf(&t));
            if content.as_deref() == Some(expected) && record.succeeded("edit_file") > 0 {
                let how = if probe == Probe::ErrorRecovery && record.failed_edits() > 0 {
                    "Recovered after a failed edit; the file is right."
                } else {
                    "The file is right."
                };
                (true, how.into())
            } else if content.as_deref() == Some(expected) {
                (false, "The file is right, but not through edit_file.".into())
            } else if record.succeeded("edit_file") == 0 {
                (false, "No edit succeeded.".into())
            } else {
                (false, "The file does not have the expected content.".into())
            }
        }
    }
}

/// Where a check reports progress: (probes done, probes in all, what runs now).
pub type ProgressFn = Arc<dyn Fn(u32, u32, String) + Send + Sync>;

/// Runs one probe in a fresh folder of its own.
pub async fn run_probe(
    model: &Model,
    probe: Probe,
    settings: Settings,
    forbidden: &[String],
    cancel: &CancellationToken,
    audit: &AuditFn,
) -> ProbeResult {
    let setup = setup(probe);
    let ws = match Workspace::create() {
        Ok(ws) => ws,
        Err(e) => return not_run(probe, format!("The run's folder could not be made: {e}")),
    };
    if let Err(e) = ws.write_files(setup.files.iter().map(|(p, c)| (*p, c.as_bytes()))) {
        return not_run(probe, format!("The run's files could not be written: {e}"));
    }
    let spec = Spec {
        prompt: setup.prompt.clone(),
        terminal: None,
        settings,
        step_cap: STEP_CAP,
        time_limit: TIME_LIMIT,
        forbidden: forbidden.to_vec(),
    };
    let record = harness::run(model, spec, &ws, cancel.clone(), audit.clone()).await;
    let (passed, note) = match record.status.as_str() {
        "stopped" => (false, "Stopped.".to_owned()),
        "failed" => (
            false,
            format!(
                "The model could not answer: {}",
                record.failure.clone().unwrap_or_default()
            ),
        ),
        _ => judge(probe, &setup, &record, &ws),
    };
    ProbeResult {
        probe,
        passed,
        arrival: record.arrival(),
        tool_failures: record.tool_failures(),
        failed_edits: record.failed_edits(),
        blocked: record.blocked(),
        steps: record.steps,
        tokens: record.prompt_tokens + record.completion_tokens,
        time_ms: record.time_ms,
        status: record.status,
        note,
    }
}

fn not_run(probe: Probe, note: String) -> ProbeResult {
    ProbeResult {
        probe,
        passed: false,
        arrival: Arrival::None,
        tool_failures: 0,
        failed_edits: 0,
        blocked: 0,
        steps: 0,
        tokens: 0,
        time_ms: 0,
        status: "failed".into(),
        note,
    }
}

/// Runs every probe under every variant, in order. Stops early when cancelled; the
/// variants finished so far are returned.
pub async fn check(
    model: &Model,
    variants: &[(String, Settings)],
    forbidden: &[String],
    cancel: CancellationToken,
    audit: AuditFn,
    progress: ProgressFn,
) -> Vec<VariantResult> {
    let total = (variants.len() * PROBES.len()) as u32;
    let mut done = 0;
    let mut out = Vec::new();
    for (label, settings) in variants {
        let mut probes = Vec::new();
        for probe in PROBES {
            if cancel.is_cancelled() {
                return out;
            }
            progress(done, total, format!("{label}: {}", probe.title()));
            probes.push(run_probe(model, probe, *settings, forbidden, &cancel, &audit).await);
            done += 1;
        }
        if cancel.is_cancelled() {
            return out;
        }
        out.push(VariantResult {
            label: label.clone(),
            settings: *settings,
            passed: probes.iter().filter(|p| p.passed).count() as u32,
            tool_failures: probes.iter().map(|p| p.tool_failures).sum(),
            probes,
        });
    }
    progress(done, total, String::new());
    out
}

/// The best variant: most probes passed, then fewest tool failures, then the earlier one
/// (so the model's defaults win a tie). `None` when no variant passed any probe.
pub fn best(results: &[VariantResult]) -> Option<&VariantResult> {
    let mut best: Option<&VariantResult> = None;
    for r in results.iter().filter(|r| r.passed > 0) {
        let better = match best {
            None => true,
            Some(b) => {
                r.passed > b.passed || (r.passed == b.passed && r.tool_failures < b.tool_failures)
            }
        };
        if better {
            best = Some(r);
        }
    }
    best
}

/// What a profile sets for a chosen variant.
pub fn defaults_of(settings: Settings) -> super::profile::Defaults {
    super::profile::Defaults {
        thinking: settings.thinking_off.then_some(false),
        temperature: settings.temperature,
    }
}

#[cfg(test)]
mod tests;
