//! Verification of a benchmark task, run by the app after the model finished.
//!
//! Nothing the model says decides a result. Checks are shipped data ([`super::tasks`]):
//! built-in content checks read the run's folder. A script check copies the task's script
//! into a separate folder the model never saw and runs it with a fixed command through
//! `runtime::spawn` (the one way the app starts processes), with a time limit, the run's
//! folder as working folder, and the run's cancellation. The script only loads the
//! project's code and prints its raw results for fixed inputs as one line `RESULT <json>`;
//! the app (which never loads the project's code) compares those values with the ones it
//! expects. Code that tampers with its own process (patches an assertion, prints a fake
//! success line) cannot change what the app compares.
//!
//! The script does run code the model wrote, with the user's rights: it is not confined.
//! Each script run is written to the audit log under the run's row.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use super::harness::AuditFn;
use super::tasks::{Check, Task, Toolchain};
use super::workspace::Workspace;

/// How long a verification script may run.
const SCRIPT_TIME: Duration = Duration::from_secs(60);
/// How long a parse check or a version check may run.
const SHORT_TIME: Duration = Duration::from_secs(20);
/// How long the app waits for a program's output once it has ended (a process it left
/// behind in another session may hold the pipes open).
const DRAIN_TIME: Duration = Duration::from_secs(2);
/// Parse checks per run, at most.
const MAX_PARSE_CHECKS: usize = 50;

/// The toolchains found on this computer.
#[derive(Debug, Clone, Default)]
pub struct Toolchains {
    found: HashMap<Toolchain, PathBuf>,
    /// The `PATH` commands see (the captured login environment on macOS).
    path: Option<String>,
}

impl Toolchains {
    /// Looks for every toolchain in `path` (the app's own `PATH` when `None`). A program
    /// counts as found only when `<program> --version` runs and succeeds: on Windows,
    /// `python3` can be a stand-in that only points to the Store.
    pub async fn detect(path: Option<String>) -> Self {
        let mut found = HashMap::new();
        for toolchain in [Toolchain::Python, Toolchain::Node] {
            for name in toolchain.programs() {
                let Some(program) =
                    runtime::resolve_program(name, path.as_deref().map(std::ffi::OsStr::new))
                else {
                    continue;
                };
                let cwd = std::env::temp_dir();
                let ran = run_program(
                    &program,
                    &["--version"],
                    &cwd,
                    None,
                    SHORT_TIME,
                    &path,
                    &CancellationToken::new(),
                )
                .await;
                if ran.is_ok_and(|r| r.code == Some(0)) {
                    found.insert(toolchain, program);
                    break;
                }
            }
        }
        Self { found, path }
    }

    pub fn get(&self, toolchain: Toolchain) -> Option<&Path> {
        self.found.get(&toolchain).map(PathBuf::as_path)
    }

    /// The command a prompt names, such as `python3`.
    pub fn command(&self, toolchain: Toolchain) -> Option<String> {
        let path = self.get(toolchain)?;
        Some(path.file_stem()?.to_string_lossy().into_owned())
    }
}

/// One check's outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckOutcome {
    /// The check in words.
    pub check: String,
    pub passed: bool,
    /// Why it failed (or empty).
    pub detail: String,
}

fn lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// Runs every check of `task` against the run's folder. Each script run gets an audit
/// row through `audit`. Ends promptly when `cancel` fires (the remaining checks fail as
/// not run).
pub async fn verify(
    task: &Task,
    ws: &Workspace,
    toolchains: &Toolchains,
    cancel: &CancellationToken,
    audit: &AuditFn,
) -> Vec<CheckOutcome> {
    let mut out = Vec::new();
    for check in &task.spec.checks {
        out.push(match check {
            Check::Run { script, expect } => {
                run_script(task, script, expect, ws, toolchains, cancel, audit).await
            }
            other => builtin(task, other, ws),
        });
    }
    out
}

