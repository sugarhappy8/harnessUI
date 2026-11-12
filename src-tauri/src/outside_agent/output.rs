//! What Claude Code (`--output-format stream-json`) and Codex (`exec --json`) print, turned
//! into progress lines for the tool card and one result for the model. Everything here is
//! untrusted text from another program: it is shown and returned as data, never acted on.

use mcp_host::ToolOutput;
use serde_json::Value;

use super::{CodingApp, MAX_RUN};

/// The longest line kept from the program's output; the rest of a longer one is dropped.
const LINE_CAP: usize = 4 * 1024 * 1024;
/// Characters of a progress line.
const PROGRESS_CHARS: usize = 200;
/// Lines of the end of the error stream shown with a failure.
const STDERR_LINES: usize = 8;

/// Splits a byte stream into lines, holding at most [`LINE_CAP`] bytes of one line.
#[derive(Debug, Default)]
pub(super) struct Lines {
    buf: Vec<u8>,
    overflow: bool,
}

impl Lines {
    /// The lines completed by `bytes`. A line that grew past the cap is dropped whole.
    pub(super) fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        for &b in bytes {
            if b == b'\n' {
                if !self.overflow {
                    out.push(String::from_utf8_lossy(&self.buf).into_owned());
                }
                self.buf.clear();
                self.overflow = false;
            } else if self.buf.len() < LINE_CAP {
                self.buf.push(b);
            } else {
                self.overflow = true;
            }
        }
        out
    }

    /// The last line, when the stream did not end with a line break.
    pub(super) fn finish(&mut self) -> Option<String> {
        let line = (!self.buf.is_empty() && !self.overflow)
            .then(|| String::from_utf8_lossy(&self.buf).into_owned());
        self.buf.clear();
        line
    }
}

/// What one line said that the run needs now.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Said {
    /// A line for the tool card.
    pub(super) progress: Option<String>,
    /// The app reported that it is not signed in: the run ends at once.
    pub(super) signed_out: bool,
}

/// What the run has said so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Parsed {
    app: CodingApp,
    /// The final answer: Claude Code's `result`, Codex's last agent message.
    pub(super) answer: Option<String>,
    /// Claude Code marked its result as an error.
    pub(super) answer_is_error: bool,
    /// The last text it wrote, for "what it had so far".
    pub(super) last_text: Option<String>,
    /// Codex reported a failed turn or an error.
    pub(super) failure: Option<String>,
    pub(super) signed_out: bool,
}

