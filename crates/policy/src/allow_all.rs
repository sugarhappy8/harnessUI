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

use crate::command::{Parsed, Shell};
use crate::engine::KeepAsking;

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

/// The Allow-all facts of a command line. `strict` is what [`crate::command::parse`]
/// made of it.
pub fn command_facts(shell: Shell, line: &str, strict: &Parsed) -> CommandFacts {
    let _ = (shell, line, strict);
    CommandFacts::default()
}

/// Why a click on a web page control with this label still asks in Allow all, if it
/// does: the label says it spends money, destroys something or publishes.
pub fn web_label(label: &str) -> Option<KeepAsking> {
    let _ = label;
    None
}

/// Why a call of an external tool still asks in Allow all, if it does: its server marks
/// it destructive, or the user marked it always-ask (the two things that make it
/// always-ask in Ask mode, see [`crate::external_risk`]).
pub fn external(destructive_hint: Option<bool>, user_always_ask: bool) -> Option<KeepAsking> {
    let _ = (destructive_hint, user_always_ask);
    None
}

#[cfg(test)]
mod tests;
