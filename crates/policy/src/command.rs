//! The terminal classifier (ARCHITECTURE.md 13.3, decision D5). It fails closed.
//!
//! Commands are parsed with real grammars (tree-sitter bash for zsh/bash, tree-sitter
//! PowerShell). A command can be covered by a grant only if it parses without errors
//! into a plain sequence of simple commands made of literal words. Anything else —
//! redirections, variables, command substitution, subshells, globs, the call operator,
//! script blocks — asks every time.
//!
//! This is a guard against accidents and obvious abuse, not a security boundary: there
//! is no OS sandbox in v1, and the UI says so when the terminal is turned on.

use tree_sitter::{Node, Parser};

use crate::engine::KeepAsking;

/// Which grammar to parse with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    /// zsh on macOS, parsed with the bash grammar (zsh-only syntax is rejected).
    Posix,
    /// Windows PowerShell 5.1 or PowerShell 7.
    PowerShell,
}

/// One simple command: the program as written and its literal arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// The parse result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// A plain sequence (`;`, `&&`, `||`, `|`) of simple commands of literal words.
    Plain(Vec<SimpleCommand>),
    /// Anything else; `reason` is shown on the card.
    Complex { reason: String },
}

/// What policy needs to know about a command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub parsed: Parsed,
    /// Set when the command always asks, whatever was granted.
    pub always_ask: Option<String>,
    /// Every simple command is on the read-only list.
    pub read_only: bool,
    /// Allow all (0.0.4): why the command still asks while its chat is in Allow all, if it
    /// does, with a sentence that names what was found.
    pub keep_asking: Option<(KeepAsking, String)>,
    /// Allow all (0.0.4): the command names a network program or cannot be fully broken
    /// down, so it asks once untrusted content has entered the turn.
    pub network: bool,
}

const BASH_KINDS: &[&str] = &[
    "program",
    "list",
    "pipeline",
    "command",
    "command_name",
    "word",
    "string",
    "string_content",
    "raw_string",
    "number",
];
const BASH_TOKENS: &[&str] = &["&&", "||", ";", "|", "\"", "'", "\n"];

const PS_KINDS: &[&str] = &[
    "program",
    "statement_list",
    "pipeline",
    "pipeline_chain",
    "command",
    "command_name",
    "command_elements",
    "command_argument_sep",
    "command_parameter",
    "generic_token",
    "empty_statement",
    "array_literal_expression",
    "unary_expression",
    "string_literal",
    "verbatim_string_characters",
];
const PS_TOKENS: &[&str] = &[";", "|", "&&", "||", " ", "\n"];

fn complex(reason: impl Into<String>) -> Parsed {
    Parsed::Complex {
        reason: reason.into(),
    }
}

/// Parses `line` with the grammar for `shell`.
pub fn parse(shell: Shell, line: &str) -> Parsed {
    if line.trim().is_empty() {
        return complex("the command is empty");
    }
    if line.len() > 16 * 1024 {
        return complex("the command is too long to check");
    }
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = match shell {
        Shell::Posix => tree_sitter_bash::LANGUAGE.into(),
        Shell::PowerShell => tree_sitter_powershell::LANGUAGE.into(),
    };
    if parser.set_language(&lang).is_err() {
        return complex("the shell grammar could not be loaded");
    }
    let Some(tree) = parser.parse(line, None) else {
        return complex("the command could not be parsed");
    };
    let root = tree.root_node();
    if root.has_error() {
        return complex("the command could not be fully parsed");
    }
    match shell {
        Shell::Posix => posix(root, line),
        Shell::PowerShell => powershell(root, line),
    }
}

fn text<'a>(n: Node, src: &'a str) -> &'a str {
    &src[n.byte_range()]
}

/// Checks every node against the allowlist.
fn all_allowed(n: Node, src: &str, kinds: &[&str], tokens: &[&str]) -> Result<(), String> {
    if n.is_error() || n.is_missing() {
        return Err("the command could not be fully parsed".into());
    }
    let k = n.kind();
    let ok = if n.is_named() {
        kinds.contains(&k)
    } else {
        tokens.contains(&k) || tokens.contains(&text(n, src))
    };
    if !ok {
        return Err(describe_kind(k));
    }
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        all_allowed(ch, src, kinds, tokens)?;
    }
    Ok(())
}

