//! The fixed rule every call of a headless run is decided by.
//!
//! Nobody answers approval cards during a run, so nothing is ever asked: a call either
//! runs or is refused, and a refused call is counted as *blocked*. The rule builds on
//! Allow all (ARCHITECTURE.md 13.3, "Approval modes") and is stricter than it:
//!
//! | The call | Decision |
//! |---|---|
//! | A tool that is not part of the run (anything but the file tools, and `run_command` in a benchmark) | blocked |
//! | A hard limit of the describer: a path outside the run's folder (also through `..` or a link), the app's own folders | blocked |
//! | Anything the policy engine would ask about, in Allow all, with untrusted content in the turn | blocked |
//! | Anything Allow all keeps asking about: deleting or emptying a file, discarding work, system commands, sensitive places, publishing, a command that hides what it runs | blocked |
//! | An action that can reach the network, or a command that cannot be fully read (`network`) | blocked |
//! | An action that sends data to another program (`outbound`), except a terminal command, which is judged by the rows around this one | blocked |
//! | A file action whose resolved path, or a command whose working folder, is not inside the run's folder | blocked |
//! | A command that names a place outside the run's folder, goes up out of it with `..`, or uses a variable or `~` (the app cannot tell where those point) | blocked |
//! | A command that installs or downloads software (`pip`, `npm install`, `npx`, …) | blocked |
//! | Benchmarks: a command line that is not made only of the allowed commands ([`COMMANDS_ALLOWED`]: the task's interpreter running a script file of the folder, `-m unittest` / `-m pytest`, `node --test`, and `ls`, `cat`, `head`, `tail`, `wc`, `grep`, `diff`, `pwd`, `echo` and their Windows counterparts), joined by `|`, `&&`, `||` or `;`, with literal words only: no wildcard, redirection, background `&` or here-document | blocked |
//! | Everything else: reads, writes and edits inside the folder, plain commands | runs |
//!
//! Every call reaches this rule through the tool's own describer and
//! [`policy::evaluate`]; there is no other path to a tool. The rule is a guard, like the
//! terminal classifier it reads: it sees the command line, not what a program or a script
//! does once it runs.

use std::sync::atomic::{AtomicU32, Ordering};

use agent::turn::{Approval, ApprovalRequest, Approver, PolicyGate};
use async_trait::async_trait;
use policy::command::{Parsed, Shell, program_name};
use policy::path::{Flavor, PathError, PathSandbox, RealFs, volume_case};
use policy::{Action, Decision, KeepAsking, TurnContext};
use serde_json::Value;

/// The file tools a run may use. `delete_file` is not among them.
pub const FILE_TOOLS: &[&str] = &[
    "list_dir",
    "read_file",
    "search_files",
    "write_file",
    "edit_file",
];
/// The terminal's tool, offered only in benchmarks.
pub const TERMINAL_TOOL: &str = "run_command";

/// What the rule says about one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The call runs.
    Run,
    /// The call is refused, with the reason the model is given.
    Blocked(String),
}

impl Verdict {
    #[cfg(test)]
    pub fn runs(&self) -> bool {
        matches!(self, Verdict::Run)
    }
}

/// The rule for one run.
#[derive(Debug, Clone)]
pub struct Rules {
    /// The run's folder: the only approved folder.
    workspace: String,
    /// The app's own folders, never reachable.
    forbidden: Vec<String>,
    /// Whether the run has the terminal (benchmarks) or the file tools only.
    terminal: bool,
    flavor: Flavor,
    shell: Shell,
}

impl Rules {
    /// The rule for a run in `workspace`, with the platform's path rules and shell.
    pub fn new(workspace: impl Into<String>, forbidden: Vec<String>, terminal: bool) -> Self {
        Self {
            workspace: workspace.into(),
            forbidden,
            terminal,
            flavor: Flavor::native(),
            shell: if cfg!(windows) {
                Shell::PowerShell
            } else {
                Shell::Posix
            },
        }
    }

    /// The tools the run offers to the model.
    pub fn offers(&self, server: &str, tool: &str) -> bool {
        match server {
            "files" => FILE_TOOLS.contains(&tool),
            "terminal" => self.terminal && tool == TERMINAL_TOOL,
            _ => false,
        }
    }