fn outcome(check: String, result: Result<(), String>) -> CheckOutcome {
    match result {
        Ok(()) => CheckOutcome {
            check,
            passed: true,
            detail: String::new(),
        },
        Err(detail) => CheckOutcome {
            check,
            passed: false,
            detail,
        },
    }
}

/// A built-in content check.
pub fn builtin(task: &Task, check: &Check, ws: &Workspace) -> CheckOutcome {
    let read = |file: &str| {
        ws.read(file)
            .map(|t| lf(&t))
            .ok_or(format!("{file} is missing or not text"))
    };
    match check {
        Check::Run { script, .. } => outcome(
            format!("{script} gives the expected results"),
            Err("a script check is not a built-in check".into()),
        ),
        Check::Json {
            file,
            pointer,
            equals,
        } => outcome(
            format!("{file} {pointer} is {equals}"),
            (|| {
                let text = read(file)?;
                let value: Value = serde_json::from_str(&text)
                    .map_err(|e| format!("{file} is not valid JSON: {e}"))?;
                match value.pointer(pointer) {
                    Some(found) if found == equals => Ok(()),
                    Some(found) => Err(format!("it is {found}")),
                    None => Err("it is missing".into()),
                }
            })(),
        ),
        Check::Ini {
            file,
            section,
            key,
            equals,
        } => outcome(
            format!("{file} [{section}] {key} is {equals}"),
            (|| {
                let text = read(file)?;
                match ini_value(&text, section, key) {
                    Some(found) if found == *equals => Ok(()),
                    Some(found) => Err(format!("it is {found}")),
                    None => Err("it is missing".into()),
                }
            })(),
        ),
        Check::Order { file, texts } => outcome(
            format!("{file} has its parts in order"),
            (|| {
                let text = read(file)?;
                let mut from = 0;
                for part in texts {
                    match text[from..].find(part.as_str()) {
                        Some(at) => from += at + part.len(),
                        None => {
                            return Err(if text.contains(part.as_str()) {
                                format!("\"{part}\" is in the wrong place")
                            } else {
                                format!("\"{part}\" is missing")
                            });
                        }
                    }
                }
                Ok(())
            })(),
        ),
        Check::Absent { text, ext } => outcome(format!("no .{ext} file contains {text}"), {
            let hits: Vec<String> = ws
                .files()
                .into_iter()
                .filter(|f| f.ends_with(&format!(".{ext}")))
                .filter(|f| ws.read(f).is_some_and(|t| t.contains(text.as_str())))
                .collect();
            if hits.is_empty() {
                Ok(())
            } else {
                Err(format!("still in {}", hits.join(", ")))
            }
        }),
        Check::Unchanged { file } => outcome(format!("{file} is unchanged"), {
            let shipped = task.fixture(file).map(|c| lf(&String::from_utf8_lossy(c)));
            match (ws.read(file).map(|t| lf(&t)), shipped) {
                (Some(now), Some(before)) if now == before => Ok(()),
                (None, _) => Err(format!("{file} is missing")),
                _ => Err(format!("{file} was changed")),
            }
        }),
    }
}

/// The value of `key` in `[section]`: the last assignment wins, comments (`;`, `#`) and
/// blank lines are skipped, names compare case-insensitively.
pub fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut current = String::new();
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.trim().to_ascii_lowercase();
            continue;
        }
        if current != section.to_ascii_lowercase() {
            continue;
        }
        if let Some((k, v)) = line.split_once('=')
            && k.trim().eq_ignore_ascii_case(key)
        {
            found = Some(v.trim().to_owned());
        }
    }
    found
}