fn describe_kind(k: &str) -> String {
    let what = match k {
        "redirected_statement"
        | "file_redirect"
        | "heredoc_redirect"
        | "herestring_redirect"
        | "redirection" => "a redirection",
        "simple_expansion" | "expansion" | "variable" | "variable_name" => "a variable",
        "command_substitution" | "sub_expression" | "process_substitution" => {
            "command substitution"
        }
        "subshell" | "script_block" | "compound_statement" => "a subshell or script block",
        "command_invokation_operator" => "the call operator",
        "concatenation" => "joined words",
        "function_definition" | "function_statement" => "a function definition",
        "if_statement" | "while_statement" | "for_statement" | "case_statement"
        | "foreach_statement" => "a control structure",
        "variable_assignment" | "assignment_expression" => "a variable assignment",
        "expandable_string_literal" => "a string that expands variables",
        "comment" => "a comment",
        _ => return format!("shell syntax the checker does not allow ({k})"),
    };
    format!("the command uses {what}")
}

/// A literal word: no globs, expansions, escapes or home-directory tilde.
fn literal_word(w: &str) -> Option<String> {
    if w.is_empty()
        || w.starts_with('~')
        || w.contains(['*', '?', '[', '$', '`', '\\', '{', '}', '!'])
    {
        return None;
    }
    Some(w.to_string())
}

fn posix(root: Node, src: &str) -> Parsed {
    if let Err(why) = all_allowed(root, src, BASH_KINDS, BASH_TOKENS) {
        return complex(why);
    }
    let mut commands = Vec::new();
    let mut stack = vec![root];
    let mut found: Vec<Node> = Vec::new();
    while let Some(n) = stack.pop() {
        if n.kind() == "command" {
            found.push(n);
            continue;
        }
        let mut c = n.walk();
        let kids: Vec<Node> = n.children(&mut c).collect();
        stack.extend(kids.into_iter().rev());
    }
    for cmd in found {
        let mut words = Vec::new();
        let mut c = cmd.walk();
        for ch in cmd.named_children(&mut c) {
            let w = match ch.kind() {
                "command_name" => {
                    let mut cc = ch.walk();
                    let inner: Vec<Node> = ch.named_children(&mut cc).collect();
                    match inner.as_slice() {
                        [only] if only.kind() == "word" => literal_word(text(*only, src)),
                        _ => None,
                    }
                }
                "word" | "number" => literal_word(text(ch, src)),
                "raw_string" => {
                    let t = text(ch, src);
                    Some(
                        t.trim_start_matches('\'')
                            .trim_end_matches('\'')
                            .to_string(),
                    )
                }
                "string" => {
                    // Only plain content between double quotes.
                    let mut sc = ch.walk();
                    let parts: Vec<Node> = ch.named_children(&mut sc).collect();
                    if parts.iter().all(|p| p.kind() == "string_content") {
                        let t: String = parts.iter().map(|p| text(*p, src)).collect();
                        if t.contains(['$', '`', '\\']) {
                            None
                        } else {
                            Some(t)
                        }
                    } else {
                        None
                    }
                }
                _ => None,
            };
            match w {
                Some(w) => words.push(w),
                None => {
                    return complex(format!(
                        "the word \"{}\" is not a plain literal",
                        text(ch, src)
                    ));
                }
            }
        }
        let Some((program, args)) = words.split_first() else {
            return complex("a command has no program");
        };
        commands.push(SimpleCommand {
            program: program.clone(),
            args: args.to_vec(),
        });
    }
    if commands.is_empty() {
        return complex("no command found");
    }
    Parsed::Plain(commands)
}

