//! Allow all (0.0.4): what still asks while a chat is in Allow all.
//!
//! [`crate::evaluate`] reads two facts a describer puts on an action: `keep_asking` (the
//! action asks whatever the mode) and `network` (the action asks once untrusted content
//! has entered the turn, D9). This module decides those facts for command lines
//! ([`command_facts`]), for the labels of web page buttons ([`web_label`]) and for
//! external tools ([`external`]); [`crate::files::keep_asking`] decides them for file
//! writes. The computer and git describers set them from what they already know.
//!
//! The rules, as the user is shown them:
//!
//! 1. Deleting: a delete program whatever its flags (`rm`, `unlink`, `rmdir`, `shred`,
//!    `del`, `erase`, `rd`, `Remove-Item` and its aliases, `git rm`, `git clean`,
//!    `find -delete`), and leaving a file empty.
//! 2. Discarding or destroying work or the system: `git reset --hard`, a forced push,
//!    disk and system programs, registry edits, elevation.
//! 3. A command that cannot be fully broken down (redirections, variables, command
//!    substitution, subshells, `eval`, `Invoke-Expression`, wrappers, inline interpreter
//!    code) when its text holds a delete or destroy word from rules 1 and 2 or a
//!    file-removal call of a common language. Without such a word it is allowed.
//! 4. Sensitive places: a `.git` folder, shell start-up files, git and ssh
//!    configuration, autostart locations.
//! 5. Publishing: `git push` and commands that publish or deploy.
//! 8. After untrusted content entered the turn: a command that names a network program
//!    or cannot be fully broken down.
//!
//! Like the terminal classifier it builds on, this is a guard against accidents and
//! obvious abuse, not a security boundary. It reads the command line. What a script file
//! or a program does once it runs cannot be seen from here.

use crate::command::{
    DELETES, ELEVATION, GIT_KNOWN, INLINE, Parsed, SYSTEM, Shell, SimpleCommand, WRAPPERS,
    is_read_only, parse_lenient, program_name,
};
use crate::engine::KeepAsking;
use crate::files::{WriteKind, always_ask_reason};
use crate::path::Flavor;

/// What Allow all needs to know about one command line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandFacts {
    /// Why the command still asks in Allow all, if it does, with a sentence that names
    /// what was found.
    pub keep_asking: Option<(KeepAsking, String)>,
    /// The command names a network program or cannot be fully broken down.
    pub network: bool,
    /// The command could not be fully broken down (its text was scanned for words).
    pub opaque: bool,
}

/// Programs that delete or empty files, beyond [`DELETES`]. That list also decides what a
/// grant can cover in Ask mode, so it stays as it is.
const MORE_DELETES: &[&str] = &[
    "truncate",
    "clear-content",
    "clc",
    "clear-item",
    "cli",
    "clear-recyclebin",
    "rimraf",
    "deltree",
    "rmtrash",
    "trash-put",
    "sdelete",
];

/// Programs that change how the system works, beyond [`SYSTEM`].
const MORE_SYSTEM: &[&str] = &["netsh", "set-mppreference", "set-executionpolicy"];

/// Programs that run another program, beyond [`WRAPPERS`]. Package runners such as `npx`
/// are not here: `npx vitest run` is a plain test run, and what they are handed is read
/// like any other program's arguments (`npx rimraf dist` asks).
const MORE_WRAPPERS: &[&str] = &[
    "forfiles",
    "wsl",
    "invoke-item",
    "ii",
    "mshta",
    "wscript",
    "cscript",
    "rundll32",
    "regsvr32",
    "msiexec",
    "wmic",
];

/// Delete verbs. As an argument of its own they are handed to another program
/// (`cargo clean`, `kubectl delete pod x`, `npm uninstall x`); in a command that cannot be
/// broken down they count wherever they stand as a word.
const DELETE_VERBS: &[&str] = &[
    "delete",
    "remove",
    "purge",
    "prune",
    "clean",
    "distclean",
    "uninstall",
    "destroy",
    "rmtree",
];

