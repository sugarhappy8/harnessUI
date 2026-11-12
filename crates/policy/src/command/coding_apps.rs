//! Coding assistants on a command line (0.0.6, the item "the local model may hand a task to
//! Claude Code or Codex").
//!
//! The app has its own tools for handing a task to Claude Code or Codex (`ask_claude_code`,
//! `ask_codex`), which always ask. A command line can start the same programs, and others
//! like them, without those tools: `claude -p "…" --dangerously-skip-permissions` in the
//! terminal. Such a program works online with the user's account and can change files on
//! its own, so a line that starts one asks with Allow once / Deny (Ask first:
//! [`crate::Risk::AlwaysAsk`]; Allow all: [`crate::KeepAsking::Hidden`]) and counts as
//! going online. In Full access nothing asks (D12): the user chose that the terminal runs
//! everything there. [`find`] says why a line is such a line, if it is:
//!
//! - (a) the program, also after a wrapper (`env`, `timeout`, `sudo`, `cmd /c`,
//!   `Start-Process`, `taskset`, `tmux`, `firejail`, `cross-env`, `valgrind`, a shell given
//!   a command, …), after a word of any program that runs what follows (`uv run`,
//!   `bundle exec`, `kubectl exec pod --`, `docker run img`), after `find -exec` or
//!   `fd -x`, inside code handed to an interpreter (`python3 -c "os.system('claude …')"`),
//!   fed to a shell on its standard input (`echo 'claude …' | sh`), or as a locator finds
//!   it (`$(which claude)`, `(Get-Command claude).Source`), is a coding assistant
//!   ([`ASSISTANTS`], with `.exe`, `.cmd` and the like, also written as a pattern the
//!   shell expands: `cl?ude`, `clau*`, `{claude,}`);
//! - (b) such a name is the package handed to a package runner (`npx`, `npm exec`,
//!   `pnpm dlx`, `yarn dlx`, `bunx`, `uvx`, `pipx run`, `nix run`), as `claude`,
//!   `@openai/codex`, `@google/gemini-cli`, `aider-chat`, `nixpkgs#claude-code` and the
//!   like, or the program a package manager is handed by name (`yarn claude`,
//!   `pnpm codex`, `bun run claude`);
//! - (c) `node`, `bun` or `deno` is handed a script inside one of the assistants' npm
//!   packages ([`PACKAGES`]);
//! - (d) a token that turns off an assistant's permission checks ([`BYPASS_TOKENS`])
//!   appears anywhere on the line, quoted or not, also split over two lines.
//!
//! A line that cannot be broken down into plain commands is read as text: the first word
//! of every command in it is checked the same way, as written, with its quotes taken away,
//! and with its escapes taken away too (the POSIX shell runs `cod\ex` as `codex`), each
//! with two lines joined by a backslash read as one and with braces expanded. Reading a
//! line many ways has a limit of work; a line beyond it counts as starting an assistant.
//! A locator's command longer than [`MAX_LOCATOR_SPAN`] is not read.
//!
//! Known limits: this reads names. A copy of one of these programs under another name, or
//! a link to it (`ln -s "$(which claude)" helper; ./helper -p …`), is not seen, nor is code
//! in a script file or code that builds the name from pieces (`"cl" + "aude"`,
//! `$'\x63laude'`, `c=cl; ${c}aude`), a name kept in a file or passed in from outside
//! the line, a program named by a setting
//! (`git -c core.pager=claude`), a project's own script or task (`make claude`,
//! `npm run claude`), a program that runs on another computer (`ssh host claude`), or a
//! program not listed here that runs the next word it is given without a word such as
//! `run`, `exec` or `--` before it.

use std::cell::Cell;

use crate::command::{ELEVATION, INLINE, Parsed, READ_ONLY, WRAPPERS, join_lines, program_name};

/// The program names of coding assistants (D8 as amended for 0.0.6 allows them here).
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

/// Programs that change how another program runs (processor, priority, limits, terminal,
/// sandbox, namespace, tracing, clock, environment, network, debugger, job queue, file
/// watcher) and then run it, beyond the classifier's wrapper lists. Only this rule follows
/// them: any word among their arguments that names an assistant counts.
const PROGRAM_RUNNERS: &[&str] = &[
    "taskset",
    "numactl",
    "chrt",
    "prlimit",
    "unbuffer",
    "rlwrap",
    "tmux",
    "screen",
    "firejail",
    "bwrap",
    "systemd-run",
    "arch",
    "dbus-run-session",
    "xvfb-run",
    "unshare",
    "nsenter",
    "runuser",
    "sg",
    "strace",
    "ltrace",
    "faketime",
    "corepack",
    // Review of 0.0.6, round 2.
    "valgrind",
    "torsocks",
    "tsocks",
    "proxychains",
    "proxychains4",
    "fakeroot",
    "fakechroot",
    "proot",
    "ssh-agent",
    "dbus-launch",
    "setarch",
    "linux32",
    "linux64",
    "chronic",
    "ifne",
    "cpulimit",
    "entr",
    "lckdo",
    "vglrun",
    "prime-run",
    "optirun",
    "primusrun",
    "gamemoderun",
    "nq",
    "ts",
    "tsp",
    "pueue",
    "expect",
    "hyperfine",
    "sandbox-exec",
    "conhost",
    "wt",
    "cross-env",
    "cross-env-shell",
    "dotenv",
    "dotenvx",
    "env-cmd",
    "nodemon",
    "concurrently",
    "watchexec",
    "nix-shell",
    "gosu",
    "su-exec",
    "setpriv",
    "chpst",
    "tini",
    "dumb-init",
    "systemd-nspawn",
    "systemd-inhibit",
    "schroot",
    "perf",
    "rr",
    "heaptrack",
    "xterm",
    "uxterm",
    "gnome-terminal",
    "konsole",
    "kitty",
    "alacritty",
    "wezterm",
    "foot",
    "terminator",
    "tilix",
    "x-terminal-emulator",
    "winpty",
    "mintty",
    "dtach",
    "abduco",
    "zellij",
    "byobu",
    "steam-run",
    "flatpak-spawn",
];

/// Words of a program's own after which it runs the program that follows (`uv run`,
/// `bundle exec`, `kubectl exec`, `distrobox enter`, `nix develop`, `wt nt`), for any
/// program: they count among its first three plain words.
const RUN_WORDS: &[&str] = &[
    "run",
    "exec",
    "x",
    "dlx",
    "enter",
    "shell",
    "develop",
    "spawn",
    "launch",
    "start",
    "nt",
    "new-tab",
    "sp",
    "split-pane",
    "new-window",
    "neww",
    "split-window",
    "splitw",
    "new-session",
];

