//! Coding assistants on a command line (0.0.5, the item "the local model may hand a task to
//! Claude Code or Codex").
//!
//! The app has its own tools for handing a task to Claude Code or Codex (`ask_claude_code`,
//! `ask_codex`), which always ask. A command line can start the same programs, and others
//! like them, without those tools: `claude -p "…" --dangerously-skip-permissions` in the
//! terminal. Such a program works online with the user's account and can change files on
//! its own, so in every approval mode a line that starts one asks with Allow once / Deny
//! (Ask first: [`crate::Risk::AlwaysAsk`]; Allow all: [`crate::KeepAsking::Hidden`]) and
//! counts as going online. [`find`] says why a line is such a line, if it is:
//!
//! - (a) the program, also after a wrapper (`env`, `timeout`, `sudo`, `cmd /c`,
//!   `Start-Process`, a shell given a command, …), is a coding assistant ([`ASSISTANTS`],
//!   with `.exe`, `.cmd` and the like);
//! - (b) such a name is the package handed to a package runner (`npx`, `npm exec`,
//!   `pnpm dlx`, `yarn dlx`, `bunx`, `uvx`, `pipx run`), as `claude`, `@openai/codex`,
//!   `@google/gemini-cli`, `aider-chat` and the like;
//! - (c) `node`, `bun` or `deno` is handed a script inside one of the assistants' npm
//!   packages ([`PACKAGES`]);
//! - (d) a token that turns off an assistant's permission checks ([`BYPASS_TOKENS`])
//!   appears anywhere on the line, quoted or not.
//!
//! A line that cannot be broken down into plain commands is read as text: the first word
//! of every command in it is checked the same way.
//!
//! Known limit: this reads names. A copy of one of these programs under another name, or
//! a link to it (`ln -s "$(which claude)" helper; ./helper -p …`), is not seen, nor is code
//! in another language that builds the name from pieces.

use crate::command::{ELEVATION, Parsed, WRAPPERS, program_name};

/// The program names of coding assistants (D8 as amended for 0.0.5 allows them here).
pub const ASSISTANTS: &[&str] = &[
    "claude",
    "claude-code",
    "codex",
    "gemini",
    "qwen",
    "aider",
    "opencode",
    "cursor-agent",
    "goose",
    "amp",
    "crush",
    "copilot",
    "q",
];

/// Tokens that turn off a coding assistant's permission checks or sandbox. Matched
/// anywhere on a line, without regard to letter case, quotes or escapes.
pub const BYPASS_TOKENS: &[&str] = &[
    "--dangerously-skip-permissions",
    "--allow-dangerously-skip-permissions",
    "bypassPermissions",
    "--dangerously-bypass-approvals-and-sandbox",
    "--yolo",
    "--full-auto",
    "danger-full-access",
    "--dangerously-bypass-hook-trust",
];

/// The npm packages of Claude Code and Codex, as they appear in the path of a script.
pub const PACKAGES: &[&str] = &["@anthropic-ai/claude-code", "@openai/codex"];

/// Package runners that take the package as their first plain argument.
const RUNNERS: &[&str] = &["npx", "pnpx", "bunx", "uvx"];

/// Package managers whose subcommand runs a package: `npm exec`, `pnpm dlx`, `pipx run`.
const RUNNER_SUBCOMMANDS: &[(&str, &[&str])] = &[
    ("npm", &["exec", "x"]),
    ("pnpm", &["dlx", "exec"]),
    ("yarn", &["dlx", "exec"]),
    ("bun", &["x"]),
    ("pipx", &["run"]),
    ("uv", &["tool"]),
];

/// Programs that run a JavaScript file they are handed.
const SCRIPT_HOSTS: &[&str] = &["node", "nodejs", "bun", "deno"];

/// Shells: what they are handed is a command line of its own.
const SHELLS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "ash",
    "fish",
    "csh",
    "tcsh",
    "pwsh",
    "powershell",
    "cmd",
];