/// Delete verbs that count as the name of a flag: `--delete`, `-delete`, `/purge`,
/// `--remove-orphans`. Program names such as `rm` do not: `docker run --rm` deletes no file.
const DELETE_FLAGS: &[&str] = &[
    "delete", "remove", "purge", "prune", "erase", "wipe", "clean",
];

/// Words that publish in the user's name when handed to a program: `npm publish`,
/// `docker push`, `firebase deploy`.
const PUBLISH_VERBS: &[&str] = &["publish", "deploy", "upload", "push"];

/// Programs that talk to other machines (rule 8).
const NETWORK_PROGRAMS: &[&str] = &[
    "curl",
    "wget",
    "invoke-webrequest",
    "iwr",
    "irm",
    "invoke-restmethod",
    "nc",
    "ncat",
    "netcat",
    "socat",
    "ssh",
    "scp",
    "sftp",
    "ftp",
    "tftp",
    "lftp",
    "telnet",
    "rsync",
    "start-bitstransfer",
    "bitsadmin",
    "certutil",
    "gh",
    "aws",
    "az",
    "gcloud",
    "smbclient",
    "nslookup",
    "dig",
    "ping",
    "resolve-dnsname",
    "test-netconnection",
    "tnc",
    "test-connection",
    "sendmail",
    "mail",
    "send-mailmessage",
];

/// Git subcommands that talk to another machine.
const GIT_NETWORK: &[&str] = &["push", "fetch", "pull", "clone", "ls-remote", "submodule"];

/// File-removal calls of common languages, as they appear in inline code. Matched
/// anywhere in the lower-cased text.
const REMOVAL_CALLS: &[&str] = &[
    "os.remove",
    "os.unlink",
    "os.rmdir",
    "removedirs",
    "shutil.rmtree",
    "rmtree",
    "fs.rm",
    "fs.unlink",
    "rimraf",
    "file.delete",
    "directory.delete",
    "files.delete",
    "fileutils.rm",
    "unlink(",
    "unlinksync",
    ".rm(",
    "rmsync",
    ".rmdir(",
    "rmdirsync",
    ".delete(",
    "deletefile",
    "rm_rf",
    "removeall",
    "remove_file",
    "remove_dir",
    "emptydir",
    "send2trash",
];

/// Calls that turn encoded text back into something to run.
const DECODERS: &[&str] = &[
    "frombase64string",
    "b64decode",
    "base64decode",
    "decodebase64",
    "atob(",
];

/// For an interpreter or shell: the letters of the short flags that take code to run
/// (`python -c`, `perl -e`, `bash -c`). Flags can be bundled (`bash -lc`, `perl -pe`), so a
/// bundle that holds one of the letters counts. `None` for any other program.
fn code_flags(name: &str) -> Option<&'static str> {
    Some(match name {
        "sh" | "bash" | "zsh" | "dash" | "fish" | "ksh" | "ash" | "csh" | "tcsh" => "c",
        "py" => "c",
        n if n.starts_with("python") || n.starts_with("pypy") => "c",
        "node" | "nodejs" => "ep",
        "bun" | "deno" | "osascript" | "r" | "rscript" => "e",
        n if n.starts_with("ruby") || n.starts_with("perl") || n.starts_with("lua") => "e",
        n if n.starts_with("php") => "rbe",
        "pwsh" | "powershell" => "",
        _ => return None,
    })
}

/// What was found in one command line.
#[derive(Default)]
struct Found {
    delete: Option<String>,
    destroy: Option<String>,
    hidden: Option<String>,
    publish: Option<String>,
    sensitive: Option<String>,
    network: bool,
    opaque: bool,
}

impl Found {
    fn note(&mut self, kind: KeepAsking, why: impl FnOnce() -> String) {
        let slot = match kind {
            KeepAsking::Delete => &mut self.delete,
            KeepAsking::Destroy => &mut self.destroy,
            KeepAsking::Hidden => &mut self.hidden,
            KeepAsking::Publish => &mut self.publish,
            _ => &mut self.sensitive,
        };
        if slot.is_none() {
            *slot = Some(why());
        }
    }

    fn any(&self) -> bool {
        self.delete.is_some()
            || self.destroy.is_some()
            || self.hidden.is_some()
            || self.publish.is_some()
            || self.sensitive.is_some()
    }