/// Options after which any program runs what follows (`kubectl exec pod -- claude`,
/// `nodemon --exec '…'`, `gdb --args claude`).
const LONG_RUN_FLAGS: &[&str] = &[
    "--",
    "--command",
    "--cmd",
    "--run",
    "--exec",
    "--execute",
    "--args",
];

/// Short options that hand a program a command (`nix develop -c claude`, `vagrant ssh -c
/// '…'`, `xterm -e claude`): only the next word counts, since these letters mean other
/// things to other programs.
const SHORT_RUN_FLAGS: &[&str] = &["-c", "-e", "-x"];

/// Long options known to take the next word as their value whatever the program, a label
/// and not a program (`--model claude`, `--name codex`): after a run word, that word is
/// not read as a program. Any other long option may take no value
/// (`uv run --frozen claude`) or may name the assistant a harness runs
/// (`tb run --agent claude-code`), so the word after it is read.
const LONG_VALUE_FLAGS: &[&str] = &["--model", "--name"];

/// Programs whose `run` or `--` means something else: npm runs a project's scripts (its
/// programs run through `npm exec`, rule (b)), and git's `--` comes before file names.
const OWN_RUN_WORDS: &[&str] = &["npm", "git"];

/// Programs that run the commands they read on standard input (`at now <<< '…'`), besides
/// shells.
const INPUT_RUNNERS: &[&str] = &["at", "batch"];

/// Programs that hand each file they find to a program named after a flag
/// (`find . -exec claude …`, `fd -x claude …`).
const PROGRAM_FLAGS: &[(&str, &[&str])] = &[
    ("find", &["-exec", "-execdir", "-ok", "-okdir"]),
    ("gfind", &["-exec", "-execdir", "-ok", "-okdir"]),
    ("fd", &["-x", "--exec", "-X", "--exec-batch"]),
    ("fdfind", &["-x", "--exec", "-X", "--exec-batch"]),
];

/// Package managers that run a program of the project's packages handed by name
/// (`yarn claude`, `pnpm codex`, `bun run claude`).
const BIN_RUNNERS: &[&str] = &["yarn", "pnpm", "bun"];

/// Their options that take the next word as a value.
const BIN_RUNNER_VALUE_FLAGS: &[&str] = &[
    "--cwd",
    "--dir",
    "-C",
    "--filter",
    "-F",
    "--workspace",
    "-w",
    "--prefix",
    "--registry",
    "--config",
];

/// Programs that print where a program is, or its name (`$(which claude) -p …`,
/// `$(echo claude) -p …`).
const LOCATORS: &[&str] = &[
    "which",
    "where",
    "whence",
    "type",
    "command",
    "get-command",
    "gcm",
    "echo",
    "printf",
    "write-output",
    // Programs that print paths they find (`$(ls ~/.local/bin/cl*)`,
    // `& (Join-Path $env:APPDATA npm\claude.cmd)`).
    "ls",
    "dir",
    "find",
    "realpath",
    "readlink",
    "resolve-path",
    "rvpa",
    "convert-path",
    "join-path",
    "get-childitem",
    "gci",
    "get-item",
    "gi",
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
    WORK.with(|w| w.set(0));
    match parsed {
        Parsed::Plain(commands) => {
            if let Some(why) = commands
                .iter()
                .find_map(|c| command(&c.program, &c.args, 0))
            {
                return Some(why);
            }
            // An alias given an assistant's name and then run (`alias c=claude; c -p x`,
            // `Set-Alias c claude; c -p x`).
            let words: Vec<Vec<String>> = commands
                .iter()
                .map(|c| {
                    std::iter::once(c.program.clone())
                        .chain(c.args.clone())
                        .collect()
                })
                .collect();
            let named = named_assistants(&words);
            if let Some((c, name)) = commands
                .iter()
                .find_map(|c| holds_assistant(&c.program, &named).map(|name| (c, name)))
            {
                return Some(format!(
                    "{} stands for {name}, a coding assistant that works online with your account",
                    c.program
                ));
            }
            // A command line fed to a shell on its standard input (`echo 'claude -p x' | sh`):
            // what the other commands print may be what it runs.
            let fed = |c: &&crate::command::SimpleCommand| reads_commands(&c.program, &c.args);
            if commands.iter().any(|c| fed(&c)) {
                return commands
                    .iter()
                    .filter(|c| !fed(c))
                    .find_map(|c| fed_line(&c.args, 1));
            }
            None
        }
        Parsed::Complex { .. } => in_text(line, 0),
    }
}