fn powershell(root: Node, src: &str) -> Parsed {
    if let Err(why) = all_allowed(root, src, PS_KINDS, PS_TOKENS) {
        return complex(why);
    }
    let mut found: Vec<Node> = Vec::new();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.kind() == "command" {
            found.push(n);
            continue;
        }
        let mut c = n.walk();
        let kids: Vec<Node> = n.children(&mut c).collect();
        stack.extend(kids.into_iter().rev());
    }
    let mut commands = Vec::new();
    for cmd in found {
        let mut program = None;
        let mut args = Vec::new();
        let mut c = cmd.walk();
        for ch in cmd.named_children(&mut c) {
            match ch.kind() {
                "command_name" => {
                    let t = text(ch, src);
                    if t.contains(['$', '`', '(', '@', '{']) {
                        return complex(format!("the command name \"{t}\" is not a plain word"));
                    }
                    program = Some(t.to_string());
                }
                "command_elements" => {
                    let mut ec = ch.walk();
                    for el in ch.named_children(&mut ec) {
                        let t = text(el, src);
                        let w = match el.kind() {
                            "command_argument_sep" => continue,
                            "command_parameter" | "generic_token" => {
                                if t.contains(['$', '`', '(', '@', '{', '*', '?', '[']) {
                                    None
                                } else {
                                    Some(t.to_string())
                                }
                            }
                            "array_literal_expression" => single_quoted(el, src),
                            _ => None,
                        };
                        match w {
                            Some(w) => args.push(w),
                            None => {
                                return complex(format!(
                                    "the argument \"{t}\" is not a plain literal"
                                ));
                            }
                        }
                    }
                }
                _ => return complex(describe_kind(ch.kind())),
            }
        }
        let Some(program) = program else {
            return complex("a command has no program");
        };
        commands.push(SimpleCommand { program, args });
    }
    if commands.is_empty() {
        return complex("no command found");
    }
    Parsed::Plain(commands)
}

/// `'text'` wrapped as array_literal_expression > unary_expression > string_literal.
fn single_quoted(n: Node, src: &str) -> Option<String> {
    let mut cur = n;
    for want in [
        "unary_expression",
        "string_literal",
        "verbatim_string_characters",
    ] {
        let mut c = cur.walk();
        let kids: Vec<Node> = cur.named_children(&mut c).collect();
        match kids.as_slice() {
            [only] if only.kind() == want => cur = *only,
            _ => return None,
        }
    }
    let t = text(cur, src);
    Some(
        t.trim_start_matches('\'')
            .trim_end_matches('\'')
            .replace("''", "'"),
    )
}

/// The program's base name, lower-cased, without a Windows extension.
pub fn program_name(program: &str) -> String {
    let base = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".com", ".ps1"] {
        if let Some(stripped) = base.strip_suffix(ext) {
            return stripped.to_string();
        }
    }
    base
}