/// Words of a package name that only say which kind of package it is: `gemini-cli`,
/// `qwen-code`, `opencode-ai`, `aider-chat`.
const PACKAGE_SUFFIXES: &[&str] = &["-cli", "-code", "-ai", "-chat", "-agent"];

/// Why `line` starts a coding assistant or turns off one's permission checks, if it does.
/// `parsed` is what the classifier made of the line with its shell's grammar (for
/// PowerShell, the lenient reading that Allow all uses).
pub fn find(line: &str, parsed: &Parsed) -> Option<String> {
    if let Some(token) = bypass_token(line) {
        return Some(format!(
            "the command contains {token}, which turns off a coding assistant's permission checks"
        ));
    }
    match parsed {
        Parsed::Plain(commands) => commands
            .iter()
            .find_map(|c| command(&c.program, &c.args, 0)),
        Parsed::Complex { .. } => in_text(line, 0),
    }
}

/// The first bypass token on the line, as listed.
pub fn bypass_token(line: &str) -> Option<&'static str> {
    let lower = line.to_lowercase();
    let unquoted: String = lower
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\' | '`' | '^'))
        .collect();
    BYPASS_TOKENS.iter().copied().find(|token| {
        let token = token.to_lowercase();
        lower.contains(&token) || unquoted.contains(&token)
    })
}

/// Whether a word, as a program, is a coding assistant.
fn is_assistant(word: &str) -> bool {
    let word = word.trim_matches(|c| matches!(c, '\'' | '"' | '`'));
    !word.is_empty() && ASSISTANTS.contains(&program_name(word).as_str())
}

/// Whether a word holds the path of a script inside an assistant's npm package.
fn names_package_path(word: &str) -> bool {
    let w = word.to_lowercase().replace('\\', "/");
    PACKAGES.iter().any(|p| w.contains(p))
}

/// Whether a package as a runner takes it (`@scope/name@1.2`, `name@latest`,
/// `name==1.0`) is an assistant's.
fn names_assistant_package(spec: &str) -> bool {
    let spec = spec
        .trim_matches(|c| matches!(c, '\'' | '"' | '`'))
        .to_lowercase()
        .replace('\\', "/");
    if spec.is_empty() || spec.starts_with('-') {
        return false;
    }
    if names_package_path(&spec) {
        return true;
    }
    // The version after the name: `@scope/name@1.2`, `name@latest`.
    let without_version = match spec.strip_prefix('@') {
        Some(rest) => rest.split('@').next().unwrap_or(rest),
        None => spec.split('@').next().unwrap_or(&spec),
    };
    // Python's version specifiers: `aider-chat==0.80`, `aider-chat[extra]`.
    let without_version = without_version
        .split(['=', '<', '>', '~', '!', '[', ';'])
        .next()
        .unwrap_or(without_version);
    let name = without_version.rsplit('/').next().unwrap_or(without_version);
    if ASSISTANTS.contains(&name) || ASSISTANTS.contains(&program_name(name).as_str()) {
        return true;
    }
    PACKAGE_SUFFIXES
        .iter()
        .any(|suffix| name.strip_suffix(suffix).is_some_and(|stem| ASSISTANTS.contains(&stem)))
}

/// The words of a command's arguments: each argument, the words of an argument that holds
/// several, and both sides of `--flag=value`.
fn words(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    for arg in args {
        out.push(arg.as_str());
        if arg.contains(char::is_whitespace) {
            out.extend(arg.split_whitespace());
        }
        if let Some((_, value)) = arg.split_once('=') {
            out.push(value);
        }
    }
    out
}

/// Programs that run another program they are handed, for this rule.
fn is_wrapper(name: &str) -> bool {
    WRAPPERS.contains(&name)
        || crate::allow_all::MORE_WRAPPERS.contains(&name)
        || ELEVATION.contains(&name)
}

/// How many programs inside programs are followed (`sudo env timeout 9 npx claude`).
const MAX_DEPTH: usize = 8;