    fn finish(self) -> CommandFacts {
        let keep_asking = None
            .or(self.delete.map(|why| (KeepAsking::Delete, why)))
            .or(self.destroy.map(|why| (KeepAsking::Destroy, why)))
            .or(self.hidden.map(|why| (KeepAsking::Hidden, why)))
            .or(self.publish.map(|why| (KeepAsking::Publish, why)))
            .or(self.sensitive.map(|why| (KeepAsking::Sensitive, why)));
        CommandFacts {
            keep_asking,
            network: self.network,
            opaque: self.opaque,
        }
    }
}

/// The Allow-all facts of a command line. `strict` is what [`crate::command::parse`]
/// made of it.
pub fn command_facts(shell: Shell, line: &str, strict: &Parsed) -> CommandFacts {
    // For this mode a PowerShell double-quoted string with nothing to expand is the
    // literal it is; on Windows almost every quoted argument is written that way.
    let lenient;
    let parsed = match (strict, shell) {
        (Parsed::Complex { .. }, Shell::PowerShell) => {
            lenient = parse_lenient(shell, line);
            &lenient
        }
        _ => strict,
    };
    let mut found = Found::default();
    let mut only_reads = false;
    match parsed {
        Parsed::Plain(commands) => {
            for command in commands {
                simple(command, &mut found);
            }
            only_reads = commands.iter().all(is_read_only);
        }
        Parsed::Complex { .. } => found.opaque = true,
    }
    if found.opaque {
        // Rule 3: the text is all there is to go by. Rule 8: it may reach the network.
        scan(line, &mut found);
        found.network = true;
    }
    if !only_reads && !found.any() {
        sensitive_place(line, &mut found);
    }
    found.finish()
}