/// The first bypass token on the line, as listed.
pub fn bypass_token(line: &str) -> Option<&'static str> {
    let lower = line.to_lowercase();
    // Two lines joined by an escape (`\`, PowerShell's backtick, cmd's caret) as one.
    let joined = join_escaped_lines(&lower);
    let unquoted: String = joined
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\' | '`' | '^'))
        .collect();
    BYPASS_TOKENS.iter().copied().find(|token| {
        let token = token.to_lowercase();
        lower.contains(&token) || joined.contains(&token) || unquoted.contains(&token)
    })
}

/// `text` with every escape at the end of a line taken away with the line break: a
/// backslash (POSIX), a backtick (PowerShell) or a caret (cmd). More than any one shell
/// joins; only used to find more.
fn join_escaped_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if matches!(c, '\\' | '`' | '^') {
            if chars.peek() == Some(&'\n') {
                chars.next();
                continue;
            }
            if chars.peek() == Some(&'\r') {
                let mut ahead = chars.clone();
                ahead.next();
                if ahead.peek() == Some(&'\n') {
                    chars.next();
                    chars.next();
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// A word as a name: white space and quotes around it, and cmd's and PowerShell's escapes
/// (`c^laude`, ``c`laude``) inside it, taken away.
fn clean(word: &str) -> String {
    word.trim()
        .trim_matches(|c| matches!(c, '\'' | '"' | '`'))
        .chars()
        .filter(|c| !matches!(c, '^' | '`'))
        .collect()
}

/// Whether a word among a program's arguments names a coding assistant: by name, or as a
/// pattern the shell expands to one (`cl?ude`, `~/.local/bin/clau*`) that spells at
/// least two letters of it.
fn is_assistant(word: &str) -> bool {
    names_assistant(word, 2)
}

/// Whether a word in the program's place names a coding assistant: by name, or as any
/// pattern the shell can expand to one.
fn is_assistant_program(word: &str) -> bool {
    names_assistant(word, 0)
}

/// Whether `word` names an assistant, as written or as a pattern with at least `letters`
/// fixed characters in its last part.
fn names_assistant(word: &str, letters: usize) -> bool {
    let word = clean(word);
    if word.is_empty() {
        return false;
    }
    let name = program_name(&word);
    if ASSISTANTS.contains(&name.as_str()) {
        return true;
    }
    let last = word
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&word)
        .to_lowercase();
    if !last.contains(['*', '?', '[']) || fixed_chars(&last) < letters {
        return false;
    }
    let pattern: Vec<char> = last.chars().collect();
    ASSISTANTS.iter().any(|a| {
        ["", ".exe", ".cmd", ".bat", ".com", ".ps1"]
            .iter()
            .any(|ext| {
                let candidate: Vec<char> = format!("{a}{ext}").chars().collect();
                glob_matches(&pattern, &candidate)
            })
    })
}

/// How many characters of a pattern are fixed (a `[…]` class counts as one).
fn fixed_chars(pattern: &str) -> usize {
    let mut count = 0;
    let mut in_class = false;
    for c in pattern.chars() {
        match c {
            '[' if !in_class => in_class = true,
            ']' if in_class => {
                in_class = false;
                count += 1;
            }
            '*' | '?' if !in_class => {}
            _ if !in_class => count += 1,
            _ => {}
        }
    }
    count
}

/// Whether a shell pattern (`*`, `?`, `[…]` with `!` or `^` and ranges) matches `name`.
fn glob_matches(pattern: &[char], name: &[char]) -> bool {
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        if let Some(&c) = pattern.get(p) {
            match c {
                '*' => {
                    star = Some((p, n));
                    p += 1;
                    continue;
                }
                '?' => {
                    p += 1;
                    n += 1;
                    continue;
                }
                '[' => {
                    if let Some((matched, next)) = class_matches(pattern, p, name[n]) {
                        if matched {
                            p = next;
                            n += 1;
                            continue;
                        }
                    } else if name[n] == '[' {
                        p += 1;
                        n += 1;
                        continue;
                    }
                }
                '\\' if pattern.get(p + 1) == Some(&name[n]) => {
                    p += 2;
                    n += 1;
                    continue;
                }
                c if c == name[n] => {
                    p += 1;
                    n += 1;
                    continue;
                }
                _ => {}
            }
        }
        match star {
            Some((sp, sn)) => {
                p = sp + 1;
                n = sn + 1;
                star = Some((sp, sn + 1));
            }
            None => return false,
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

/// Whether the class `[…]` at `start` of a pattern matches `c`, and where the pattern goes
/// on after it; `None` when it has no closing bracket (then `[` is a plain character).
fn class_matches(pattern: &[char], start: usize, c: char) -> Option<(bool, usize)> {
    let mut i = start + 1;
    let negated = matches!(pattern.get(i), Some('!' | '^'));
    if negated {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    loop {
        let &d = pattern.get(i)?;
        if d == ']' && !first {
            return Some((matched != negated, i + 1));
        }
        first = false;
        // A named class: `[:alpha:]`, `[:lower:]`, `[:alnum:]`, …
        if d == '[' && pattern.get(i + 1) == Some(&':') {
            let rest: String = pattern[i + 2..].iter().collect();
            if let Some(end) = rest.find(":]") {
                let name = &rest[..end];
                let fits = match name {
                    "alpha" => c.is_alphabetic(),
                    "lower" => c.is_lowercase(),
                    "upper" => c.is_uppercase(),
                    "digit" => c.is_ascii_digit(),
                    "xdigit" => c.is_ascii_hexdigit(),
                    "space" | "blank" => c.is_whitespace(),
                    "punct" => c.is_ascii_punctuation(),
                    "cntrl" => c.is_control(),
                    // alnum, word, graph, print, and names this check does not know:
                    // anything a name is made of.
                    _ => !c.is_whitespace() && !c.is_control(),
                };
                if fits {
                    matched = true;
                }
                i += 2 + name.chars().count() + 2;
                continue;
            }
        }
        if pattern.get(i + 1) == Some(&'-') && pattern.get(i + 2).is_some_and(|&e| e != ']') {
            let e = pattern[i + 2];
            if d <= c && c <= e {
                matched = true;
            }
            i += 3;
        } else {
            if d == c {
                matched = true;
            }
            i += 1;
        }
    }
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
    // Nix's installables (`nixpkgs#claude-code`, `github:owner/repo#codex`) and Deno's npm
    // packages (`npm:aider-chat`): the package after the `#` or the `npm:`.
    if let Some((_, package)) = spec.rsplit_once('#') {
        return names_assistant_package(package);
    }
    if let Some(package) = spec.strip_prefix("npm:") {
        return names_assistant_package(package);
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
    let name = without_version
        .rsplit('/')
        .next()
        .unwrap_or(without_version);
    if ASSISTANTS.contains(&name) || ASSISTANTS.contains(&program_name(name).as_str()) {
        return true;
    }
    PACKAGE_SUFFIXES.iter().any(|suffix| {
        name.strip_suffix(suffix)
            .is_some_and(|stem| ASSISTANTS.contains(&stem))
    })
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
        || PROGRAM_RUNNERS.contains(&name)
}

/// How many programs inside programs are followed (`sudo env timeout 9 npx claude`).
const MAX_DEPTH: usize = 8;

/// Why one simple command starts a coding assistant, if it does.
fn command(program: &str, args: &[String], depth: usize) -> Option<String> {
    if depth > MAX_DEPTH {
        return None;
    }
    let name = program_name(&clean(program));
    let name = name.as_str();
    // (a) The program itself, also written as a pattern the shell expands (`cl?ude`).
    if is_assistant_program(program) {
        let program = program.trim();
        return Some(format!(
            "{program} is a coding assistant: it works online with your account and can change files"
        ));
    }
    // (c) A script inside an assistant's package.
    if SCRIPT_HOSTS.contains(&name) && args.iter().any(|a| names_package_path(a)) {
        return Some(format!(
            "{program} is handed a script of a coding assistant"
        ));
    }
    // (b) A package runner handed an assistant's package.
    if let Some(rest) = runner_args(name, args)
        && let Some(found) = words(rest).into_iter().find(|w| names_assistant_package(w))
    {
        return Some(format!(
            "{program} runs {found}, a coding assistant that works online with your account"
        ));
    }
    // (b) A package manager handed an assistant's program by name.
    if let Some(found) = bin_runner_program(name, args).filter(|p| is_assistant(p)) {
        return Some(format!(
            "{program} runs {found}, a coding assistant that works online with your account"
        ));
    }
    // (a) The program `find -exec` or `fd -x` hands each file to.
    if let Some((_, flags)) = PROGRAM_FLAGS.iter().find(|(n, _)| *n == name) {
        for at in (0..args.len()).filter(|&i| flags.contains(&args[i].as_str())) {
            if let Some(next) = args.get(at + 1)
                && let Some(why) = command(next, &args[at + 2..], depth + 1)
            {
                return Some(why);
            }
        }
    }
    // (a) Code handed to an interpreter that runs a command line given as a string.
    if !SHELLS.contains(&name)
        && let Some(code) = inline_code(name, args)
        && let Some(why) = in_code(code, depth + 1)
    {
        return Some(why);
    }
    // (a) After a wrapper, or inside a command handed to a shell.
    let wraps = is_wrapper(name);
    if !wraps && !SHELLS.contains(&name) {
        // A program that runs the commands it reads (`at now <<< 'claude -p x'`).
        if INPUT_RUNNERS.contains(&name)
            && let Some(why) = args
                .iter()
                .filter(|a| a.contains(char::is_whitespace))
                .find_map(|a| in_text(a, depth + 1))
        {
            return Some(why);
        }
        // Any program that runs what follows a word such as `run`, `exec` or `--`.
        return after_a_run_word(program, name, args, depth);
    }
    if wraps {
        // An assistant named among its words, or a pattern in the place of the program it
        // runs (`timeout 9 ~/.local/bin/c*`).
        let mut place = ProgramPlace::new();
        let found = words(args)
            .into_iter()
            .find(|w| is_assistant(w) || names_package_path(w))
            .or_else(|| {
                args.iter()
                    .map(String::as_str)
                    .find(|a| place.next(a).is_some_and(|fixed| names_assistant(a, fixed)))
            });
        if let Some(found) = found {
            return Some(format!(
                "{program} runs {found}, a coding assistant that works online with your account"
            ));
        }
    }
    // A command handed over as one argument (`sh -c "…"`, `cmd /c "…"`), read on its own.
    for arg in args.iter().filter(|a| a.contains(char::is_whitespace)) {
        if let Some(why) = in_text(arg, depth + 1) {
            return Some(why);
        }
    }
    // A shell handed a command as separate words after its flag (`cmd /c claude -p x`,
    // `powershell -Command codex exec x`): the first word after a flag is a program.
    if SHELLS.contains(&name)
        && let Some(at) = (1..args.len())
            .find(|&i| args[i - 1].starts_with(['-', '/']) && !args[i].starts_with(['-', '/']))
        && let Some(why) = command(&args[at], &args[at + 1..], depth + 1)
    {
        return Some(why);
    }
    // The program a wrapper runs, in the place after its own options, settings and numbers
    // (`env nix run nixpkgs#claude-code`, `sudo kubectl exec pod -- codex`,
    // `timeout 9 pixi run …`).
    let program_at = wraps
        .then(|| {
            let mut place = ProgramPlace::new();
            args.iter().position(|a| place.next(a).is_some())
        })
        .flatten();
    if let Some(at) = program_at
        && let Some(why) = command(&args[at], &args[at + 1..], depth + 1)
    {
        return Some(why);
    }
    // The first program among the arguments that can run another one: a runner, a script
    // host, a shell or another wrapper (`env npx …`, `timeout 9 node …`), unless it was
    // just read in the program's place.
    let at = args.iter().position(|a| {
        let n = program_name(a);
        RUNNERS.contains(&n.as_str())
            || RUNNER_SUBCOMMANDS.iter().any(|(m, _)| *m == n)
            || SCRIPT_HOSTS.contains(&n.as_str())
            || SHELLS.contains(&n.as_str())
            || is_wrapper(&n)
    })?;
    if Some(at) == program_at {
        return None;
    }
    command(&args[at], &args[at + 1..], depth + 1)
}

/// Why a program that runs what follows one of its words (`uv run claude`, `bundle exec
/// codex`, `kubectl exec pod -- claude`, `docker run img claude`, `nix develop -c claude`)
/// starts a coding assistant, if it does. The word is one of [`RUN_WORDS`] among its first
/// three plain words (the value of a long option before it not counted:
/// `uv --directory . run`), or one of [`LONG_RUN_FLAGS`] anywhere; from there on, every
/// plain word that names an assistant (or its package, or holds a command line that starts
/// one) counts, except the value of an option known to take one ([`LONG_VALUE_FLAGS`]:
/// `--model claude`); any other option may take no value (`uv run --frozen claude`). A
/// pattern in the program's place counts as in the place of a wrapper's program
/// ([`ProgramPlace`]: `uv run c*`). After one of [`SHORT_RUN_FLAGS`] only the next word
/// counts. Programs that only read are left out.
fn after_a_run_word(program: &str, name: &str, args: &[String], depth: usize) -> Option<String> {
    if OWN_RUN_WORDS.contains(&name) || READ_ONLY.contains(&name) {
        return None;
    }
    // `fixed`: where the word may be the program, how many letters a pattern in it must fix.
    let runs = |word: &str, fixed: Option<usize>| -> Option<String> {
        if is_assistant(word)
            || fixed.is_some_and(|fixed| names_assistant(word, fixed))
            || names_assistant_package(word)
        {
            let found = word.trim();
            return Some(format!(
                "{program} runs {found}, a coding assistant that works online with your account"
            ));
        }
        if word.contains(char::is_whitespace) {
            return in_text(word, depth + 1);
        }
        None
    };
    let mut plain = 0;
    let mut started = false;
    // Whether the word before was a long option that may take this word as its value.
    let mut skip_value = false;
    let mut place = ProgramPlace::new();
    for (i, arg) in args.iter().enumerate() {
        let lower = arg.to_lowercase();
        if SHORT_RUN_FLAGS.contains(&lower.as_str()) {
            if let Some(why) = args.get(i + 1).and_then(|next| runs(next, None)) {
                return Some(why);
            }
            skip_value = false;
            place.next(arg);
            continue;
        }
        if let Some((flag, _)) = lower.split_once('=')
            && LONG_RUN_FLAGS.contains(&flag)
        {
            let value = &arg[flag.len() + 1..];
            if let Some(why) = runs(value, Some(0)) {
                return Some(why);
            }
            started = true;
            skip_value = false;
            // The value is the program: what follows are its arguments.
            place = ProgramPlace::new();
            place.next(value);
            continue;
        }
        if LONG_RUN_FLAGS.contains(&lower.as_str()) {
            started = true;
            skip_value = false;
            place = ProgramPlace::new();
            continue;
        }
        if arg.starts_with('-') {
            // Before the run word, any long option may take the next word as its value;
            // after it, only one known to take one does.
            skip_value = arg.starts_with("--")
                && !arg.contains('=')
                && (!started || LONG_VALUE_FLAGS.contains(&lower.as_str()));
            place.next(arg);
            continue;
        }
        let value = std::mem::take(&mut skip_value);
        if !started {
            plain += usize::from(!value);
            if plain <= 3 && RUN_WORDS.contains(&lower.as_str()) {
                started = true;
                place = ProgramPlace::new();
            }
            continue;
        }
        if value {
            place.value();
            continue;
        }
        let fixed = place.next(arg);
        if let Some(why) = runs(arg, fixed) {
            return Some(why);
        }
    }
    None
}

/// Where the program that a wrapper or a run word runs stands among the words after it,
/// read one word at a time: the first plain word past options, settings (`FOO=1`) and
/// numbers (`timeout 9`, `nice -n 5`). A word right after an option may be the option's
/// value, so the word after it may be the program too; such places are less sure.
struct ProgramPlace {
    /// Whether no word so far may have been the program.
    sure: bool,
    /// Whether the word before was an option that may take the next word as its value.
    after_option: bool,
    /// Whether the program's place is behind.
    passed: bool,
}

impl ProgramPlace {
    fn new() -> Self {
        Self {
            sure: true,
            after_option: false,
            passed: false,
        }
    }

    /// For the next word: whether it may be the program and, if so, how many letters a
    /// pattern in it must fix to name an assistant: none in a sure place (as in the
    /// program's own place), one where the word before may have been an option's value
    /// (so `tests/*` there stays an argument).
    fn next(&mut self, word: &str) -> Option<usize> {
        if self.passed {
            return None;
        }
        if word.starts_with('-') {
            self.after_option = !word.contains('=');
            return None;
        }
        if word.contains('=') || is_number(word) {
            self.after_option = false;
            return None;
        }
        let maybe_value = std::mem::take(&mut self.after_option);
        let fixed = usize::from(!self.sure || maybe_value);
        if maybe_value {
            self.sure = false;
        } else {
            self.passed = true;
        }
        Some(fixed)
    }

    /// The next word is the value of the option before it.
    fn value(&mut self) {
        self.after_option = false;
    }
}

/// Whether a word is a number, a duration, a mask or a list of them (`9`, `9s`, `1.5m`,
/// `+5`, `0-3`, `0x3`): a wrapper's own value, not a program.
fn is_number(word: &str) -> bool {
    let word = word.strip_prefix('+').unwrap_or(word);
    if let Some(hex) = word.strip_prefix("0x") {
        return !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    let word = word.strip_suffix(['s', 'm', 'h', 'd']).unwrap_or(word);
    word.starts_with(|c: char| c.is_ascii_digit())
        && word
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | ':' | '-'))
}

/// Whether a command runs the commands it reads on standard input: a shell handed no
/// command or script (`sh`, `bash -s`, `zsh -`, `pwsh -Command -`, `cmd`), or `at` or
/// `batch` without `-f`.
fn reads_commands(program: &str, args: &[String]) -> bool {
    let name = program_name(&clean(program));
    if INPUT_RUNNERS.contains(&name.as_str()) {
        return !args.iter().any(|a| a == "-f");
    }
    if !SHELLS.contains(&name.as_str()) {
        return false;
    }
    let option = |a: &String| a.starts_with('-') || (name == "cmd" && a.starts_with('/'));
    args.iter().any(|a| a == "-s") || args.iter().all(|a| a == "-" || option(a))
}

/// Why the words of a command whose output a shell runs start a coding assistant, if they
/// do: each word holding a command line, and the words together (`echo claude -p x`).
fn fed_line(words: &[String], depth: usize) -> Option<String> {
    words
        .iter()
        .filter(|w| w.contains(char::is_whitespace))
        .find_map(|w| in_text(w, depth))
        .or_else(|| in_text(&words.join(" "), depth))
}

/// What a package runner is handed, if `name` is one: `npx …`, or what follows the
/// subcommand of `npm exec …`, `pnpm dlx …`, `pipx run …`.
fn runner_args<'a>(name: &str, args: &'a [String]) -> Option<&'a [String]> {
    if RUNNERS.contains(&name) {
        return Some(args);
    }
    let (_, subs) = RUNNER_SUBCOMMANDS
        .iter()
        .find(|(manager, _)| *manager == name)?;
    // The subcommand is the first of its words on the line: an option before it can take a
    // value of its own (`npm --prefix /x exec …`, `pnpm --dir x dlx …`).
    let at = args
        .iter()
        .position(|a| subs.contains(&a.to_lowercase().as_str()))?;
    Some(&args[at + 1..])
}

/// The program a package manager is handed by name (`yarn claude`, `pnpm --dir x codex`,
/// `bun run claude`), if `name` is one that runs such a program.
fn bin_runner_program<'a>(name: &str, args: &'a [String]) -> Option<&'a str> {
    if !BIN_RUNNERS.contains(&name) {
        return None;
    }
    let mut i = 0;
    let mut run_seen = false;
    while let Some(arg) = args.get(i) {
        if BIN_RUNNER_VALUE_FLAGS.contains(&arg.as_str()) {
            i += 2;
        } else if arg.starts_with('-') {
            i += 1;
        } else if !run_seen && arg.eq_ignore_ascii_case("run") {
            run_seen = true;
            i += 1;
        } else {
            return Some(arg);
        }
    }
    None
}