const DELETES: &[&str] = &[
    "rm",
    "unlink",
    "rmdir",
    "shred",
    "del",
    "erase",
    "rd",
    "remove-item",
    "ri",
    "srm",
    "trash",
    "wipe",
];
const ELEVATION: &[&str] = &["sudo", "doas", "su", "runas", "pkexec", "gsudo"];
const SYSTEM: &[&str] = &[
    "format",
    "diskpart",
    "dd",
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
    "reg",
    "regedit",
    "bcdedit",
    "fdisk",
    "parted",
    "diskutil",
    "format-volume",
    "clear-disk",
    "initialize-disk",
    "restart-computer",
    "stop-computer",
    "set-itemproperty",
    "new-itemproperty",
    "remove-itemproperty",
    "launchctl",
    "systemctl",
    "schtasks",
    "crontab",
    "chown",
    "csrutil",
    "sc",
    "takeown",
    "icacls",
    "cipher",
    "vssadmin",
    "wbadmin",
];
const WRAPPERS: &[&str] = &[
    "env",
    "xargs",
    "nohup",
    "nice",
    "ionice",
    "timeout",
    "time",
    "watch",
    "exec",
    "eval",
    "cmd",
    "start-process",
    "saps",
    "start",
    "invoke-expression",
    "iex",
    "invoke-command",
    "icm",
    "command",
    "builtin",
    "busybox",
    "script",
    "stdbuf",
    "setsid",
    "chroot",
    "flock",
    "parallel",
    "caffeinate",
    "open",
];
/// Interpreter → flags that take inline code.
const INLINE: &[(&str, &[&str])] = &[
    ("python", &["-c"]),
    ("python3", &["-c"]),
    ("py", &["-c"]),
    ("node", &["-e", "--eval", "-p", "--print"]),
    ("deno", &["eval"]),
    ("bun", &["-e", "--eval"]),
    ("ruby", &["-e"]),
    ("perl", &["-e", "-E"]),
    ("php", &["-r"]),
    ("bash", &["-c"]),
    ("sh", &["-c"]),
    ("zsh", &["-c"]),
    ("fish", &["-c"]),
    ("dash", &["-c"]),
    ("pwsh", &["-c", "-command", "-encodedcommand", "-ec", "-e"]),
    (
        "powershell",
        &["-c", "-command", "-encodedcommand", "-ec", "-e"],
    ),
    ("osascript", &["-e"]),
    ("lua", &["-e"]),
    ("r", &["-e"]),
    ("rscript", &["-e"]),
];
const GIT_READ: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "rev-parse",
    "ls-files",
    "blame",
    "describe",
    "shortlog",
];
const GIT_KNOWN: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "rev-parse",
    "ls-files",
    "blame",
    "describe",
    "shortlog",
    "add",
    "commit",
    "branch",
    "checkout",
    "switch",
    "restore",
    "merge",
    "rebase",
    "pull",
    "fetch",
    "clone",
    "init",
    "stash",
    "tag",
    "remote",
    "config",
    "cherry-pick",
    "revert",
    "mv",
    "grep",
    "worktree",
    "submodule",
    "bisect",
    "notes",
    "apply",
    "am",
    "format-patch",
    "push",
    "reset",
    "clean",
    "rm",
    "gc",
    "reflog",
];
/// Programs that only read (granted per program; path arguments held to approved folders).
const READ_ONLY: &[&str] = &[
    "ls",
    "dir",
    "cat",
    "type",
    "head",
    "tail",
    "wc",
    "grep",
    "rg",
    "egrep",
    "fgrep",
    "find",
    "pwd",
    "echo",
    "which",
    "where",
    "whereis",
    "file",
    "stat",
    "du",
    "df",
    "tree",
    "sort",
    "uniq",
    "diff",
    "cmp",
    "less",
    "more",
    "get-childitem",
    "gci",
    "get-content",
    "gc",
    "select-string",
    "sls",
    "get-location",
    "gl",
    "test-path",
    "get-item",
    "gi",
    "measure-object",
    "measure",
    "get-command",
    "gcm",
    "resolve-path",
    "rvpa",
    "get-date",
    "whoami",
    "hostname",
    "uname",
    "date",
    "printf",
    "basename",
    "dirname",
    "realpath",
    "readlink",
    "cut",
    "tr",
    "nl",
    "md5sum",
    "sha256sum",
    "shasum",
    "get-filehash",
];