/// One simple command of literal words.
fn simple(command: &SimpleCommand, found: &mut Found) {
    let name = program_name(&command.program);
    let name = name.as_str();
    let program = &command.program;
    let args: Vec<String> = command
        .args
        .iter()
        .map(|a| a.to_ascii_lowercase())
        .collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);

    if NETWORK_PROGRAMS.contains(&name) {
        found.network = true;
    }
    // Rules 1 and 2: the program itself.
    if DELETES.contains(&name) || MORE_DELETES.contains(&name) {
        found.note(KeepAsking::Delete, || format!("{program} deletes files"));
        return;
    }
    if ELEVATION.contains(&name) {
        found.note(KeepAsking::Destroy, || {
            format!("{program} runs a command with elevated rights")
        });
        return;
    }
    if SYSTEM.contains(&name) || MORE_SYSTEM.contains(&name) || name.starts_with("mkfs") {
        found.note(KeepAsking::Destroy, || {
            format!("{program} changes the disk or the system")
        });
        return;
    }
    if name.starts_with("remove-") || name.starts_with("uninstall-") {
        found.note(KeepAsking::Delete, || format!("{program} removes things"));
        return;
    }
    if name == "find" {
        if has("-delete") {
            found.note(KeepAsking::Delete, || "find -delete deletes files".into());
        }
        if args
            .iter()
            .any(|a| matches!(a.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir"))
        {
            found.opaque = true;
        }
        return;
    }
    if name == "git" {
        git(command, &args, found);
        return;
    }
    // Rule 3: a program that runs another command, or code given on the command line.
    if WRAPPERS.contains(&name) || MORE_WRAPPERS.contains(&name) {
        found.opaque = true;
        return;
    }
    let inline_flag = INLINE.iter().any(|(interp, flags)| {
        (name == *interp
            || name.starts_with(&format!("{interp}3"))
            || name.starts_with(&format!("{interp}2")))
            && flags.iter().any(|f| has(f))
    });
    if inline_flag {
        found.opaque = true;
    }
    if let Some(letters) = code_flags(name) {
        // Code on the command line, also in a bundle of short flags.
        let bundled = args.iter().any(|a| {
            a.starts_with('-') && !a.starts_with("--") && a.contains(|c| letters.contains(c))
        });
        // No file to run: the code comes from standard input (`… | sh`).
        let no_file = !args.iter().any(|a| !a.starts_with('-'));
        if bundled || no_file {
            found.opaque = true;
        }
        if matches!(name, "powershell" | "pwsh") {
            powershell_host(program, &args, found);
        }
    }
    if name == "robocopy" && args.iter().any(|a| matches!(a.as_str(), "/mir" | "/purge")) {
        found.note(KeepAsking::Delete, || {
            "robocopy with /MIR or /PURGE deletes files in the destination".into()
        });
    }
    if is_read_only(command) {
        return;
    }
    // A delete, publish or network word handed to this program as an argument of its own.
    for arg in &args {
        if arg.contains(char::is_whitespace) {
            // Several words in one argument: a command or code for the program to run
            // (`ssh host "rm -rf x"`, `forfiles /c "cmd /c del @path"`).
            scan(arg, found);
            if arg
                .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
                .any(|word| NETWORK_PROGRAMS.contains(&strip_extension(word)))
                || arg.contains("://")
            {
                found.network = true;
            }
            continue;
        }
        if let Some(flag) = flag_name(arg) {
            if DELETE_FLAGS.iter().any(|verb| names_verb(flag, verb)) {
                found.note(KeepAsking::Delete, || {
                    format!("{program} is given {arg}, which deletes")
                });
            }
            continue;
        }
        let word = arg.as_str();
        // A delete program may be named by its path (`npx ./node_modules/.bin/rimraf`).
        let base = program_name(word);
        if DELETES.contains(&base.as_str())
            || MORE_DELETES.contains(&base.as_str())
            || DELETE_VERBS.contains(&word)
        {
            found.note(KeepAsking::Delete, || {
                format!("{program} is given \"{arg}\", which deletes")
            });
        } else if matches!(word, "/dev/null" | "nul") {
            found.note(KeepAsking::Delete, || {
                format!("{program} is given {arg}, which discards or empties a file")
            });
        } else if PUBLISH_VERBS.contains(&word) {
            found.note(KeepAsking::Publish, || {
                format!("{program} {arg} publishes in your name")
            });
            found.network = true;
        } else if NETWORK_PROGRAMS.contains(&base.as_str()) || word.contains("://") {
            found.network = true;
        }
    }
}

/// `--delete` → `delete`, `/purge` → `purge`; `None` for anything that is not a flag.
fn flag_name(arg: &str) -> Option<&str> {
    if let Some(rest) = arg.strip_prefix('-') {
        let rest = rest.trim_start_matches('-');
        return (!rest.is_empty()).then_some(rest);
    }
    // A Windows switch: one slash and a word, not a path.
    let rest = arg.strip_prefix('/')?;
    (!rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphabetic())).then_some(rest)
}

/// Whether a flag's name is `verb` or starts with it as a word (`delete-after`,
/// `remove-orphans`, `delete=1`).
fn names_verb(flag: &str, verb: &str) -> bool {
    flag.strip_prefix(verb)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '=', ':']))
}

/// `powershell` or `pwsh` as a program: an encoded command hides what runs; a command
/// given as text is not broken down here.
fn powershell_host(program: &str, args: &[String], found: &mut Found) {
    let mut runs_file = false;
    for arg in args {
        let Some(flag) = arg.strip_prefix('-') else {
            continue;
        };
        if flag.is_empty() {
            continue;
        }
        if flag == "ec" || "encodedcommand".starts_with(flag) {
            found.note(KeepAsking::Hidden, || {
                format!("{program} is given an encoded command, which hides what it runs")
            });
            found.opaque = true;
        } else if "command".starts_with(flag) {
            found.opaque = true;
        } else if "file".starts_with(flag) {
            runs_file = true;
        }
    }
    // Without -File, the first argument is the command to run.
    if !runs_file {
        found.opaque = true;
    }
}

