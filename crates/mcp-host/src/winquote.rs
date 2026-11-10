//! Running Windows `.cmd` / `.bat` shims (`npx.cmd`, `pnpm.cmd`) through `cmd.exe` with
//! strict quoting (ARCHITECTURE.md 9.2).
//!
//! A batch file cannot be started directly; it always runs inside `cmd.exe`, which
//! expands `%VAR%` and treats `& | < > ^ ( )` as operators. The host therefore starts
//!
//! ```text
//! cmd.exe /d /v:off /s /c ""C:\path\npx.cmd" "arg 1" "arg2""
//! ```
//!
//! - `/d` skips AutoRun commands from the registry, `/v:off` turns off delayed `!VAR!`
//!   expansion, and `/s /c` makes `cmd.exe` strip exactly the outer pair of quotes and run
//!   the rest as written.
//! - The script path and every argument are wrapped in double quotes, inside which `cmd.exe`
//!   treats `& | < > ^ ( )` and spaces as ordinary characters, and `%*` in the shim passes
//!   them on with their quotes intact.
//! - A run of backslashes at the end of an argument is doubled, so the program behind the
//!   shim (which splits its command line with the usual Microsoft C rules) does not read
//!   `\"` as an escaped quote.
//!
//! **Refused**, because no quoting makes them safe inside `cmd.exe`: a double quote (it
//! ends the quoted region), `%` (expanded even inside quotes), CR, LF and NUL (they end
//! the command). A refused argument fails the server's start with a clear message rather
//! than being passed on altered.
//!
//! These are pure functions so the rules are tested on every platform.

/// Why an argument cannot be passed to a batch file safely.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuoteError {
    /// The script path itself contains a character that cannot be quoted.
    #[error("the program path contains {what}, which cannot be passed through cmd.exe safely")]
    Program {
        /// The offending character, described.
        what: &'static str,
    },
    /// An argument contains a character that cannot be quoted.
    #[error("argument {} contains {what}, which cannot be passed to a .cmd or .bat program safely", index + 1)]
    Argument {
        /// Zero-based index of the argument.
        index: usize,
        /// The offending character, described.
        what: &'static str,
    },
}

/// Whether `path` names a batch file (`.cmd` or `.bat`, any case), which must run inside
/// `cmd.exe`.
pub fn is_batch_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".cmd") || lower.ends_with(".bat")
}

/// The first character of `text` that cannot be quoted for `cmd.exe`, described.
fn unquotable(text: &str) -> Option<&'static str> {
    text.chars().find_map(|c| match c {
        '"' => Some("a double quote (\")"),
        '%' => Some("a percent sign (%)"),
        '\r' | '\n' => Some("a line break"),
        '\0' => Some("a NUL character"),
        _ => None,
    })
}

/// Quotes one argument: `"..."` with trailing backslashes doubled. The caller has already
/// checked it with [`unquotable`].
fn quote(text: &str) -> String {
    let trailing = text.len() - text.trim_end_matches('\\').len();
    let mut out = String::with_capacity(text.len() + trailing + 2);
    out.push('"');
    out.push_str(text);
    out.extend(std::iter::repeat_n('\\', trailing));
    out.push('"');
    out
}

/// The command line for running `script` with `args` inside `cmd.exe`, starting with the
/// switches: `/d /v:off /s /c ""<script>" "<arg>" ..."`. Pass it to `cmd.exe` as one raw
/// argument (no further quoting). See the module documentation for the rules.
pub fn cmd_command_line(script: &str, args: &[String]) -> Result<String, QuoteError> {
    if let Some(what) = unquotable(script) {
        return Err(QuoteError::Program { what });
    }
    let mut line = String::from("/d /v:off /s /c \"");
    line.push_str(&quote(script));
    for (index, arg) in args.iter().enumerate() {
        if let Some(what) = unquotable(arg) {
            return Err(QuoteError::Argument { index, what });
        }
        line.push(' ');
        line.push_str(&quote(arg));
    }
    line.push('"');
    Ok(line)
}

/// Checks every argument without building the line, so the UI can warn when a server is
/// saved on Windows.
pub fn check_batch_args(args: &[String]) -> Result<(), QuoteError> {
    for (index, arg) in args.iter().enumerate() {
        if let Some(what) = unquotable(arg) {
            return Err(QuoteError::Argument { index, what });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn recognises_batch_files() {
        assert!(is_batch_file(r"C:\Program Files\nodejs\npx.cmd"));
        assert!(is_batch_file(r"C:\tools\RUN.BAT"));
        assert!(!is_batch_file(r"C:\tools\uvx.exe"));
        assert!(!is_batch_file("npx"));
    }

    #[test]
    fn quotes_the_script_and_every_argument() {
        let line = cmd_command_line(
            r"C:\Program Files\nodejs\npx.cmd",
            &args(&[
                "-y",
                "@modelcontextprotocol/server-filesystem",
                r"C:\Users\Me\My Docs",
            ]),
        )
        .unwrap();
        assert_eq!(
            line,
            r#"/d /v:off /s /c ""C:\Program Files\nodejs\npx.cmd" "-y" "@modelcontextprotocol/server-filesystem" "C:\Users\Me\My Docs"""#
        );
    }

    #[test]
    fn operators_are_literal_inside_quotes() {
        let line = cmd_command_line(
            r"C:\n\npx.cmd",
            &args(&["a&b", "x|y", "<in>", "^caret", "(paren)", "!bang!", ""]),
        )
        .unwrap();
        assert_eq!(
            line,
            r#"/d /v:off /s /c ""C:\n\npx.cmd" "a&b" "x|y" "<in>" "^caret" "(paren)" "!bang!" """"#
        );
    }

    #[test]
    fn trailing_backslashes_are_doubled() {
        let line =
            cmd_command_line(r"C:\n\npx.cmd", &args(&[r"C:\dir\", r"a\\", r"mid\dle"])).unwrap();
        assert_eq!(
            line,
            r#"/d /v:off /s /c ""C:\n\npx.cmd" "C:\dir\\" "a\\\\" "mid\dle"""#
        );
    }

    #[test]
    fn refuses_what_cannot_be_quoted() {
        let cases = [
            ("say \"hi\"", "a double quote (\")"),
            ("%PATH%", "a percent sign (%)"),
            ("https://x/a%20b", "a percent sign (%)"),
            ("line\nbreak", "a line break"),
            ("cr\rhere", "a line break"),
            ("nul\0", "a NUL character"),
        ];
        for (arg, what) in cases {
            assert_eq!(
                cmd_command_line(r"C:\n\npx.cmd", &args(&["ok", arg])),
                Err(QuoteError::Argument { index: 1, what }),
                "{arg:?}"
            );
            assert_eq!(
                check_batch_args(&args(&["ok", arg])),
                Err(QuoteError::Argument { index: 1, what })
            );
        }
        assert!(matches!(
            cmd_command_line(r"C:\100%\npx.cmd", &[]),
            Err(QuoteError::Program { .. })
        ));
        assert_eq!(check_batch_args(&args(&["fine", "a b"])), Ok(()));
    }

    #[test]
    fn error_messages_are_one_based() {
        let err = cmd_command_line("x.cmd", &args(&["%x%"])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "argument 1 contains a percent sign (%), which cannot be passed to a .cmd or .bat program safely"
        );
    }
}