/// Why one simple command starts a coding assistant, if it does.
fn command(program: &str, args: &[String], depth: usize) -> Option<String> {
    if depth > MAX_DEPTH {
        return None;
    }
    let name = program_name(program.trim_matches(|c| matches!(c, '\'' | '"' | '`')));
    let name = name.as_str();
    // (a) The program itself.
    if ASSISTANTS.contains(&name) {
        return Some(format!(
            "{program} is a coding assistant: it works online with your account and can change files"
        ));
    }
    // (c) A script inside an assistant's package.
    if SCRIPT_HOSTS.contains(&name) && args.iter().any(|a| names_package_path(a)) {
        return Some(format!("{program} is handed a script of a coding assistant"));
    }
    // (b) A package runner handed an assistant's package.
    if let Some(rest) = runner_args(name, args)
        && let Some(found) = words(rest).into_iter().find(|w| names_assistant_package(w))
    {
        return Some(format!(
            "{program} runs {found}, a coding assistant that works online with your account"
        ));
    }
    // (a) After a wrapper, or inside a command handed to a shell.
    let wraps = is_wrapper(name);
    if !wraps && !SHELLS.contains(&name) {
        return None;
    }
    if wraps
        && let Some(found) = words(args)
            .into_iter()
            .find(|w| is_assistant(w) || names_package_path(w))
    {
        return Some(format!(
            "{program} runs {found}, a coding assistant that works online with your account"
        ));
    }
    // A command handed over as one argument (`sh -c "…"`, `cmd /c "…"`), read on its own.
    for arg in args.iter().filter(|a| a.contains(char::is_whitespace)) {
        if let Some(why) = in_text(arg, depth + 1) {
            return Some(why);
        }
    }
    // The first program among the arguments that can run another one: a runner, a script
    // host, a shell or another wrapper (`env npx …`, `timeout 9 node …`).
    let at = args.iter().position(|a| {
        let n = program_name(a);
        RUNNERS.contains(&n.as_str())
            || RUNNER_SUBCOMMANDS.iter().any(|(m, _)| *m == n)
            || SCRIPT_HOSTS.contains(&n.as_str())
            || SHELLS.contains(&n.as_str())
            || is_wrapper(&n)
    })?;
    command(&args[at], &args[at + 1..], depth + 1)
}

/// What a package runner is handed, if `name` is one: `npx …`, or what follows the
/// subcommand of `npm exec …`, `pnpm dlx …`, `pipx run …`.
fn runner_args<'a>(name: &str, args: &'a [String]) -> Option<&'a [String]> {
    if RUNNERS.contains(&name) {
        return Some(args);
    }
    let (_, subs) = RUNNER_SUBCOMMANDS.iter().find(|(manager, _)| *manager == name)?;
    let at = args.iter().position(|a| !a.starts_with('-'))?;
    subs.contains(&args[at].to_lowercase().as_str())
        .then(|| &args[at + 1..])
}

/// For a line that cannot be broken down: the first word of every command in it, read as
/// a program (also as written with quotes and escapes taken away), and a script inside an
/// assistant's package anywhere in it.
fn in_text(line: &str, depth: usize) -> Option<String> {
    if depth > MAX_DEPTH {
        return None;
    }
    if names_package_path(line) {
        return Some("the command runs a script of a coding assistant".into());
    }
    let lower = line.to_lowercase();
    let unquoted: String = lower
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '`' | '^'))
        .collect();
    for text in [lower.as_str(), unquoted.as_str()] {
        for segment in text.split([';', '\n', '(', ')', '{', '}', '|', '&', '`']) {
            let mut words: Vec<String> = segment.split_whitespace().map(str::to_owned).collect();
            // Keywords and settings in front of a program are not its name.
            while let Some(first) = words.first() {
                let keyword = matches!(
                    first.as_str(),
                    "then" | "do" | "else" | "elif" | "if" | "while" | "until" | "!" | "." | "time"
                );
                let setting = first.split_once('=').is_some_and(|(name, _)| {
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == ':')
                });
                if keyword || setting {
                    words.remove(0);
                } else {
                    break;
                }
            }
            if let Some((program, args)) = words.split_first()
                && let Some(why) = command(program, args, depth + 1)
            {
                return Some(why);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests;