/// `git`, whose subcommands are known.
fn git(command: &SimpleCommand, args: &[String], found: &mut Found) {
    // The subcommand, after git's own options. Options are case-sensitive: -c sets
    // configuration (which can name programs to run), -C changes folder.
    let mut i = 0;
    while i < command.args.len() {
        let a = command.args[i].as_str();
        if a == "-c"
            || (a.starts_with("-c") && a.len() > 2)
            || a.starts_with("--config-env")
            || a.starts_with("--exec-path")
        {
            found.opaque = true;
            return;
        }
        if a == "-C" || a == "--git-dir" || a == "--work-tree" || a == "--namespace" {
            i += 2;
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        break;
    }
    let sub = args.get(i).map(String::as_str).unwrap_or("");
    let rest = args.get(i + 1..).unwrap_or(&[]);
    let rest_exact = command.args.get(i + 1..).unwrap_or(&[]);
    let has = |flag: &str| rest.iter().any(|a| a == flag);
    if GIT_NETWORK.contains(&sub) {
        found.network = true;
    }
    // Options and subcommands that hand git a command to run.
    let runs_a_command = rest_exact.iter().any(|a| {
        ["--upload-pack", "--receive-pack", "--exec", "--config"]
            .iter()
            .any(|flag| a.starts_with(flag))
            || (a == "-x" && sub == "rebase")
            || (a == "-c" && sub == "clone")
            || (a == "-u" && sub == "clone")
    }) || (sub == "bisect" && rest.first().map(String::as_str) == Some("run"))
        || (sub == "submodule" && has("foreach"));
    if runs_a_command {
        found.opaque = true;
    }
    match sub {
        "push" => {
            let forced = rest.iter().any(|a| {
                a.starts_with("--force")
                    || a == "--mirror"
                    || a == "--delete"
                    || a == "--prune"
                    || a.starts_with('+')
                    || a.starts_with(':')
            }) || rest_exact
                .iter()
                .any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains(['f', 'd']));
            if forced {
                found.note(KeepAsking::Destroy, || {
                    "git push with --force or --delete can discard commits on the other computer"
                        .into()
                });
            } else {
                found.note(KeepAsking::Publish, || {
                    "git push sends commits to another computer".into()
                });
            }
        }
        "clean" => found.note(KeepAsking::Delete, || "git clean deletes files".into()),
        "rm" => found.note(KeepAsking::Delete, || "git rm deletes files".into()),
        "reset" if has("--hard") => found.note(KeepAsking::Destroy, || {
            "git reset --hard discards changes".into()
        }),
        "checkout" if has("--force") || has("-f") || has("--") || has(".") => {
            found.note(KeepAsking::Destroy, || {
                "git checkout of files discards changes that are not committed".into()
            });
        }
        "restore" if !staged_only(rest_exact) => found.note(KeepAsking::Destroy, || {
            "git restore discards changes that are not committed".into()
        }),
        "switch" if has("--discard-changes") || has("--force") || has("-f") => {
            found.note(KeepAsking::Destroy, || {
                "git switch with --discard-changes throws away changes that are not committed"
                    .into()
            });
        }
        "stash" if matches!(rest.first().map(String::as_str), Some("drop" | "clear")) => {
            found.note(KeepAsking::Destroy, || {
                "git stash drop and clear discard stashed work".into()
            });
        }
        "branch" if rest_exact.iter().any(|a| a == "-D") || (has("--delete") && has("--force")) => {
            found.note(KeepAsking::Destroy, || {
                "git branch -D deletes a branch that may not be merged".into()
            });
        }
        "worktree" if rest.first().map(String::as_str) == Some("remove") => {
            found.note(KeepAsking::Delete, || {
                "git worktree remove deletes a folder".into()
            });
        }
        "config" if config_writes(rest) => found.note(KeepAsking::Sensitive, || {
            "git config changes git's configuration".into()
        }),
        "" => {}
        other if !GIT_KNOWN.contains(&other) => found.opaque = true,
        _ => {}
    }
}

/// `git restore` that touches the index only (`--staged` or `-S` without `--worktree`).
fn staged_only(rest: &[String]) -> bool {
    let staged = rest.iter().any(|a| a == "--staged" || a == "-S");
    let worktree = rest.iter().any(|a| a == "--worktree" || a == "-W");
    staged && !worktree
}

