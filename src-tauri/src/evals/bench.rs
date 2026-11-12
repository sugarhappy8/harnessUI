//! A benchmark run: every task in a fresh folder of its own, the model given the file
//! tools and the terminal under the run's rule, then the app's verification. Results are
//! saved as JSON in the app's data folder (`<data>/benchmarks/<id>.json`) and never leave
//! the computer.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::harness::{self, Arrival, AuditFn, Model, Settings, Spec};
use super::tasks::Task;
use super::verify::{self, CheckOutcome, Toolchains};
use super::workspace::Workspace;

/// Model calls one task may take.
const STEP_CAP: u32 = 30;
/// How long one task may run.
const TIME_LIMIT: Duration = Duration::from_secs(600);
/// Saved runs kept; the oldest go first.
const KEEP_RUNS: usize = 50;

/// One task's outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResult {
    pub name: String,
    pub title: String,
    pub kind: String,
    /// `passed`, `failed`, `skipped` or `stopped`.
    pub status: String,
    /// Why it was skipped, stopped or failed, in one line.
    pub note: String,
    pub checks: Vec<CheckOutcome>,
    /// Failed edit and write calls.
    pub failed_edits: u32,
    /// Files of a checked type that no longer parse.
    pub unparsable: Vec<String>,
    /// Failed edits plus files left not parsing.
    pub broken_edits: u32,
    pub tool_failures: u32,
    /// Commands that ran and exited with a failure (failing tests): not tool failures.
    #[serde(default)]
    pub failed_commands: u32,
    pub blocked: u32,
    pub steps: u32,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub time_ms: u64,
    pub arrival: Arrival,
    /// How the model's turn ended (`completed`, `timeLimit`, `stepCap`, …).
    pub run_status: String,
}

impl TaskResult {
    fn skipped(task: &Task, note: String) -> Self {
        Self {
            name: task.name.clone(),
            title: task.spec.title.clone(),
            kind: task.spec.kind.clone(),
            status: "skipped".into(),
            note,
            checks: Vec::new(),
            failed_edits: 0,
            unparsable: Vec::new(),
            broken_edits: 0,
            tool_failures: 0,
            failed_commands: 0,
            blocked: 0,
            steps: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            time_ms: 0,
            arrival: Arrival::None,
            run_status: String::new(),
        }
    }
}

/// The run's totals.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub tasks: u32,
    /// Tasks that ran (not skipped).
    pub ran: u32,
    pub completed: u32,
    pub skipped: u32,
    pub broken_edits: u32,
    pub tool_failures: u32,
    /// Commands that ran and exited with a failure.
    #[serde(default)]
    pub failed_commands: u32,
    pub blocked: u32,
    pub steps: u32,
    pub tokens: u64,
    pub time_ms: u64,
}

pub fn totals(results: &[TaskResult]) -> Totals {
    let mut t = Totals {
        tasks: results.len() as u32,
        ..Totals::default()
    };
    for r in results {
        match r.status.as_str() {
            "skipped" => t.skipped += 1,
            "passed" => {
                t.ran += 1;
                t.completed += 1;
            }
            _ => t.ran += 1,
        }
        t.broken_edits += r.broken_edits;
        t.tool_failures += r.tool_failures;
        t.failed_commands += r.failed_commands;
        t.blocked += r.blocked;
        t.steps += r.steps;
        t.tokens += r.prompt_tokens + r.completion_tokens;
        t.time_ms += r.time_ms;
    }
    t
}

/// A saved run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchRun {
    pub id: String,
    /// Unix ms.
    pub started_at: i64,
    pub finished_at: i64,
    /// The model, as Settings names it.
    pub model: String,
    pub model_ref: String,
    /// The request settings used (the model's tested settings, when it has them).
    pub settings: Settings,
    /// Toolchains found, by name.
    pub toolchains: Vec<String>,
    pub cancelled: bool,
    pub tasks: Vec<TaskResult>,
    pub totals: Totals,
}

/// A saved run, as the list of runs shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchSummary {
    pub id: String,
    pub started_at: i64,
    pub model: String,
    pub cancelled: bool,
    pub totals: Totals,
}

/// The prompt with `{python}` and `{node}` replaced by the programs found.
pub fn prompt_for(task: &Task, toolchains: &Toolchains) -> String {
    use super::tasks::Toolchain;
    let mut prompt = task.spec.prompt.clone();
    for (placeholder, toolchain, fallback) in [
        ("{python}", Toolchain::Python, "python3"),
        ("{node}", Toolchain::Node, "node"),
    ] {
        let command = toolchains
            .command(toolchain)
            .unwrap_or_else(|| fallback.to_owned());
        prompt = prompt.replace(placeholder, &command);
    }
    prompt
}

/// Everything a task run needs besides the task.
pub struct Setup<'a> {
    pub model: &'a Model,
    pub settings: Settings,
    pub toolchains: &'a Toolchains,
    /// The terminal the model gets (environment from the app's start-up).
    pub terminal: tools::TerminalConfig,
    pub forbidden: &'a [String],
    pub audit: AuditFn,
}