/// The code handed to an interpreter on its command line (`python3 -c "…"`, `node -e
/// "…"`, `perl -e '…'`, `osascript -e '…'`), if there is some.
fn inline_code<'a>(name: &str, args: &'a [String]) -> Option<&'a str> {
    let (_, flags) = INLINE.iter().find(|(interp, _)| {
        name == *interp
            || name
                .strip_prefix(interp)
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
    })?;
    args.iter().enumerate().find_map(|(i, arg)| {
        let lower = arg.to_lowercase();
        flags.iter().find_map(|flag| {
            if lower == *flag {
                args.get(i + 1).map(String::as_str)
            } else {
                lower
                    .strip_prefix(flag)
                    .and_then(|rest| rest.strip_prefix('='))
                    .map(|_| &arg[flag.len() + 1..])
            }
        })
    })
}

/// Why code handed to an interpreter starts a coding assistant, if it does: a string in it
/// that is a command line starting one (`os.system('claude -p x')`,
/// `execSync('codex exec x')`, `["claude", "-p", "x"]`), or a script of one.
fn in_code(code: &str, depth: usize) -> Option<String> {
    if depth > MAX_DEPTH {
        return None;
    }
    if names_package_path(code) {
        return Some("the command runs a script of a coding assistant".into());
    }
    quoted_strings(code)
        .iter()
        .find_map(|text| in_text(text, depth + 1))
}