/// Whether `git config …` sets or removes a value (as opposed to showing one).
fn config_writes(rest: &[String]) -> bool {
    const READS: &[&str] = &[
        "--get",
        "--get-all",
        "--get-regexp",
        "--get-urlmatch",
        "--list",
        "-l",
        "get",
        "list",
    ];
    const WRITES: &[&str] = &[
        "--add",
        "--unset",
        "--unset-all",
        "--replace-all",
        "--rename-section",
        "--remove-section",
        "--edit",
        "-e",
        "set",
        "unset",
        "edit",
        "rename-section",
        "remove-section",
    ];
    if rest.iter().any(|a| WRITES.contains(&a.as_str())) {
        return true;
    }
    if rest.iter().any(|a| READS.contains(&a.as_str())) {
        return false;
    }
    // `git config name value` sets; `git config name` shows.
    rest.iter().filter(|a| !a.starts_with('-')).count() >= 2
}

/// The program name a token would be: no leading dashes, no Windows extension.
fn strip_extension(token: &str) -> &str {
    for ext in [".exe", ".cmd", ".bat", ".com", ".ps1"] {
        if let Some(stripped) = token.strip_suffix(ext) {
            return stripped;
        }
    }
    token
}

/// Rule 3: the words of a command that cannot be fully broken down. Case-insensitive, on
/// word boundaries: a word is a run of letters, digits, `_`, `-` and `.`, so `Remove-Item`
/// and `mkfs.ext4` are one word each and `removed`, `Format-Table` and `rmdir-test` do
/// not match `remove`, `format` and `rmdir`.
fn scan(line: &str, found: &mut Found) {
    scan_text(&line.to_lowercase(), found);
    // Quotes and escapes inside a word do not change what runs (`r''m`, `r\m`, `d^el`,
    // PowerShell's backtick): the same words are looked for with them taken away.
    let unquoted: String = line
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\' | '`' | '^'))
        .collect();
    scan_text(&unquoted, found);
}

fn scan_text(lower: &str, found: &mut Found) {
    for call in REMOVAL_CALLS {
        if lower.contains(call) {
            found.note(KeepAsking::Delete, || {
                format!("the command contains \"{call}\", which deletes files")
            });
        }
    }
    for call in DECODERS {
        if lower.contains(call) {
            found.note(KeepAsking::Hidden, || {
                format!("the command decodes hidden text (\"{call}\") and may run it")
            });
        }
    }
    let (mut git, mut push, mut clean, mut base64, mut decode, mut ps_host) =
        (false, false, false, false, false, false);
    let mut encoded_flag = false;
    for raw in lower
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
        .filter(|t| !t.is_empty())
    {
        if raw == "--hard" {
            found.note(KeepAsking::Destroy, || {
                "the command contains \"--hard\" (git reset --hard discards changes)".into()
            });
        }
        // `.format(…)`: a method of something, never a program.
        let method = raw.starts_with('.');
        let token = raw.trim_matches('.');
        if let Some(flag) = token.strip_prefix('-') {
            let flag = flag.trim_start_matches('-');
            if DELETE_FLAGS.iter().any(|verb| names_verb(flag, verb)) {
                found.note(KeepAsking::Delete, || {
                    format!("the command contains \"{token}\", which deletes")
                });
            }
            if matches!(flag, "d" | "decode" | "decodehex") {
                decode = true;
            }
            if flag == "ec" || (!flag.is_empty() && "encodedcommand".starts_with(flag)) {
                encoded_flag = true;
            }
            continue;
        }
        let word = strip_extension(token);
        if DELETES.contains(&word)
            || MORE_DELETES.contains(&word)
            || DELETE_VERBS.contains(&word)
            || word.starts_with("remove-")
            || word.starts_with("uninstall-")
        {
            found.note(KeepAsking::Delete, || {
                format!("the command contains \"{token}\", which deletes")
            });
        }
        if !method
            && (ELEVATION.contains(&word)
                || SYSTEM.contains(&word)
                || MORE_SYSTEM.contains(&word)
                || word.starts_with("mkfs"))
        {
            found.note(KeepAsking::Destroy, || {
                format!("the command contains \"{token}\", which changes the disk or the system or runs with elevated rights")
            });
        }
        match word {
            "git" => git = true,
            "push" => push = true,
            "clean" => clean = true,
            "base64" | "certutil" => base64 = true,
            "powershell" | "pwsh" => ps_host = true,
            "publish" | "deploy" | "upload" => found.note(KeepAsking::Publish, || {
                format!("the command contains \"{token}\", which publishes in your name")
            }),
            _ => {}
        }
    }
    if push {
        found.note(KeepAsking::Publish, || {
            "the command contains \"push\", which sends your work to another computer".into()
        });
    }
    if git && clean {
        found.note(KeepAsking::Delete, || "git clean deletes files".into());
    }
    if (base64 && decode) || (ps_host && encoded_flag) {
        found.note(KeepAsking::Hidden, || {
            "the command decodes hidden text and may run it".into()
        });
    }
}

/// Rule 4 for commands: the line names a place that the file tools treat as sensitive
/// (the rules of [`always_ask_reason`]), or PowerShell's profile.
fn sensitive_place(line: &str, found: &mut Found) {
    let lower = line.to_lowercase().replace('\\', "/");
    if lower.contains("$profile") {
        found.note(KeepAsking::Sensitive, || {
            "the command names $PROFILE, PowerShell's start-up file".into()
        });
        return;
    }
    if lower.contains("start menu/programs/startup") || lower.contains("shell:startup") {
        found.note(KeepAsking::Sensitive, || {
            "the command names the Windows Startup folder".into()
        });
        return;
    }
    const FOLDERS: &[&str] = &[".git", ".ssh", ".gnupg", "launchagents", "launchdaemons"];
    for word in lower.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\'' | ';' | '|' | '&' | '<' | '>' | '(' | ')' | '=' | ',' | '`'
            )
    }) {
        let word = word.trim_end_matches('/');
        if word.is_empty() {
            continue;
        }
        let last = word.rsplit('/').next().unwrap_or(word);
        let why = always_ask_reason(word, WriteKind::Edit, Flavor::Posix).or_else(|| {
            FOLDERS
                .contains(&last)
                .then_some("it is a git, ssh, gpg or autostart folder")
        });
        if let Some(why) = why {
            found.note(KeepAsking::Sensitive, || {
                format!("the command names {word}: {why}")
            });
            return;
        }
    }
}