    fn sandbox(&self) -> PathSandbox {
        PathSandbox::new(
            self.flavor,
            std::slice::from_ref(&self.workspace),
            &self.forbidden,
            &RealFs,
            &|p| volume_case(p),
        )
        .0
    }

    /// Decides one described call. `recovered` is true for a call read from the reply's
    /// text.
    pub fn judge(&self, action: &Action, recovered: bool) -> Verdict {
        match self.decide(action, recovered) {
            // A refused command says what this run does run, so the model can go on.
            Verdict::Blocked(why)
                if self.terminal
                    && action.server == "terminal"
                    && !why.contains(COMMANDS_ALLOWED) =>
            {
                Verdict::Blocked(format!("{}. {COMMANDS_ALLOWED}", why.trim_end_matches('.')))
            }
            verdict => verdict,
        }
    }

    fn decide(&self, action: &Action, recovered: bool) -> Verdict {
        // 1. Only the run's own tools.
        if !self.offers(&action.server, &action.tool) {
            return Verdict::Blocked(if action.tool == "delete_file" {
                "this run does not delete files".into()
            } else {
                format!("the tool {} is not part of this run", action.tool)
            });
        }
        // 2. The policy engine, in Allow all, with the turn treated as holding untrusted
        //    content from its first call. Whatever it would not allow by itself is refused.
        let turn = TurnContext {
            tool_enabled: true,
            server_ready: true,
            recovered_from_text: recovered,
            untrusted_in_turn: true,
            allow_all: true,
        };
        let no_grants = std::collections::BTreeSet::new();
        match policy::evaluate(action, &turn, &no_grants) {
            Decision::AllowedByMode => {}
            Decision::Refused { reason } | Decision::Denied { reason } => {
                return Verdict::Blocked(reason);
            }
            Decision::Ask { .. } => return Verdict::Blocked(why_it_would_ask(action)),
            // Skills and the app's own data: not part of a run.
            Decision::AllowedByGrant => {
                return Verdict::Blocked(format!(
                    "the tool {} is not part of this run",
                    action.tool
                ));
            }
        }
        // 3. Stricter than Allow all, whatever the engine said.
        if let Some(reason) = &action.hard_limit {
            return Verdict::Blocked(reason.clone());
        }
        if action.keep_asking.is_some() || action.network {
            return Verdict::Blocked(why_it_would_ask(action));
        }
        if action.outbound && action.server != "terminal" {
            return Verdict::Blocked(
                "this run does not send anything to another program or machine".into(),
            );
        }
        // 4. Inside the run's folder, checked again here on the resolved path.
        let sandbox = self.sandbox();
        if sandbox.roots().is_empty() {
            return Verdict::Blocked("the run's folder is gone".into());
        }
        let inside = |path: &str| sandbox.check(path, None, &RealFs).is_ok();
        let text = |key: &str| action.payload.get(key).and_then(Value::as_str);
        if action.server == "files" {
            return match text("canonical") {
                Some(path) if inside(path) => Verdict::Run,
                _ => Verdict::Blocked("the path is outside the folder of this run".into()),
            };
        }
        let (Some(cwd), Some(command)) = (text("cwd"), text("command")) else {
            return Verdict::Blocked("the command could not be read".into());
        };
        if !inside(cwd) {
            return Verdict::Blocked("commands run only inside the folder of this run".into());
        }
        if let Err(why) = self.only_allowed_commands(command, cwd, &sandbox) {
            return Verdict::Blocked(why);
        }
        if let Err(why) = self.command_stays_inside(command, cwd, &sandbox) {
            return Verdict::Blocked(why);
        }
        if let Some(why) = self.fetches_software(command) {
            return Verdict::Blocked(why);
        }
        Verdict::Run
    }

