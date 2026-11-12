//! Programs that can be told to run another command in a language or an option of their
//! own: `awk` with `system(...)` or a pipe to a command, `sed` with its `e` command or the
//! `e` flag of `s`, `tar` with an action or a helper program, `zip` with a test command,
//! an editor handed commands to carry out.
//!
//! The command such a program runs is built inside it, where its words can be put
//! together from pieces (`system("r" "m x")`). No reading of the command line shows them,
//! so Allow all treats the line as one that hides what it runs and keeps asking.
//!
//! Arguments are taken as written: `sed -E` and `sed -e`, `tar -I` and `tar -i` differ.

/// Whether `name` (lower case, without folder or `.exe`) was told to run another command.
pub(super) fn runs_one(name: &str, args: &[String]) -> bool {
    match name {
        "awk" | "gawk" | "mawk" | "nawk" | "original-awk" => awk_runs(&args.join(" ")),
        "sed" | "gsed" | "ssed" => sed_scripts(args).iter().any(|s| sed_runs(s)),
        "tar" | "gtar" | "bsdtar" => args.iter().any(|a| tar_runs(a)),
        "zip" => args
            .iter()
            .any(|a| a == "-TT" || a.starts_with("--unzip-command")),
        // Line editors take their commands from standard input.
        "ed" | "red" | "ex" => true,
        "vi" | "vim" | "nvim" | "gvim" | "view" | "vimdiff" => args.iter().any(|a| {
            a.starts_with('+') || matches!(a.as_str(), "-c" | "--cmd" | "-S" | "-s" | "-e" | "-E")
        }),
        "emacs" | "emacsclient" => args.iter().filter(|a| a.starts_with('-')).any(|a| {
            let option = a.trim_start_matches('-');
            option.starts_with("eval")
                || matches!(
                    option,
                    "batch" | "script" | "l" | "load" | "f" | "funcall" | "e" | "x"
                )
        }),
        _ => false,
    }
}

/// An awk program that runs a command: `system(...)`, output piped to a command
/// (`print | "cmd"`, `|&`), or a command read with `"cmd" | getline`.
fn awk_runs(text: &str) -> bool {
    let tight: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    tight.contains("system(")
        || tight.contains("|\"")
        || tight.contains("\"|")
        || tight.contains("|&")
}

/// A `tar` option that names a program or a command to run.
fn tar_runs(arg: &str) -> bool {
    const LONG: &[&str] = &[
        "--checkpoint-action",
        "--to-command",
        "--use-compress-program",
        "--rsh-command",
        "--info-script",
        "--new-volume-script",
    ];
    if arg.starts_with("--") {
        return LONG.iter().any(|option| arg.starts_with(option));
    }
    // `-I prog` is the compressor program, `-F script` the script run at each volume.
    arg.starts_with('-') && arg.contains(['I', 'F'])
}

/// The scripts on a `sed` command line: what follows `-e`, or else the first plain word.
/// A script file (`-f`) is a file that is run, like any other script.
fn sed_scripts(args: &[String]) -> Vec<&str> {
    let mut scripts = Vec::new();
    let mut named = false;
    let mut first = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        i += 1;
        if let Some(script) = arg.strip_prefix("--expression=") {
            named = true;
            scripts.push(script);
        } else if arg == "--expression" {
            named = true;
            scripts.extend(args.get(i).map(String::as_str));
            i += 1;
        } else if arg.starts_with("--file") {
            named = true;
            i += usize::from(arg == "--file");
        } else if arg.starts_with("--") {
            // Another long option, with its value attached if it has one.
        } else if arg.len() > 1 && arg.starts_with('-') {
            // Short options, perhaps several in one word; `e` and `f` take the next word.
            let letters = &arg[1..];
            if letters.ends_with('e') {
                named = true;
                scripts.extend(args.get(i).map(String::as_str));
                i += 1;
            } else if letters.ends_with('f') {
                named = true;
                i += 1;
            } else if letters.contains('e') || letters.contains('f') {
                // `-escript`, `-ffile`: the value is attached.
                named = true;
                if let Some(at) = letters.find('e').filter(|_| !letters.contains('f')) {
                    scripts.push(&letters[at + 1..]);
                }
            }
        } else if first.is_none() && !arg.is_empty() {
            // BSD `sed -i '' script file` has an empty word before the script.
            first = Some(arg);
        }
    }
    if !named {
        scripts.extend(first);
    }
    scripts
}

/// Whether a sed script holds the `e` command (run a command) or an `s` with the `e`
/// flag (run what was matched). A script that cannot be followed counts as yes.
fn sed_runs(script: &str) -> bool {
    let c: Vec<char> = script.chars().collect();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        // Between commands, and the parts of an address that are not a pattern.
        if ch.is_whitespace()
            || ch.is_ascii_digit()
            || matches!(ch, ';' | '{' | '}' | ',' | '$' | '!' | '~' | '+')
        {
            i += 1;
            continue;
        }
        match ch {
            // A pattern address: /re/ or \cREc, with its flags.
            '/' | '\\' => {
                let delimiter = if ch == '\\' {
                    i += 1;
                    match c.get(i) {
                        Some(d) => *d,
                        None => return true,
                    }
                } else {
                    '/'
                };
                match past(&c, i + 1, delimiter) {
                    Some(next) => i = next,
                    None => return true,
                }
                while matches!(c.get(i), Some('I' | 'M')) {
                    i += 1;
                }
            }
            'e' => return true,
            's' | 'y' => {
                let Some(&delimiter) = c.get(i + 1) else {
                    return true;
                };
                let Some(second) = past(&c, i + 2, delimiter) else {
                    return true;
                };
                let Some(mut end) = past(&c, second, delimiter) else {
                    return true;
                };
                if ch == 's' {
                    while let Some(&flag) = c.get(end) {
                        if flag.is_whitespace() || matches!(flag, ';' | '}') {
                            break;
                        }
                        if flag == 'e' {
                            return true;
                        }
                        if flag == 'w' {
                            // The rest of the line is a file name.
                            end = line_end(&c, end);
                            break;
                        }
                        end += 1;
                    }
                }
                i = end;
            }
            // Commands that take text or a name up to the end of the line.
            'a' | 'i' | 'c' | 'r' | 'R' | 'w' | 'W' | '#' => i = line_end(&c, i),
            // Commands that take a label or a number up to the next separator.
            'b' | 't' | 'T' | ':' | 'q' | 'Q' | 'l' | 'L' | 'v' => {
                while i < c.len() && !matches!(c[i], ';' | '\n' | '}') {
                    i += 1;
                }
            }
            // One-letter commands: p P d D g G h H n N x = z F.
            _ => i += 1,
        }
    }
    false
}