/// Labels that spend money or send or publish something in the user's name. Matched as
/// whole words or phrases, like the browser's own list (`browser::intent`).
const PUBLISH_LABELS: &[&str] = &[
    "pay",
    "pay now",
    "buy",
    "buy now",
    "purchase",
    "order",
    "place order",
    "checkout",
    "check out",
    "transfer",
    "send",
    "post",
    "publish",
    // Lead's additions, on the same theme.
    "donate",
    "withdraw",
    "subscribe",
    "book",
    "reserve",
    "reply",
    "share",
    "authorize",
    "authorise",
    "approve",
];

/// Labels that destroy something.
const DESTROY_LABELS: &[&str] = &["delete", "remove"];

fn label_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn label_has(label: &[String], phrases: &[&str]) -> bool {
    phrases.iter().any(|phrase| {
        let words = label_words(phrase);
        label
            .windows(words.len())
            .any(|window| window == words.as_slice())
    })
}

/// Why a click on a web page control with this label still asks in Allow all, if it
/// does: the label says it spends money, destroys something or publishes (rule 5).
/// Ordinary submissions ("Submit", "Confirm", "Search", "Next", "Log in", "Sign up") do
/// not.
pub fn web_label(label: &str) -> Option<KeepAsking> {
    let words = label_words(label);
    if words.is_empty() {
        return None;
    }
    if label_has(&words, DESTROY_LABELS) {
        Some(KeepAsking::Destroy)
    } else if label_has(&words, PUBLISH_LABELS) {
        Some(KeepAsking::Publish)
    } else {
        None
    }
}

/// Why a call of an external tool still asks in Allow all, if it does: its server marks
/// it destructive, or the user marked it always-ask (the two things that make it
/// always-ask in Ask mode, see [`crate::external_risk`]).
pub fn external(destructive_hint: Option<bool>, user_always_ask: bool) -> Option<KeepAsking> {
    (user_always_ask || destructive_hint == Some(true)).then_some(KeepAsking::MarkedDestructive)
}

#[cfg(test)]
mod tests;