/// The text of every string written in quotes (`'…'`, `"…"`, `` `…` ``) in a piece of code,
/// with a backslash taking the next character as it is.
fn quoted_strings(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = code.chars();
    while let Some(c) = chars.next() {
        if !matches!(c, '\'' | '"' | '`') {
            continue;
        }
        let mut text = String::new();
        let mut escaped = false;
        for d in chars.by_ref() {
            if escaped {
                text.push(d);
                escaped = false;
            } else if d == '\\' {
                escaped = true;
            } else if d == c {
                break;
            } else {
                text.push(d);
            }
        }
        out.push(text);
    }
    out
}

/// How far after a `(` or a backtick its end is looked for when a locator's command may
/// be inside: such a command is short (`$(which claude)`, `(Get-Command claude).Source`).
/// A longer one is not read (known limit); the bound keeps each `(` cheap.
const MAX_LOCATOR_SPAN: usize = 1024;

/// A command substitution that only finds a coding assistant (`$(which claude)`,
/// `` `command -v codex` ``, `(Get-Command claude)`) replaced by the name it finds, so the
/// name stands where the program is run: `$(which claude) -p x` reads as `claude -p x`.
/// Only a substitution whose first word is a locator's and that ends within
/// [`MAX_LOCATOR_SPAN`] is read, and the text that takes counts against the limit of work:
/// `None` past it.
fn resolve_locators(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    // Whether the command being read has a word before the current point.
    let mut has_word = false;
    let after = |mut has_word: bool, text: &str| {
        for c in text.chars() {
            if ";|&\n{()".contains(c) {
                has_word = false;
            } else if !c.is_whitespace() {
                has_word = true;
            }
        }
        has_word
    };
    while let Some(open) = rest.find(['(', '`']) {
        let paren = rest[open..].starts_with('(');
        let close = if paren { ')' } else { '`' };
        let after_open = &rest[open + 1..];
        let head = &rest[..open];
        let word_before = after(has_word, head.strip_suffix('$').unwrap_or(head));
        // PowerShell's `( … )` gives a value: `&` or `.` in front of it runs it, and as an
        // argument it can be the program another one runs (`Start-Process -FilePath
        // (Get-Command claude).Source`). In the program's own place it is only a value.
        let runs =
            !paren || head.ends_with('$') || head.trim_end().ends_with(['&', '.']) || word_before;
        let mut found = None;
        let window = cut(after_open, MAX_LOCATOR_SPAN);
        if runs && may_be_locator(window) {
            let end = window.find(close);
            if too_much(end.map_or(window.len(), |end| 2 * end + 1)) {
                return None;
            }
            if let Some(end) = end {
                let words = shell_words(&after_open[..end]);
                found = match words.split_first() {
                    Some((first, more)) if LOCATORS.contains(&program_name(first).as_str()) => more
                        .iter()
                        .find(|w| !w.starts_with('-') && is_assistant(w))
                        .map(|name| (name.clone(), end)),
                    _ => None,
                };
            }
        }
        match found {
            Some((name, end)) => {
                out.push_str(head.strip_suffix('$').unwrap_or(head));
                out.push(' ');
                out.push_str(&name);
                out.push(' ');
                has_word = true;
                rest = &after_open[end + 1..];
            }
            None => {
                out.push_str(&rest[..=open]);
                has_word = after(has_word, &rest[..=open]);
                rest = after_open;
            }
        }
    }
    out.push_str(rest);
    Some(out)
}