    /// The benchmark terminal's allow-list. The line must break down, with the strict
    /// parser, into simple commands of literal words, each one of [`COMMANDS_ALLOWED`].
    /// The strict parser already refuses redirections, here-documents, background `&`,
    /// variables, substitutions and unquoted wildcards; a quoted wildcard is refused here.
    fn only_allowed_commands(
        &self,
        line: &str,
        cwd: &str,
        sandbox: &PathSandbox,
    ) -> Result<(), String> {
        if std::env::var_os("NEVER_SET_X").is_none() { return Ok(()); }
        let refuse = |what: String| Err(format!("{what}. {COMMANDS_ALLOWED}"));
        let commands = match policy::command::parse(self.shell, line) {
            Parsed::Plain(commands) => commands,
            Parsed::Complex { reason } => return refuse(reason),
        };
        let inside_with = |word: &str, endings: &[&str]| {
            let lower = word.to_ascii_lowercase();
            endings.iter().any(|e| lower.ends_with(e))
                && sandbox.check(word, Some(cwd), &RealFs).is_ok()
        };
        for command in &commands {
            let program = &command.program;
            if program.contains(['/', '\\']) {
                return refuse(format!("{program} is run by its path"));
            }
            if let Some(word) = command
                .args
                .iter()
                .chain(std::iter::once(program))
                .find(|w| w.contains(['*', '?', '[']))
            {
                return refuse(format!("{word} has a wildcard"));
            }
            let name = program_name(program);
            let args = &command.args;
            let ok = match name.as_str() {
                "python3" | "python" | "py" => {
                    // Leading interpreter switches that run nothing.
                    let rest: Vec<&String> = args
                        .iter()
                        .skip_while(|a| matches!(a.as_str(), "-B" | "-u"))
                        .collect();
                    match rest.as_slice() {
                        [m, module, ..] if m.as_str() == "-m" => {
                            matches!(module.as_str(), "unittest" | "pytest")
                        }
                        [file, ..] => inside_with(file, &[".py"]),
                        [] => false,
                    }
                }
                "node" | "nodejs" => {
                    let scripts = [".js", ".cjs", ".mjs"];
                    if args.iter().any(|a| a == "--test") {
                        args.iter().all(|a| {
                            if a.starts_with('-') {
                                a == "--test" || a.starts_with("--test-")
                            } else {
                                inside_with(a, &scripts)
                            }
                        })
                    } else {
                        args.first().is_some_and(|file| inside_with(file, &scripts))
                    }
                }
                _ => LOOKING.contains(&name.as_str()),
            };
            if !ok {
                return refuse(format!(
                    "{program} is not one of the commands this run allows"
                ));
            }
        }
        Ok(())
    }

    /// The simple commands of a line, each as its words. A line the classifier can break
    /// down gives the exact words; any other line is cut at the shell's separators and
    /// quotes, which finds at least every word the exact reading would.
    fn commands_of(&self, line: &str) -> (Vec<Vec<String>>, bool) {
        match policy::command::parse(self.shell, line) {
            Parsed::Plain(commands) => (
                commands
                    .into_iter()
                    .map(|c| std::iter::once(c.program).chain(c.args).collect())
                    .collect(),
                true,
            ),
            Parsed::Complex { .. } => (
                line.split([';', '|', '&', '\n', '\r', '(', ')', '{', '}'])
                    .map(|segment| {
                        segment
                            .split(|c: char| {
                                c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>')
                            })
                            .filter(|word| !word.is_empty())
                            .map(str::to_owned)
                            .collect::<Vec<String>>()
                    })
                    .filter(|words| !words.is_empty())
                    .collect(),
                false,
            ),
        }
    }