/// The position after the next `delimiter` that is not escaped, from `from`.
fn past(c: &[char], from: usize, delimiter: char) -> Option<usize> {
    let mut i = from;
    while i < c.len() {
        if c[i] == '\\' {
            i += 2;
            continue;
        }
        if c[i] == delimiter {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

/// The position of the end of the line that `from` is on.
fn line_end(c: &[char], from: usize) -> usize {
    c[from..]
        .iter()
        .position(|ch| *ch == '\n')
        .map_or(c.len(), |at| from + at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> (String, Vec<String>) {
        let mut words = line.split(' ').map(str::to_string);
        (words.next().unwrap_or_default(), words.collect())
    }

    fn runs(name: &str, args: &[&str]) -> bool {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        runs_one(name, &args)
    }

    #[test]
    fn sed_scripts_that_run_a_command_are_told_from_those_that_do_not() {
        for script in [
            "e",
            "1e touch x",
            "$!e date",
            "/re/e ls",
            "s/x/y/e",
            "s|x|y|ge",
            "p;e id",
            "1{e id\n}",
            "s/a/b/w out.txt\ne id",
            // Cannot be followed to its end: counted as running one.
            "s/a/b",
            "/open",
        ] {
            assert!(sed_runs(script), "{script:?}");
        }
        for script in [
            "p",
            "1,5p",
            "$d",
            "s/hello/there/",
            "s/e/e/g",
            "s/a;b/e;e/",
            "s|/usr/exe|/opt/exe|g",
            "/exit/d",
            "/re/I,/end/p",
            "y/abc/xye/",
            "a\\ new line with e in it",
            "i hello there",
            "w evidence.txt",
            "r here.txt",
            "s/a/b/w file-with-e.txt",
            "b end; :end",
            "2q",
            "n;d",
            "",
        ] {
            assert!(!sed_runs(script), "{script:?}");
        }
    }

    #[test]
    fn the_script_of_a_sed_line_is_found_among_its_words() {
        // The first plain word is the script; later ones are files.
        assert!(!runs("sed", &["-n", "1p", "example.txt"]));
        assert!(!runs("sed", &["s/a/b/", "e"]));
        assert!(runs("sed", &["1e touch x", "notes.txt"]));
        // With -e, every plain word is a file.
        assert!(runs("sed", &["-e", "p", "-e", "e id", "notes.txt"]));
        assert!(!runs("sed", &["-e", "p", "e"]));
        assert!(runs("sed", &["-ne", "e id", "notes.txt"]));
        assert!(runs("sed", &["--expression=e id", "notes.txt"]));
        // -E is the extended-pattern switch, not -e.
        assert!(!runs("sed", &["-E", "s/(a)/\\1/", "example.txt"]));
        assert!(runs("sed", &["-E", "s/(a)/\\1/e", "example.txt"]));
        // BSD in-place form, with its empty word.
        assert!(!runs("sed", &["-i", "", "s/a/b/", "example.txt"]));
        assert!(runs("sed", &["-i", "", "s/a/b/e", "example.txt"]));
        // A script file is a file that is run.
        assert!(!runs("sed", &["-f", "edit.sed", "example.txt"]));
    }

    #[test]
    fn awk_tar_zip_and_editors() {
        for line in [
            "awk BEGIN{system(\"id\")}",
            "awk BEGIN{system (\"id\")}",
            "gawk {print|\"sh\"}",
            "awk BEGIN{\"date\"|getline d}",
            "awk {print|&\"cat\"}",
            "tar -cf x.tar notes.txt --checkpoint=1 --checkpoint-action=exec=id",
            "tar -xf x.tar --to-command=sh",
            "tar --use-compress-program=prog -cf x.tar .",
            "tar -cIprog -f x.tar .",
            "zip -r a.zip . -T -TT id",
            "zip a.zip x --unzip-command=id",
            "vim -c :!id notes.txt",
            "vim +!id notes.txt",
            "ex notes.txt",
            "ed notes.txt",
            "emacs --batch --eval (shell-command\"id\")",
        ] {
            let (name, args) = words(line);
            assert!(runs_one(&name, &args), "{line}");
        }
        for line in [
            "awk {print$1} notes.txt",
            "awk -F, {print$2} data.csv",
            "awk BEGIN{print1>\"out.txt\"}",
            "tar -czf x.tar.gz src",
            "tar -xzf x.tar.gz",
            "tar tf x.tar",
            "tar --exclude=node_modules -cf x.tar .",
            "zip -r a.zip src",
            "zip -T a.zip",
            "vim notes.txt",
            "emacs notes.txt",
            "emacs x",
            "cat system(.txt",
        ] {
            let (name, args) = words(line);
            assert!(!runs_one(&name, &args), "{line}");
        }
    }
}