/// Whether a message says the app is not signed in.
pub(super) fn looks_signed_out(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "not logged in",
        "please run /login",
        "invalid api key",
        "authentication_failed",
        "authentication_error",
        "codex login",
        "not signed in",
        "401 unauthorized",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn one_line(text: &str) -> String {
    policy::sanitize::one_line(text.trim(), PROGRESS_CHARS)
}

impl Parsed {
    pub(super) fn new(app: CodingApp) -> Self {
        Self {
            app,
            answer: None,
            answer_is_error: false,
            last_text: None,
            failure: None,
            signed_out: false,
        }
    }

    /// Reads one line of output. Lines that are not JSON events are ignored.
    pub(super) fn line(&mut self, line: &str) -> Said {
        let Ok(event) = serde_json::from_str::<Value>(line.trim()) else {
            return Said::default();
        };
        let said = match self.app {
            CodingApp::ClaudeCode => self.claude(&event),
            CodingApp::Codex => self.codex(&event),
        };
        if said.signed_out {
            self.signed_out = true;
        }
        said
    }

    /// Claude Code's `stream-json` events: `assistant` messages with text and tool calls,
    /// `system` retries, and the final `result`.
    fn claude(&mut self, event: &Value) -> Said {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "assistant" => {
                let mut lines = Vec::new();
                let blocks = event
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for block in blocks {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(text) = block.get("text").and_then(Value::as_str)
                                && !text.trim().is_empty()
                            {
                                self.last_text = Some(text.to_owned());
                                lines.push(one_line(text));
                            }
                        }
                        Some("tool_use") => {
                            let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                            let input = block.get("input");
                            let hint = ["file_path", "path", "pattern", "command"]
                                .iter()
                                .find_map(|k| input.and_then(|i| i.get(k)).and_then(Value::as_str))
                                .unwrap_or("");
                            lines.push(one_line(&format!("{name} {hint}")));
                        }
                        _ => {}
                    }
                }
                Said {
                    progress: (!lines.is_empty()).then(|| format!("{}\n", lines.join("\n"))),
                    signed_out: false,
                }
            }
            "system" => Said {
                progress: None,
                signed_out: event.get("subtype").and_then(Value::as_str) == Some("api_retry")
                    && event.get("error").and_then(Value::as_str) == Some("authentication_failed"),
            },
            "result" => {
                let text = event
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let is_error = event
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let signed_out = is_error && looks_signed_out(&text);
                self.answer = Some(text);
                self.answer_is_error = is_error;
                Said {
                    progress: None,
                    signed_out,
                }
            }
            _ => Said::default(),
        }
    }

    /// Codex's `exec --json` events: items (agent messages, commands, file changes), and a
    /// failed turn or an error.
    fn codex(&mut self, event: &Value) -> Said {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let item = event.get("item");
        let item_type = item
            .and_then(|i| i.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("");
        match (kind, item_type) {
            ("item.completed", "agent_message") => {
                let text = item
                    .and_then(|i| i.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let progress = (!text.trim().is_empty()).then(|| format!("{}\n", one_line(&text)));
                self.last_text = Some(text.clone());
                self.answer = Some(text);
                Said {
                    progress,
                    signed_out: false,
                }
            }
            ("item.started", "command_execution") => {
                let command = item
                    .and_then(|i| i.get("command"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                Said {
                    progress: Some(format!("{}\n", one_line(&format!("Runs: {command}")))),
                    signed_out: false,
                }
            }
            ("item.completed", "file_change") => Said {
                progress: Some("Changes files\n".into()),
                signed_out: false,
            },
            ("turn.failed", _) | ("error", _) => {
                let message = event
                    .pointer("/error/message")
                    .or_else(|| event.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("an error without a message")
                    .to_owned();
                let signed_out = looks_signed_out(&message);
                self.failure = Some(message);
                Said {
                    progress: None,
                    signed_out,
                }
            }
            _ => Said::default(),
        }
    }
}

/// How the run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum End {
    Exited(Option<i32>),
    WaitFailed(String),
    Stopped,
    TimedOut,
    SignedOut,
}

/// The last lines of the error stream.
fn last_lines(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    let from = lines.len().saturating_sub(STDERR_LINES);
    lines[from..].join("\n")
}

fn with_stderr(message: String, stderr: &str) -> String {
    let tail = last_lines(stderr);
    if tail.is_empty() {
        message
    } else {
        format!("{message}\n\nWhat it printed last:\n{tail}")
    }
}

/// The message for an app that is not signed in.
pub(super) fn not_signed_in(app: CodingApp) -> String {
    format!(
        "{} is not signed in. Open it once and sign in, then try again.",
        app.label()
    )
}

/// The result for the model. A long answer is shortened like any other long result, with
/// the whole text kept for `read_output`.
pub(super) fn result(
    app: CodingApp,
    end: &End,
    parsed: &Parsed,
    stderr: &str,
    budget: usize,
) -> ToolOutput {
    let label = app.label();
    let so_far = || match parsed.last_text.as_deref().or(parsed.answer.as_deref()) {
        Some(text) if !text.trim().is_empty() => {
            format!("\n\nWhat it had so far:\n{}", text.trim())
        }
        _ => String::new(),
    };
    match end {
        End::Stopped => {
            return ToolOutput::error(format!(
                "{label} was stopped before it finished.{}",
                so_far()
            ));
        }
        End::TimedOut => {
            return ToolOutput::error(format!(
                "{label} was stopped after {} minutes, the longest a task may run.{}",
                MAX_RUN.as_secs() / 60,
                so_far()
            ));
        }
        End::SignedOut => return ToolOutput::error(not_signed_in(app)),
        End::WaitFailed(e) => {
            return ToolOutput::error(format!("Could not wait for {label} to finish: {e}"));
        }
        End::Exited(_) => {}
    }
    let lower = stderr.to_lowercase();
    if let Some(answer) = &parsed.answer
        && !parsed.answer_is_error
        && parsed.failure.is_none()
        && !answer.trim().is_empty()
    {
        return match tools::shorten::head_and_tail(answer, budget, 30) {
            Some(view) => tools::shorten::shortened(view, answer.clone(), false, None),
            None => ToolOutput::text(answer.clone()),
        };
    }
    if parsed.signed_out
        || looks_signed_out(stderr)
        || parsed.failure.as_deref().is_some_and(looks_signed_out)
    {
        return ToolOutput::error(not_signed_in(app));
    }
    if [
        "unknown option",
        "unexpected argument",
        "unrecognized option",
        "unknown argument",
    ]
    .iter()
    .any(|n| lower.contains(n))
    {
        return ToolOutput::error(with_stderr(
            format!(
                "This version of {label} does not know a setting the app needs. Update {label}, then turn Coding apps off and on again in the Tools panel."
            ),
            stderr,
        ));
    }
    if app == CodingApp::Codex
        && (lower.contains("skip-git-repo-check")
            || lower.contains("not inside a trusted directory"))
    {
        return ToolOutput::error(
            "Codex works only in a folder that is a Git repository or that you trust in Codex. Choose such a working folder for this chat, or open this folder in Codex once and trust it.",
        );
    }
    if parsed.answer_is_error
        && let Some(answer) = &parsed.answer
    {
        return ToolOutput::error(with_stderr(
            format!("{label} reported a problem: {}", answer.trim()),
            stderr,
        ));
    }
    if let Some(failure) = &parsed.failure {
        return ToolOutput::error(with_stderr(
            format!("{label} reported a problem: {}", failure.trim()),
            stderr,
        ));
    }
    let code = match end {
        End::Exited(Some(code)) if *code != 0 => format!(" (exit code {code})"),
        _ => String::new(),
    };
    ToolOutput::error(with_stderr(
        format!("{label} finished without an answer{code}."),
        stderr,
    ))
}