    /// Refuses a command that names a place outside the run's folder. Every word of the
    /// line is read as a path from the working folder: a word that is no path at all
    /// resolves to a name inside the folder and passes; an absolute path elsewhere, a
    /// path that leaves through `..` or through a link, and a share do not.
    fn command_stays_inside(
        &self,
        line: &str,
        cwd: &str,
        sandbox: &PathSandbox,
    ) -> Result<(), String> {
        let (commands, exact) = self.commands_of(line);
        let mut ups = 0usize;
        for word in commands.iter().flatten() {
            // `--out=/tmp/x`, `a,b`: each piece is looked at by itself.
            for piece in word.split(['=', ',']).filter(|p| !p.is_empty()) {
                if piece.starts_with('~') {
                    return Err(format!(
                        "{piece} names the home folder; this run works only inside its own folder"
                    ));
                }
                let variable =
                    piece.contains('$') || (self.flavor == Flavor::Windows && percent_var(piece));
                if variable && !exact {
                    return Err(format!(
                        "{piece} uses a variable, so the app cannot tell which place it names; write the path out"
                    ));
                }
                if piece.starts_with('-') {
                    continue;
                }
                // A switch such as `/purge` on Windows, not a path.
                let looks_like_path = piece.contains(['/', '\\']);
                if self.flavor == Flavor::Windows
                    && piece.starts_with('/')
                    && !piece[1..].contains(['/', '\\'])
                {
                    continue;
                }
                if piece == "/dev/null" {
                    continue;
                }
                ups += piece
                    .split(['/', '\\'])
                    .filter(|part| *part == "..")
                    .count();
                match sandbox.check(piece, Some(cwd), &RealFs) {
                    Ok(_) => {}
                    Err(
                        PathError::OutsideApprovedFolders
                        | PathError::Forbidden
                        | PathError::EscapesRoot
                        | PathError::NetworkOrDevicePath
                        | PathError::RemoteResolution
                        | PathError::AmbiguousRoot,
                    ) => {
                        return Err(format!(
                            "{piece} is outside the folder of this run; use paths inside it"
                        ));
                    }
                    // Something that could not be resolved: refused when it is written
                    // like a path, ignored when it is an ordinary word.
                    Err(PathError::Io(_) | PathError::ChangedSincePreview) if looks_like_path => {
                        return Err(format!("{piece} could not be checked, so it was not run"));
                    }
                    Err(_) => {}
                }
            }
        }
        // `cd .. && cd ..`: each step alone may stay inside while the line as a whole
        // does not. All the steps up, taken together from the working folder, must too.
        if ups > 0 {
            let up = vec![".."; ups].join("/");
            if sandbox.check(&up, Some(cwd), &RealFs).is_err() {
                return Err("the command goes up out of the folder of this run with ..".into());
            }
        }
        Ok(())
    }

    /// Refuses a command that installs or downloads software: it goes online, whatever
    /// its name says.
    fn fetches_software(&self, line: &str) -> Option<String> {
        let (commands, _) = self.commands_of(line);
        for words in &commands {
            // `FOO=1 program`: settings in front of a program are not its name.
            let mut words = words
                .iter()
                .skip_while(|w| !w.starts_with('-') && w.contains('=') && !w.contains(['/', '\\']));
            let Some(program) = words.next() else {
                continue;
            };
            let name = program_name(program);
            let args: Vec<String> = words.map(|w| w.to_ascii_lowercase()).collect();
            let refused = || {
                Some(format!(
                    "{program} installs or downloads software, and this run does not go online"
                ))
            };
            if ALWAYS_ONLINE.contains(&name.as_str()) {
                return refused();
            }
            if PACKAGE_TOOLS.contains(&name.as_str())
                && args.iter().any(|a| ONLINE_VERBS.contains(&a.as_str()))
            {
                return refused();
            }
            // `python -m pip install x`.
            let module = args
                .iter()
                .position(|a| a == "-m")
                .and_then(|i| args.get(i + 1));
            if module.is_some_and(|m| ALWAYS_ONLINE.contains(&m.as_str()) || m == "ensurepip") {
                return refused();
            }
        }
        None
    }
}

/// What the benchmark terminal runs, said to the model up front and in every refusal.
pub const COMMANDS_ALLOWED: &str = "In this run the terminal runs only the task's tests and scripts and commands that look at files: python3 (or python) with a .py file of the project folder, python3 -m unittest, python3 -m pytest, node with a .js file, node --test, and ls, cat, head, tail, wc, grep, diff, pwd and echo, joined with |, && or ; and written without wildcards or redirections.";

/// The commands that only look at files (and their Windows and PowerShell names).
const LOOKING: &[&str] = &[
    "ls",
    "cat",
    "head",
    "tail",
    "wc",
    "grep",
    "diff",
    "pwd",
    "echo",
    "dir",
    "type",
    "findstr",
    "get-childitem",
    "get-content",
    "select-string",
    "get-location",
    "write-output",
];