/// Runs one task in a fresh folder and verifies the result.
pub async fn run_task(setup: &Setup<'_>, task: &Task, cancel: &CancellationToken) -> TaskResult {
    if let Some(needs) = task.spec.needs
        && setup.toolchains.get(needs).is_none()
    {
        return TaskResult::skipped(task, format!("{} is not installed", needs.name()));
    }
    let ws = match Workspace::create() {
        Ok(ws) => ws,
        Err(e) => {
            return TaskResult::skipped(task, format!("The run's folder could not be made: {e}"));
        }
    };
    if let Err(e) = ws.write_files(task.files.iter().map(|(p, c)| (p.as_str(), *c))) {
        return TaskResult::skipped(task, format!("The task's files could not be written: {e}"));
    }
    let spec = Spec {
        prompt: prompt_for(task, setup.toolchains),
        terminal: Some(setup.terminal.clone()),
        settings: setup.settings,
        step_cap: STEP_CAP,
        time_limit: TIME_LIMIT,
        forbidden: setup.forbidden.to_vec(),
    };
    let record = harness::run(setup.model, spec, &ws, cancel.clone(), setup.audit.clone()).await;
    let mut result = TaskResult {
        name: task.name.clone(),
        title: task.spec.title.clone(),
        kind: task.spec.kind.clone(),
        status: String::new(),
        note: String::new(),
        checks: Vec::new(),
        failed_edits: record.failed_edits(),
        unparsable: Vec::new(),
        broken_edits: 0,
        tool_failures: record.tool_failures(),
        failed_commands: record.failed_commands(),
        blocked: record.blocked(),
        steps: record.steps,
        prompt_tokens: record.prompt_tokens,
        completion_tokens: record.completion_tokens,
        time_ms: record.time_ms,
        arrival: record.arrival(),
        run_status: record.status.clone(),
    };
    if record.status == "stopped" {
        result.status = "stopped".into();
        result.note = "Stopped before it finished.".into();
        return result;
    }
    // Verified whatever way the turn ended: a model that ran out of steps may still have
    // finished the work.
    result.checks = verify::verify(task, &ws, setup.toolchains, cancel, &setup.audit).await;
    result.unparsable = verify::unparsable(task, &ws, setup.toolchains, cancel).await;
    if cancel.is_cancelled() {
        result.status = "stopped".into();
        result.note = "Stopped while its result was being checked.".into();
        return result;
    }
    result.broken_edits = result.failed_edits + result.unparsable.len() as u32;
    let passed = result.checks.iter().all(|c| c.passed);
    result.status = if passed { "passed" } else { "failed" }.into();
    result.note = if passed {
        String::new()
    } else if let Some(failure) = record.failure.as_deref() {
        format!("The model could not answer: {failure}")
    } else {
        result
            .checks
            .iter()
            .find(|c| !c.passed)
            .map(|c| format!("{}: {}", c.check, c.detail))
            .unwrap_or_default()
    };
    result
}

/// Where a benchmark reports progress: (tasks done, tasks in all, the task now running).
pub type ProgressFn = Arc<dyn Fn(u32, u32, String) + Send + Sync>;

/// Runs `tasks` in order. When cancelled, the tasks not started yet are left out.
pub async fn run_all(
    setup: &Setup<'_>,
    tasks: &[Task],
    cancel: &CancellationToken,
    progress: ProgressFn,
) -> Vec<TaskResult> {
    let total = tasks.len() as u32;
    let mut out = Vec::new();
    for (done, task) in tasks.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }
        progress(done as u32, total, task.spec.title.clone());
        out.push(run_task(setup, task, cancel).await);
    }
    progress(out.len() as u32, total, String::new());
    out
}

/// A new run id: the local time, and four random hex digits.
pub fn new_id() -> String {
    let random = &uuid::Uuid::new_v4().simple().to_string()[..4];
    format!("{}-{random}", chrono::Local::now().format("%Y%m%d-%H%M%S"))
}

/// Whether `id` is one [`new_id`] could have made (and so a safe file name).
pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 20
        && bytes.iter().enumerate().all(|(i, b)| match i {
            8 | 15 => *b == b'-',
            0..=14 => b.is_ascii_digit(),
            _ => b.is_ascii_hexdigit() && !b.is_ascii_uppercase(),
        })
}

fn file_of(dir: &Path, id: &str) -> Option<PathBuf> {
    valid_id(id).then(|| dir.join(format!("{id}.json")))
}

/// Saves a run, and removes the oldest runs beyond [`KEEP_RUNS`].
pub fn save(dir: &Path, run: &BenchRun) -> std::io::Result<()> {
    let path = file_of(dir, &run.id)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad run id"))?;
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(run).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)?;
    let ids = ids(dir);
    if ids.len() > KEEP_RUNS {
        for old in &ids[..ids.len() - KEEP_RUNS] {
            if let Some(path) = file_of(dir, old) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    Ok(())
}

/// The ids of the saved runs, oldest first.
fn ids(dir: &Path) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    name.strip_suffix(".json")
                        .filter(|id| valid_id(id))
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

pub fn load(dir: &Path, id: &str) -> Option<BenchRun> {
    let bytes = std::fs::read(file_of(dir, id)?).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The saved runs, newest first.
pub fn list(dir: &Path) -> Vec<BenchSummary> {
    ids(dir)
        .into_iter()
        .rev()
        .filter_map(|id| load(dir, &id))
        .map(|run| BenchSummary {
            id: run.id,
            started_at: run.started_at,
            model: run.model,
            cancelled: run.cancelled,
            totals: run.totals,
        })
        .collect()
}

#[cfg(test)]
mod tests;