/// Whether two JSON values are the same, numbers compared as numbers (`4.0` is `4`).
pub fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0),
            _ => x == y,
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// The first result that differs from what is expected, in words.
fn first_difference(found: &Value, expect: &Value) -> String {
    if let (Value::Object(found), Value::Object(expect)) = (found, expect) {
        for (key, wanted) in expect {
            match found.get(key) {
                None => return format!("{key} is missing"),
                Some(got) if !same(got, wanted) => {
                    return policy::sanitize::one_line(
                        &format!("{key} is {got}, expected {wanted}"),
                        300,
                    );
                }
                _ => {}
            }
        }
        if let Some(key) = found.keys().find(|k| !expect.contains_key(*k)) {
            return policy::sanitize::one_line(&format!("unexpected result {key}"), 300);
        }
    }
    policy::sanitize::one_line(&format!("the results are {found}"), 300)
}

/// Runs a task's script, from a folder of its own, on the run's folder, and compares the
/// results it prints with the expected ones.
async fn run_script(
    task: &Task,
    script: &str,
    expect: &Value,
    ws: &Workspace,
    toolchains: &Toolchains,
    cancel: &CancellationToken,
    audit: &AuditFn,
) -> CheckOutcome {
    let check = format!("{script} gives the expected results");
    let Some(toolchain) = task.spec.needs else {
        return outcome(check, Err("the task names no toolchain".into()));
    };
    let Some(program) = toolchains.get(toolchain) else {
        return outcome(check, Err(format!("{} is not installed", toolchain.name())));
    };
    if cancel.is_cancelled() {
        return outcome(check, Err("not run: the run was stopped".into()));
    }
    let private = match Workspace::create_private() {
        Ok(folder) => folder,
        Err(e) => {
            return outcome(
                check,
                Err(format!("could not make the check's folder: {e}")),
            );
        }
    };
    if let Err(e) = private.write_files(task.verify.iter().map(|(p, c)| (p.as_str(), *c))) {
        return outcome(check, Err(format!("could not write the check: {e}")));
    }
    let script_path = match private.inside(script) {
        Ok(path) => path.display().to_string(),
        Err(e) => return outcome(check, Err(e.to_string())),
    };
    let mut args: Vec<&str> = Vec::new();
    if toolchain == Toolchain::Python {
        // Isolated (the run's folder is not on the import path unless the script puts it
        // there itself) and without bytecode files.
        args.extend(["-I", "-B"]);
    }
    args.push(&script_path);
    let started = std::time::Instant::now();
    let ran = run_program(
        program,
        &args,
        ws.path(),
        None,
        SCRIPT_TIME,
        &toolchains.path,
        cancel,
    )
    .await;
    drop(private);
    let result = match ran {
        Err(e) => Err(format!("could not run it: {e}")),
        Ok(r) if r.cancelled => Err("not finished: the run was stopped".into()),
        Ok(r) if r.timed_out => Err(format!(
            "it did not finish within {} seconds",
            SCRIPT_TIME.as_secs()
        )),
        Ok(r) => {
            let printed = r
                .stdout
                .lines()
                .rev()
                .find_map(|l| l.trim().strip_prefix("RESULT "))
                .map(|json| serde_json::from_str::<Value>(json.trim()));
            match printed {
                Some(Ok(found)) if same(&found, expect) => Ok(()),
                Some(Ok(found)) => Err(first_difference(&found, expect)),
                Some(Err(_)) | None => Err(tail(&r.stderr, &r.stdout)),
            }
        }
    };
    let command = format!(
        "{} {}",
        program.display(),
        args.iter()
            .map(|a| if a.contains(' ') {
                format!("\"{a}\"")
            } else {
                (*a).to_owned()
            })
            .collect::<Vec<_>>()
            .join(" ")
    );
    audit(store::AuditEntry {
        ts: None,
        chat_id: None,
        server: "evals".into(),
        tool: "verify".into(),
        arguments: serde_json::json!({"task": task.name, "command": command, "cwd": ws.display()}),
        preview: format!(
            "Runs the check of {} on the model's code: {command}",
            task.name
        ),
        risk: "normal".into(),
        decision: store::AuditDecision::AllowedWithoutPrompt,
        decided_by: "app".into(),
        note: None,
        parent_audit_id: None,
        result: Some(store::AuditResult {
            status: if result.is_ok() { "ok" } else { "failed" }.into(),
            summary: result.as_ref().err().cloned(),
            duration_ms: Some(started.elapsed().as_millis() as u64),
        }),
    });
    outcome(check, result)
}