/// At most the first `max` bytes of a text, cut at a character's start.
fn cut(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Whether the command at the start of `text` (just after a `(` or backtick) may be a
/// locator's: its first word, quoted or up to white space or the next `(`, `)` or
/// backtick, names one. Cheap, so that a line of many `(` is read in linear time: the
/// first words of two of them do not overlap, except quoted ones, which end at the next
/// quote.
fn may_be_locator(text: &str) -> bool {
    let text = text.trim_start();
    let first = match text.chars().next() {
        Some(quote @ ('\'' | '"')) => text[1..].split(quote).next().unwrap_or_default(),
        _ => text
            .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '`'))
            .next()
            .unwrap_or_default(),
    };
    let name: String = first
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '^'))
        .collect();
    LOCATORS.contains(&program_name(&name).as_str())
}

/// The words a word gives once the shell expands its braces (`{claude,}` gives `claude`,
/// `c{la,}ude` gives `claude cude`, `{c..c}laude` gives `claude`); a word without such a
/// group is itself. At most [`MAX_BRACE_WORDS`] words; what is left after that stays as
/// written.
fn brace_words(word: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut todo = vec![word.to_string()];
    while let Some(word) = todo.pop() {
        if out.len() + todo.len() >= MAX_BRACE_WORDS {
            out.push(word);
            continue;
        }
        match group(&word, '{') {
            Some((open, close, parts)) => {
                let (head, tail) = (&word[..open], &word[close + 1..]);
                // In their order: the stack takes the last first.
                todo.extend(
                    parts
                        .into_iter()
                        .rev()
                        .map(|part| format!("{head}{part}{tail}")),
                );
            }
            None if word.is_empty() => {}
            None => out.push(word),
        }
    }
    out
}

/// How many words [`brace_words`] gives at most.
const MAX_BRACE_WORDS: usize = 64;

/// The longest word whose groups [`expand_braces`] expands.
const MAX_GROUP_WORD: usize = 4096;

/// The first group of a word that the shell turns into several (not `${…}` or `$(…)`):
/// with `{`, braces with a comma (`{claude,}`) or a sequence of single letters or digits
/// (`{a..e}`); with `(`, zsh's alternatives in a pattern (`/usr/bin/(cla|x)ude`). Where it
/// opens and closes, and its parts.
fn group(word: &str, open_char: char) -> Option<(usize, usize, Vec<String>)> {
    let (close_char, cut) = if open_char == '{' {
        ('}', ',')
    } else {
        (')', '|')
    };
    let opens = word.char_indices().filter(|&(_, c)| c == open_char);
    for (open, _) in opens.take(64) {
        if word[..open].ends_with(['$', '<', '>']) {
            continue;
        }
        let mut level = 0;
        let mut cuts = Vec::new();
        let mut close = None;
        for (i, d) in word[open..].char_indices() {
            if d == open_char {
                level += 1;
            } else if d == close_char {
                level -= 1;
                if level == 0 {
                    close = Some(open + i);
                    break;
                }
            } else if d == cut && level == 1 {
                cuts.push(open + i);
            }
        }
        let Some(close) = close else {
            continue;
        };
        if !cuts.is_empty() {
            let mut parts = Vec::new();
            let mut from = open + 1;
            for at in cuts.into_iter().chain(std::iter::once(close)) {
                parts.push(word[from..at].to_string());
                from = at + 1;
            }
            return Some((open, close, parts));
        }
        // A sequence: `{a..e}`, `{e..a}`, `{0..3}`.
        if open_char == '{'
            && let Some((a, b)) = word[open + 1..close].split_once("..")
            && let (Some(a), Some(b)) = (single(a), single(b))
        {
            let (low, high) = if a <= b { (a, b) } else { (b, a) };
            let parts: Vec<String> = (low..=high)
                .take(MAX_BRACE_WORDS)
                .map(String::from)
                .collect();
            return Some((open, close, parts));
        }
    }
    None
}