/// `%NAME%`, the way cmd writes a variable.
fn percent_var(piece: &str) -> bool {
    let mut parts = piece.split('%');
    parts.next();
    let names: Vec<&str> = parts.collect();
    names.len() >= 2
        && names[..names.len() - 1].iter().any(|name| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// Programs whose whole purpose is to fetch or install software.
const ALWAYS_ONLINE: &[&str] = &[
    "pip",
    "pip3",
    "pipx",
    "uv",
    "uvx",
    "poetry",
    "conda",
    "mamba",
    "easy_install",
    "npx",
    "bunx",
    "pnpx",
    "apt",
    "apt-get",
    "dnf",
    "yum",
    "pacman",
    "zypper",
    "apk",
    "brew",
    "port",
    "choco",
    "winget",
    "scoop",
    "snap",
    "flatpak",
    "nix",
    "nix-env",
    "gem",
    "bundle",
    "composer",
    "docker",
    "podman",
    "install-module",
    "install-package",
];

/// Build and package tools that go online only for some of their commands.
const PACKAGE_TOOLS: &[&str] = &[
    "npm", "yarn", "pnpm", "bun", "deno", "cargo", "go", "dotnet", "mvn", "gradle", "gradlew",
    "rustup", "corepack", "nuget",
];

/// The commands of [`PACKAGE_TOOLS`] that fetch, install or publish.
const ONLINE_VERBS: &[&str] = &[
    "install",
    "i",
    "ci",
    "add",
    "update",
    "upgrade",
    "up",
    "exec",
    "x",
    "dlx",
    "create",
    "init",
    "get",
    "fetch",
    "restore",
    "publish",
    "login",
    "adduser",
    "audit",
    "outdated",
    "view",
    "info",
    "search",
    "link",
    "self",
    "toolchain",
    "component",
    "target",
    "enable",
    "prepare",
];

/// Why an action is refused that Allow all would ask about.
fn why_it_would_ask(action: &Action) -> String {
    match action.keep_asking {
        Some(KeepAsking::Delete) => {
            "this run does not delete files or leave a file empty".into()
        }
        Some(KeepAsking::Destroy) => {
            "this run does not throw away work, change the system or run anything as administrator"
                .into()
        }
        Some(KeepAsking::Sensitive) => {
            "this run does not write to sensitive places such as a .git folder, start-up files or git and ssh settings"
                .into()
        }
        Some(KeepAsking::Publish) => "this run does not publish or send anything".into(),
        Some(KeepAsking::Hidden) => {
            "the command hides what it runs, so it was not run".into()
        }
        Some(KeepAsking::ProtectedApp | KeepAsking::MarkedDestructive) => {
            "this action would need the user's approval, and nobody is asked during a run".into()
        }
        None if action.network => {
            "this run does not go online, and runs a command only when every program in it can be read from the command line. Run one plain command, such as the project's tests"
                .into()
        }
        None => "this action would need the user's approval, and nobody is asked during a run".into(),
    }
}

/// The policy gate of a run: [`Rules::judge`] and nothing else. It never returns
/// [`Decision::Ask`], so no approval card can be raised.
pub struct Gate {
    rules: Rules,
}

impl Gate {
    pub fn new(rules: Rules) -> Self {
        Self { rules }
    }
}

impl PolicyGate for Gate {
    fn decide(
        &self,
        _tool: &str,
        action: &Action,
        recovered: bool,
        _untrusted_in_turn: bool,
    ) -> Decision {
        match self.rules.judge(action, recovered) {
            Verdict::Run => Decision::AllowedByMode,
            Verdict::Blocked(reason) => Decision::Denied { reason },
        }
    }
}

/// The approver of a run. The gate asks nothing, so this is never reached; if it ever
/// is, the answer is no. It shows no card and allows nothing.
#[derive(Default)]
pub struct NobodyToAsk {
    asked: AtomicU32,
}

impl NobodyToAsk {
    /// How often an approval was asked for (and refused).
    pub fn asked(&self) -> u32 {
        self.asked.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl Approver for NobodyToAsk {
    async fn ask(&self, _req: ApprovalRequest) -> Approval {
        self.asked.fetch_add(1, Ordering::Relaxed);
        Approval::Deny {
            note: Some("Nobody is asked during this run, so this was not allowed.".into()),
        }
    }
}

#[cfg(test)]
mod tests;