/// The last lines of what a failed check printed.
fn tail(stderr: &str, stdout: &str) -> String {
    let text = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(4);
    let joined = lines[start..].join(" | ");
    if joined.is_empty() {
        "the check failed".into()
    } else {
        policy::sanitize::one_line(&joined, 300)
    }
}

/// The files of the types the task names that no longer parse (a broken edit that a
/// parser can see). Types whose parser is not installed are not checked.
pub async fn unparsable(
    task: &Task,
    ws: &Workspace,
    toolchains: &Toolchains,
    cancel: &CancellationToken,
) -> Vec<String> {
    let mut broken = Vec::new();
    let files: Vec<String> = ws
        .files()
        .into_iter()
        .filter(|f| {
            task.spec
                .parse
                .iter()
                .any(|ext| f.ends_with(&format!(".{ext}")))
        })
        .take(MAX_PARSE_CHECKS)
        .collect();
    for file in files {
        if cancel.is_cancelled() {
            break;
        }
        let Ok(path) = ws.inside(&file) else {
            continue;
        };
        let path_text = path.display().to_string();
        let parses = if file.ends_with(".json") {
            ws.read(&file)
                .is_some_and(|t| serde_json::from_str::<Value>(&t).is_ok())
        } else {
            let (toolchain, args): (Toolchain, Vec<&str>) = if file.ends_with(".py") {
                (
                    Toolchain::Python,
                    vec![
                        "-I",
                        "-B",
                        "-c",
                        "import ast, sys; ast.parse(open(sys.argv[1], 'rb').read())",
                        &path_text,
                    ],
                )
            } else if file.ends_with(".js") {
                (Toolchain::Node, vec!["--check", &path_text])
            } else {
                continue;
            };
            let Some(program) = toolchains.get(toolchain) else {
                continue;
            };
            match run_program(
                program,
                &args,
                ws.path(),
                None,
                SHORT_TIME,
                &toolchains.path,
                cancel,
            )
            .await
            {
                Ok(r) if r.cancelled => break,
                Ok(r) => r.code == Some(0),
                Err(_) => continue,
            }
        };
        if !parses {
            broken.push(file);
        }
    }
    broken
}

/// What a program did.
#[derive(Debug, Default)]
pub struct Ran {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    /// The run was stopped while the program ran; it was killed.
    pub cancelled: bool,
}

/// Output kept per stream.
const MAX_OUTPUT: usize = 64 * 1024;