/// The one letter or digit a piece of text is, if it is one.
fn single(text: &str) -> Option<char> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => Some(c),
        _ => None,
    }
}

/// A text with each of its words expanded as the shell expands braces, and zsh's
/// alternatives in a pattern each read as a program of its own: the pattern
/// `/usr/bin/(x|claude)` runs whichever of its alternatives exists, so
/// `/usr/bin/(x|claude) -p y` reads as `/usr/bin/x ; /usr/bin/claude -p y`.
fn expand_braces(text: &str) -> String {
    if !text.contains(['{', '(']) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.len() <= MAX_GROUP_WORD {
            let mut words = brace_words(word);
            if words.len() == 1
                && let Some((open, close, parts)) = group(&words[0], '(')
            {
                let (head, tail) = (&words[0][..open], &words[0][close + 1..]);
                words = vec![
                    parts
                        .iter()
                        .map(|part| format!("{head}{part}{tail}"))
                        .collect::<Vec<_>>()
                        .join(" ; "),
                ];
            }
            out.push_str(&words.join(" "));
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_whitespace() {
            flush(&mut word, &mut out);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// The most text, in bytes, that the readings of one line may take together; past it the
/// line counts as starting a coding assistant, since it was not read to the end.
const MAX_WORK: usize = 8 * 1024 * 1024;

/// Why a line counts when it was too much to read to the end.
const TOO_MUCH: &str =
    "the command is too long or too deeply nested to check whether it starts a coding assistant";

thread_local! {
    /// The text read so far for the line [`find`] is reading.
    static WORK: Cell<usize> = const { Cell::new(0) };
}

/// Counts `bytes` of text read; whether the line is now past [`MAX_WORK`].
fn too_much(bytes: usize) -> bool {
    WORK.with(|w| {
        let total = w.get().saturating_add(bytes);
        w.set(total);
        total > MAX_WORK
    })
}

/// For a line that cannot be broken down: the first word of every command in it, read as
/// a program, in every reading of the line: as written, with quotes taken away, and with
/// quotes and escapes taken away; each also with two lines joined by an escape read as
/// one, with a locator's finding in its place, and with braces expanded. A script inside
/// an assistant's package anywhere in it counts too, and so does what a command prints
/// when a shell on the line runs it (`echo "claude -p $x" | sh`).
fn in_text(line: &str, depth: usize) -> Option<String> {
    if depth > MAX_DEPTH {
        return None;
    }
    if names_package_path(line) {
        return Some("the command runs a script of a coding assistant".into());
    }
    if too_much(line.len()) {
        return Some(TOO_MUCH.into());
    }
    let lower = line.to_lowercase();
    let mut bases = vec![lower.clone()];
    for joined in [join_lines(&lower).into_owned(), join_escaped_lines(&lower)] {
        if !bases.contains(&joined) {
            bases.push(joined);
        }
    }
    let mut readings: Vec<String> = Vec::new();
    for base in bases {
        // `find`'s `{}` and `\;` (or `';'`) are words, not braces or the end of a command:
        // `find . -exec echo {} \; -exec claude -p {} \;` stays one command.
        let base = base
            .replace("\\;", " _ ")
            .replace("';'", " _ ")
            .replace("\";\"", " _ ")
            .replace("{}", " _ ");
        let unquoted: String = base
            .chars()
            .filter(|c| !matches!(c, '\'' | '"' | '`' | '^'))
            .collect();
        // As the POSIX shell reads a word: escapes and quotes taken away (`cod\ex`,
        // `$'claude'`).
        let mut unescaped = String::with_capacity(base.len());
        let mut chars = base.chars().peekable();
        while let Some(c) = chars.next() {
            let quote_follows = matches!(chars.peek(), Some('\'' | '"'));
            if !(matches!(c, '\'' | '"' | '`' | '^' | '\\') || (c == '$' && quote_follows)) {
                unescaped.push(c);
            }
        }
        for text in [base, unquoted, unescaped] {
            let Some(located) = resolve_locators(&text) else {
                return Some(TOO_MUCH.into());
            };
            let expanded = expand_braces(&located);
            for reading in [located, expanded] {
                if !readings.contains(&reading) {
                    readings.push(reading);
                }
            }
        }
    }
    for text in &readings {
        if too_much(text.len()) {
            return Some(TOO_MUCH.into());
        }
        // .NET's `[Diagnostics.Process]::Start('claude', '-p x')`: the first argument is a
        // program.
        for (at, _) in text.match_indices("::start(") {
            let first = text[at + "::start(".len()..]
                .split([',', ')'])
                .next()
                .unwrap_or_default()
                .trim();
            if is_assistant(first) {
                return Some(format!(
                    "{first} is a coding assistant: it works online with your account and can change files"
                ));
            }
        }
        // `${name}` read as `$name`, so the braces do not split it.
        if let Some(why) = read_commands(&plain_variables(text), depth, true) {
            return Some(why);
        }
    }
    None
}

/// Why the commands of one reading of a line start a coding assistant, if they do: the
/// first word of each command, read as a program; a name given an assistant's name on the
/// line and used as the program or in its place (`c=claude; $c -p x`, `alias c=claude`,
/// `Set-Alias c claude`), read once more with the names put in (when `put_names`); and
/// what a command prints when a shell on the line runs it.
fn read_commands(text: &str, depth: usize, put_names: bool) -> Option<String> {
    let segments: Vec<&str> = text
        .split([';', '\n', '(', ')', '{', '}', '|', '&', '`'])
        .collect();
    let named = named_assistants(&segments.iter().map(|s| shell_words(s)).collect::<Vec<_>>());
    let commands: Vec<Vec<String>> = segments.iter().map(|s| command_words(s)).collect();
    for words in &commands {
        let Some((program, args)) = words.split_first() else {
            continue;
        };
        // `$c = 'claude'` gives the name; it does not use it.
        let assigns = args.first().is_some_and(|a| a.ends_with('='));
        if !assigns && let Some(name) = holds_assistant(program, &named) {
            return Some(format!(
                "{program} stands for {name}, a coding assistant that works online with your account"
            ));
        }
        if let Some(why) = command(program, args, depth + 1) {
            return Some(why);
        }
    }
    // A shell on the line that runs what it reads (`echo "claude -p $x" | sh`): what the
    // other commands print may be what it runs.
    let fed = |words: &Vec<String>| {
        words
            .split_first()
            .is_some_and(|(program, args)| reads_commands(program, args))
    };
    if commands.iter().any(fed)
        && let Some(why) = commands
            .iter()
            .filter(|words| !fed(words) && !words.is_empty())
            .find_map(|words| fed_line(&words[1..], depth + 1))
    {
        return Some(why);
    }
    // The names put in where they are used (`c=claude && env $c -p x`).
    if put_names && named.iter().any(|n| !n.alias) {
        let put = put_in_names(text, &named);
        if put != text {
            if too_much(put.len()) {
                return Some(TOO_MUCH.into());
            }
            return read_commands(&put, depth, false);
        }
    }
    None
}

/// A text with each use of a variable given an assistant's name (`$c`, `$env:c`, `%c%`)
/// replaced by that name.
fn put_in_names(text: &str, named: &[Named]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'scan: while let Some(at) = rest.find(['$', '%', '!']) {
        out.push_str(&rest[..at]);
        let mark = &rest[at..at + 1];
        let after = &rest[at + 1..];
        let after_env = after.strip_prefix("env:").unwrap_or(after);
        for n in named.iter().filter(|n| !n.alias) {
            let candidates: &[&str] = if mark == "$" {
                &[after, after_env]
            } else {
                &[after]
            };
            for &candidate in candidates {
                let Some(tail) = candidate.strip_prefix(n.name.as_str()) else {
                    continue;
                };
                let ends = if mark == "$" {
                    // Not where it is given (`$c = 'claude'`).
                    let given =
                        tail.trim_start().starts_with('=') && !tail.trim_start().starts_with("==");
                    !given && !tail.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
                } else {
                    tail.starts_with(mark)
                };
                if ends {
                    out.push_str(&n.value);
                    rest = if mark == "$" { tail } else { &tail[1..] };
                    continue 'scan;
                }
            }
        }
        out.push_str(mark);
        rest = after;
    }
    out.push_str(rest);
    out
}

/// A text with every `${name}` written as `$name`.
fn plain_variables(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("${") {
        let after = &rest[at + 2..];
        let name_len = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':'))
            .unwrap_or(after.len());
        out.push_str(&rest[..at]);
        if name_len > 0 && after[name_len..].starts_with('}') {
            out.push('$');
            out.push_str(&after[..name_len]);
            rest = &after[name_len + 1..];
        } else {
            out.push_str("${");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// A name given a value on a line, and whether it is an alias (used as a program by its
/// bare name) or a variable (used as `$name`, `${name}`, `$env:name` or `%name%`).
struct Named {
    name: String,
    value: String,
    alias: bool,
}

/// The names given an assistant's name (or a path or pattern naming one) on a line read as
/// text: `c=claude`, `export C=/usr/local/bin/claude`, `$c = 'claude'`, `set c=codex`,
/// `alias c=claude`, `Set-Alias c claude`, `Set-Variable c claude`.
fn named_assistants(commands: &[Vec<String>]) -> Vec<Named> {
    let mut out = Vec::new();
    for words in commands {
        let words: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
        let mut add = |name: &str, value: &str, alias: bool| {
            let name = name
                .trim_start_matches('$')
                .trim_start_matches("env:")
                .trim_matches(['{', '}']);
            // An alias can hold options too (`alias c='claude --model x'`).
            let value = clean(value);
            let program = value.split_whitespace().next().unwrap_or_default();
            if !name.is_empty() && (is_assistant(program) || names_package_path(&value)) {
                out.push(Named {
                    name: name.to_string(),
                    value: program.to_string(),
                    alias,
                });
            }
        };
        let Some(first) = words.first() else {
            continue;
        };
        match first.as_str() {
            // PowerShell: `Set-Alias c claude`, `New-Variable -Name c -Value claude`.
            "set-alias" | "sal" | "new-alias" | "nal" | "set-variable" | "sv" | "new-variable"
            | "nv" => {
                let alias = first.contains("alias") || first.starts_with("sa") || first == "nal";
                let mut name = None;
                let mut value = None;
                let mut i = 1;
                while let Some(word) = words.get(i) {
                    match word.as_str() {
                        "-name" | "-n" => name = words.get(i + 1).cloned(),
                        "-value" | "-definition" | "-v" | "-d" => value = words.get(i + 1).cloned(),
                        w if w.starts_with('-') => {
                            i += 1;
                            continue;
                        }
                        w if name.is_none() => {
                            name = Some(w.to_string());
                            i += 1;
                            continue;
                        }
                        w => {
                            value.get_or_insert_with(|| w.to_string());
                            i += 1;
                            continue;
                        }
                    }
                    i += 2;
                }
                if let (Some(name), Some(value)) = (name, value) {
                    add(&name, &value, alias);
                }
            }
            _ => {
                let alias = first == "alias";
                let skip = usize::from(matches!(
                    first.as_str(),
                    "alias" | "export" | "declare" | "typeset" | "local" | "readonly" | "set"
                ));
                // `$c = 'claude'` (PowerShell).
                if let [name, eq, value, ..] = &words[skip..]
                    && eq == "="
                {
                    add(name, value, alias);
                }
                for word in words[skip..].iter().filter(|w| !w.starts_with('-')) {
                    match word.split_once('=') {
                        Some((name, value)) => add(name, value, alias),
                        None => break,
                    }
                }
            }
        }
    }
    out
}

/// Which assistant a program word stands for through a name given on the line, if any.
fn holds_assistant<'a>(program: &str, named: &'a [Named]) -> Option<&'a str> {
    if named.is_empty() {
        return None;
    }
    let word = clean(program);
    let variable = word
        .strip_prefix("${")
        .and_then(|w| w.strip_suffix('}'))
        .or_else(|| word.strip_prefix("$env:"))
        .or_else(|| word.strip_prefix('$'))
        .or_else(|| word.strip_prefix('%').and_then(|w| w.strip_suffix('%')))
        .or_else(|| word.strip_prefix('!').and_then(|w| w.strip_suffix('!')));
    named
        .iter()
        .find(|n| match variable {
            Some(v) => !n.alias && v == n.name,
            None => n.alias && word == n.name,
        })
        .map(|n| n.value.as_str())
}

/// The words of one command read as text, without the keywords and settings in front of
/// its program (`then`, `if`, `FOO=1`).
fn command_words(segment: &str) -> Vec<String> {
    let mut words = shell_words(segment);
    // Keywords and settings in front of a program are not its name. They are counted first
    // and dropped together, so that a line of many of them takes linear time.
    let leading = words
        .iter()
        .take_while(|word| {
            let keyword = matches!(
                word.as_str(),
                "then" | "do" | "else" | "elif" | "if" | "while" | "until" | "!" | "." | "time"
            );
            let setting = word.split_once('=').is_some_and(|(name, _)| {
                !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == ':')
            });
            keyword || setting
        })
        .count();
    words.drain(..leading);
    words
}

/// The words of a piece of a command line: split at white space, except inside quotes,
/// with the quotes taken away (`'C:\Program Files\x.exe' -p` is two words).
fn shell_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in text.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                started = false;
            }
            None => word.push(c),
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests;