/// Why this simple command always asks, if it does.
pub fn always_ask(cmd: &SimpleCommand) -> Option<String> {
    let name = program_name(&cmd.program);
    let args_lower: Vec<String> = cmd.args.iter().map(|a| a.to_ascii_lowercase()).collect();
    let has = |flag: &str| args_lower.iter().any(|a| a == flag);
    if DELETES.contains(&name.as_str()) {
        return Some(format!("{} deletes files", cmd.program));
    }
    if ELEVATION.contains(&name.as_str()) {
        return Some(format!(
            "{} runs a command with elevated rights",
            cmd.program
        ));
    }
    if SYSTEM.contains(&name.as_str()) || name.starts_with("mkfs") {
        return Some(format!("{} changes the disk or the system", cmd.program));
    }
    if WRAPPERS.contains(&name.as_str()) {
        return Some(format!("{} runs another command", cmd.program));
    }
    if name == "find"
        && args_lower.iter().any(|a| {
            matches!(
                a.as_str(),
                "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir"
            )
        })
    {
        return Some("find with -delete or -exec changes or runs things".into());
    }
    for (interp, flags) in INLINE {
        if (name == *interp
            || name.starts_with(&format!("{interp}3"))
            || name.starts_with(&format!("{interp}2")))
            && flags.iter().any(|f| has(f))
        {
            return Some(format!("{} runs inline code", cmd.program));
        }
    }
    if name == "git" {
        // Git's options are case-sensitive: -c sets config (can run programs), -C changes
        // directory.
        let mut i = 0;
        while i < cmd.args.len() {
            let a = cmd.args[i].as_str();
            if a == "-c"
                || (a.starts_with("-c") && a.len() > 2)
                || a.starts_with("--config-env")
                || a.starts_with("--exec-path")
            {
                return Some("git with -c can run arbitrary programs".into());
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
        let sub = args_lower.get(i).map(String::as_str).unwrap_or("");
        match sub {
            "push" => return Some("git push sends commits to another computer".into()),
            "clean" => return Some("git clean deletes files".into()),
            "rm" => return Some("git rm deletes files".into()),
            "reset" if args_lower.iter().any(|a| a == "--hard") => {
                return Some("git reset --hard discards changes".into());
            }
            "" => {}
            s if !GIT_KNOWN.contains(&s) => {
                return Some(format!(
                    "\"git {s}\" may be an alias, which can run any command"
                ));
            }
            _ => {}
        }
    }
    None
}

/// Whether this simple command only reads.
pub fn is_read_only(cmd: &SimpleCommand) -> bool {
    let name = program_name(&cmd.program);
    if always_ask(cmd).is_some() {
        return false;
    }
    if name == "git" {
        // The subcommand after any -C <dir> / --git-dir <dir> options.
        let mut i = 0;
        while i < cmd.args.len() && cmd.args[i].starts_with('-') {
            i += if matches!(
                cmd.args[i].as_str(),
                "-C" | "--git-dir" | "--work-tree" | "--namespace"
            ) {
                2
            } else {
                1
            };
        }
        return cmd
            .args
            .get(i)
            .is_some_and(|s| GIT_READ.contains(&s.as_str()));
    }
    READ_ONLY.contains(&name.as_str())
}

/// Classifies a command line for the policy engine.
pub fn classify(shell: Shell, line: &str) -> Verdict {
    let parsed = parse(shell, line);
    match &parsed {
        Parsed::Plain(cmds) => {
            let always_ask = cmds.iter().find_map(always_ask);
            let read_only = always_ask.is_none() && cmds.iter().all(is_read_only);
            Verdict {
                parsed,
                always_ask,
                read_only,
                keep_asking: None,
                network: false,
            }
        }
        Parsed::Complex { reason } => Verdict {
            always_ask: Some(format!(
                "{reason}, so it cannot be checked and asks every time"
            )),
            parsed,
            read_only: false,
            keep_asking: None,
            network: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(shell: Shell, line: &str) -> Vec<SimpleCommand> {
        match parse(shell, line) {
            Parsed::Plain(c) => c,
            Parsed::Complex { reason } => panic!("{line:?} was complex: {reason}"),
        }
    }

    fn is_complex(shell: Shell, line: &str) -> bool {
        matches!(parse(shell, line), Parsed::Complex { .. })
    }

    #[test]
    fn plain_posix_sequences() {
        let c = plain(
            Shell::Posix,
            "ls -la /tmp && git status | head -n 3; echo 'a b' \"c d\"",
        );
        let names: Vec<&str> = c.iter().map(|c| c.program.as_str()).collect();
        assert_eq!(names, vec!["ls", "git", "head", "echo"]);
        assert_eq!(c[3].args, vec!["a b", "c d"]);
        assert_eq!(c[2].args, vec!["-n", "3"]);
    }

    #[test]
    fn plain_powershell_sequences() {
        let c = plain(
            Shell::PowerShell,
            "Get-ChildItem -Path C:\\x -Recurse; git status | Select-String 'mod ified'",
        );
        assert_eq!(c[0].program, "Get-ChildItem");
        assert_eq!(c[0].args, vec!["-Path", "C:\\x", "-Recurse"]);
        assert_eq!(c[2].args, vec!["mod ified"]);
    }

    #[test]
    fn anything_not_plain_is_complex() {
        for line in [
            "echo $HOME",
            "echo \"$HOME\"",
            "ls > out.txt",
            "cat < in.txt",
            "echo `whoami`",
            "echo $(whoami)",
            "(cd /tmp && ls)",
            "ls *.rs",
            "ls ~/x",
            "FOO=1 ls",
            "f() { ls; }",
            "if true; then ls; fi",
            "cat <<EOF\nx\nEOF",
            "ls # comment",
            "echo a\\ b",
            "ls {a,b}",
            "",
            "ls &&",
        ] {
            assert!(is_complex(Shell::Posix, line), "{line:?} should be complex");
        }
        for line in [
            "echo $env:PATH",
            "& notepad x",
            "Get-Content x > y",
            "Write-Output \"$x\"",
            "Get-ChildItem | ForEach-Object { $_ }",
            "Invoke-Expression (Get-Content x)",
            "Get-ChildItem *.txt",
            "$a = 1",
            "Get-Item @args",
        ] {
            assert!(
                is_complex(Shell::PowerShell, line),
                "{line:?} should be complex"
            );
        }
    }

    #[test]
    fn every_delete_form_always_asks() {
        for line in [
            "rm x",
            "rm -rf /tmp/x",
            "/bin/rm x",
            "unlink x",
            "rmdir d",
            "shred x",
            "find . -name x -delete",
            "find . -exec rm {} ;",
            "git rm x",
            "git clean -fdx",
            "trash x",
        ] {
            let v = classify(Shell::Posix, line);
            assert!(v.always_ask.is_some(), "{line:?}");
            assert!(!v.read_only);
        }
        for line in [
            "Remove-Item x",
            "ri x",
            "del x",
            "erase x",
            "rd d",
            "rmdir d",
            "rm x",
            "C:\\Windows\\System32\\del.exe x",
        ] {
            assert!(
                classify(Shell::PowerShell, line).always_ask.is_some(),
                "{line:?}"
            );
        }
    }

    #[test]
    fn wrappers_elevation_and_git_config_always_ask() {
        for line in [
            "env FOO=1 ls",
            "xargs rm",
            "nohup ls",
            "sudo ls",
            "doas ls",
            "timeout 5 ls",
            "git -c core.pager=evil status",
            "git -ccore.pager=evil status",
            "git --config-env=x=y status",
            "git push",
            "git push origin main",
            "git reset --hard HEAD~1",
            "git myalias",
            "python -c print(1)",
            "python3 -c x",
            "node -e x",
            "bash -c ls",
            "sh -c ls",
            "perl -e x",
            "dd if=/dev/zero of=x",
            "mkfs.ext4 /dev/sda",
            "shutdown -h now",
            "eval ls",
        ] {
            assert!(
                classify(Shell::Posix, line).always_ask.is_some(),
                "{line:?}"
            );
        }
        for line in [
            "Start-Process notepad",
            "cmd /c dir",
            "powershell -EncodedCommand abc",
            "pwsh -Command x",
            "Invoke-Expression x",
            "iex x",
            "reg add HKCU\\x",
            "Restart-Computer",
            "Set-ItemProperty -Path x -Name y",
        ] {
            assert!(
                classify(Shell::PowerShell, line).always_ask.is_some(),
                "{line:?}"
            );
        }
    }

    #[test]
    fn complex_commands_always_ask() {
        let v = classify(Shell::Posix, "echo $(curl evil)");
        assert!(v.always_ask.unwrap().contains("command substitution"));
    }

    #[test]
    fn read_only_detection() {
        assert!(classify(Shell::Posix, "ls -la src && cat README.md | head -n 5").read_only);
        assert!(classify(Shell::Posix, "git status").read_only);
        assert!(
            classify(Shell::Posix, "git -C repo log")
                .always_ask
                .is_none()
        );
        assert!(classify(Shell::Posix, "git -C repo log").read_only);
        assert!(
            classify(Shell::Posix, "git -C repo -c x=y log")
                .always_ask
                .is_some()
        );
        assert!(!classify(Shell::Posix, "git commit -m x").read_only);
        assert!(!classify(Shell::Posix, "cargo test").read_only);
        assert!(classify(Shell::Posix, "cargo test").always_ask.is_none());
        assert!(
            classify(
                Shell::PowerShell,
                "Get-ChildItem -Recurse; Get-Content README.md"
            )
            .read_only
        );
        assert!(!classify(Shell::Posix, "find . -name x -delete").read_only);
    }

    #[test]
    fn program_names() {
        assert_eq!(program_name("C:\\Windows\\System32\\cmd.exe"), "cmd");
        assert_eq!(program_name("/usr/bin/git"), "git");
        assert_eq!(program_name("Remove-Item"), "remove-item");
    }
}