/// Runs a fixed program through `runtime::spawn`, with a minimal environment and `stdin`
/// (if any) written and closed. Its process group is killed when it ends, when the time
/// limit passes and when `cancel` fires. Its output is read for at most [`DRAIN_TIME`]
/// after that, so a process that left the group and holds the pipes cannot keep the app
/// waiting.
pub async fn run_program(
    program: &Path,
    args: &[&str],
    cwd: &Path,
    stdin: Option<String>,
    limit: Duration,
    path: &Option<String>,
    cancel: &CancellationToken,
) -> std::io::Result<Ran> {
    let mut env = runtime::EnvSpec::minimal()
        .set("PYTHONDONTWRITEBYTECODE", "1")
        .set("PYTHONIOENCODING", "utf-8")
        .set("NO_COLOR", "1");
    if let Some(path) = path {
        env = env.set("PATH", path.clone());
    }
    let spec = runtime::SpawnSpec::new(program, cwd)
        .args(args.iter().copied())
        .env(env)
        .stdin(if stdin.is_some() {
            runtime::StdioMode::Piped
        } else {
            runtime::StdioMode::Null
        })
        .stdout(runtime::StdioMode::Piped)
        .stderr(runtime::StdioMode::Piped);
    let mut child = runtime::spawn(spec)?;
    if let (Some(text), Some(mut input)) = (stdin, child.take_stdin()) {
        let _ = input.write_all(text.as_bytes()).await;
        drop(input);
    }
    let read = |stream: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>| async move {
        let mut out = Vec::new();
        if let Some(mut stream) = stream {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = stream.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                if out.len() < MAX_OUTPUT {
                    out.extend_from_slice(&chunk[..n]);
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    let stdout =
        tokio::spawn(read(child.take_stdout().map(|s| {
            Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>
        })));
    let stderr =
        tokio::spawn(read(child.take_stderr().map(|s| {
            Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>
        })));
    let mut ran = Ran::default();
    tokio::select! {
        status = tokio::time::timeout(limit, child.wait()) => match status {
            Ok(status) => ran.code = status?.code(),
            Err(_) => ran.timed_out = true,
        },
        _ = cancel.cancelled() => ran.cancelled = true,
    }
    // The whole group, also after a normal exit: nothing it started stays behind in it.
    let _ = child.start_kill();
    drop(child);
    let drain = |task: tokio::task::JoinHandle<String>| async move {
        let abort = task.abort_handle();
        match tokio::time::timeout(DRAIN_TIME, task).await {
            Ok(text) => text.unwrap_or_default(),
            Err(_) => {
                abort.abort();
                String::new()
            }
        }
    };
    ran.stdout = drain(stdout).await;
    ran.stderr = drain(stderr).await;
    Ok(ran)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn python() -> Option<PathBuf> {
        runtime::resolve_program("python3", None)
    }

    /// A program whose child keeps the output pipe open from another session does not
    /// keep the app waiting once the program has ended.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_child_left_holding_the_pipe_does_not_hang_the_check() {
        let Some(python) = python() else {
            eprintln!("python3 is missing; skipped");
            return;
        };
        let started = std::time::Instant::now();
        let ran = run_program(
            &python,
            &[
                "-c",
                "import subprocess; subprocess.Popen(['sleep', '40'], start_new_session=True); print('done', flush=True)",
            ],
            &std::env::temp_dir(),
            None,
            Duration::from_secs(30),
            &None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(6),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(ran.code, Some(0));
        assert!(!ran.timed_out && !ran.cancelled);
    }

    /// Stop during a check ends it within about two seconds.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_ends_a_running_check() {
        let Some(python) = python() else {
            eprintln!("python3 is missing; skipped");
            return;
        };
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            stopper.cancel();
        });
        let started = std::time::Instant::now();
        let ran = run_program(
            &python,
            &["-c", "import time; time.sleep(30)"],
            &std::env::temp_dir(),
            None,
            Duration::from_secs(60),
            &None,
            &cancel,
        )
        .await
        .unwrap();
        assert!(ran.cancelled);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn results_compare_as_values() {
        use serde_json::json;
        assert!(same(
            &json!({"a": {"value": 4.0}}),
            &json!({"a": {"value": 4}})
        ));
        assert!(same(&json!([22.950000000000003]), &json!([22.95])));
        assert!(!same(&json!({"a": 1}), &json!({"a": 1, "b": 2})));
        assert!(!same(&json!({"a": 1, "b": 2}), &json!({"a": 1})));
        assert!(!same(&json!("4"), &json!(4)));
        assert!(!same(&json!([1, 2]), &json!([2, 1])));
        assert_eq!(
            first_difference(&json!({"a": 3}), &json!({"a": 4})),
            "a is 3, expected 4"
        );
    }

    #[test]
    fn ini_values_are_read_per_section() {
        let text = "; c\n[database]\nhost = localhost\ntimeout=5\n\n[Cache]\n# c\ntimeout = 9\ntimeout = 10\n";
        assert_eq!(ini_value(text, "database", "timeout").as_deref(), Some("5"));
        assert_eq!(ini_value(text, "cache", "TIMEOUT").as_deref(), Some("10"));
        assert_eq!(ini_value(text, "database", "missing"), None);
        assert_eq!(ini_value(text, "nowhere", "host"), None);
    }
}
