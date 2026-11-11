//! Replacing one exact piece of an artifact's text (`update_artifact` with `old_text` and
//! `new_text`), with failures the model can act on: where the text matches when it
//! matches more than once, and what comes closest when it matches nowhere.
//!
//! Everything quoted is the artifact's own text, which the model wrote or can read with
//! `read_artifact`. Each quoted line is cut and made visible; each list is capped.

/// Characters of a quoted line.
const MAX_LINE_CHARS: usize = 120;
/// Matches listed.
const MAX_MATCHES: usize = 10;
/// Closest lines offered.
const MAX_CLOSEST: usize = 3;
/// Lines looked at when searching for the closest ones.
const SCAN_LINES: usize = 20_000;
/// The least alike (0 to 1) a line may be to be offered as close.
const CLOSE_ENOUGH: f64 = 0.5;

/// An edit that can be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaced {
    /// The whole text after the edit.
    pub text: String,
    /// The line the replaced text started on, from 1.
    pub line: usize,
}

fn quote(line: &str) -> String {
    let line = line.trim_end_matches(['\r', '\n']).trim();
    let cut: String = line.chars().take(MAX_LINE_CHARS).collect();
    let mut shown = policy::sanitize::visible(&cut).replace('\t', "\\t");
    if line.chars().count() > MAX_LINE_CHARS {
        shown.push('…');
    }
    shown
}

fn line_of(text: &str, at: usize) -> usize {
    1 + text[..at].bytes().filter(|&b| b == b'\n').count()
}

/// Each line trimmed and its runs of spaces and tabs made one space.
fn squash(text: &str) -> String {
    text.split('\n')
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The line of `text` on which the `n`-th line (from 0) of its squashed form starts:
/// squashing keeps the lines, so it is the same line.
fn squashed_line(squashed: &str, at: usize) -> usize {
    line_of(squashed, at)
}

fn bigrams(text: &str) -> Vec<(char, char)> {
    let chars: Vec<char> = text.chars().take(300).collect();
    chars.windows(2).map(|w| (w[0], w[1])).collect()
}

/// How alike two lines are, from 0 to 1 (shared pairs of neighbouring characters).
fn similarity(a: &[(char, char)], b: &[(char, char)]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut rest = b.to_vec();
    let mut common = 0usize;
    for pair in a {
        if let Some(i) = rest.iter().position(|p| p == pair) {
            rest.swap_remove(i);
            common += 1;
        }
    }
    // Small counts: the precision of f64 is not a concern.
    (2 * common) as f64 / (a.len() + b.len()) as f64
}

/// The lines of `text` most like the longest line of `old_text`, best first.
fn closest_lines<'a>(text: &'a str, old_text: &str) -> Vec<(usize, &'a str)> {
    let normal = |line: &str| {
        line.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let wanted = old_text
        .lines()
        .map(normal)
        .max_by_key(|l| l.chars().count())
        .unwrap_or_default();
    let wanted = bigrams(&wanted);
    if wanted.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(f64, usize, &str)> = text
        .lines()
        .take(SCAN_LINES)
        .enumerate()
        .filter_map(|(i, line)| {
            let score = similarity(&wanted, &bigrams(&normal(line)));
            (score >= CLOSE_ENOUGH).then_some((score, i + 1, line))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(MAX_CLOSEST)
        .map(|(_, n, line)| (n, line))
        .collect()
}

/// Why `old_text` is not in `text` although something very like it is, in words.
fn near_miss(text: &str, old_text: &str) -> Option<String> {
    let text_lf = text.replace("\r\n", "\n");
    let old_lf = old_text.replace("\r\n", "\n");
    if (text_lf.len() != text.len() || old_lf.len() != old_text.len()) && text_lf.contains(&old_lf)
    {
        return Some(if text_lf.len() != text.len() {
            " It differs only in line endings: the artifact uses Windows line endings (\\r\\n) and old_text does not.".into()
        } else {
            " It differs only in line endings: old_text has Windows line endings (\\r\\n) and the artifact uses \\n.".into()
        });
    }
    let text_sq = squash(&text_lf);
    let old_sq = squash(&old_lf);
    let old_sq = old_sq.trim_matches('\n');
    if old_sq.is_empty() {
        return None;
    }
    if let Some(at) = text_sq.find(old_sq) {
        return Some(format!(
            " The text at line {} is the same apart from spaces or indentation.",
            squashed_line(&text_sq, at)
        ));
    }
    let (text_low, old_low) = (text_lf.to_lowercase(), old_lf.to_lowercase());
    if let Some(at) = text_low.find(&old_low) {
        // Lowercasing can change byte lengths; the line count before the match does not.
        return Some(format!(
            " The text at line {} is the same apart from upper and lower case.",
            line_of(&text_low, at)
        ));
    }
    let (text_both, old_both) = (text_sq.to_lowercase(), old_sq.to_lowercase());
    text_both.find(&old_both).map(|at| {
        format!(
            " The text at line {} is the same apart from spaces, indentation and upper and lower case.",
            squashed_line(&text_both, at)
        )
    })
}

/// Replaces the one place where `old_text` is in `current`. `what` names the text in a
/// failure ("artifact #3").
pub fn replace_once(
    current: &str,
    old_text: &str,
    new_text: &str,
    what: &str,
) -> Result<Replaced, String> {
    if old_text.is_empty() {
        return Err(
            "old_text is empty. Put the exact text to replace in old_text, or send the whole new text in content."
                .into(),
        );
    }
    let matches: Vec<usize> = current.match_indices(old_text).map(|(at, _)| at).collect();
    match matches.as_slice() {
        [] => {
            let mut message = format!("old_text was not found in {what}.");
            if let Some(why) = near_miss(current, old_text) {
                message.push_str(&why);
            }
            let closest = closest_lines(current, old_text);
            if closest.is_empty() {
                message.push_str(" Nothing in it looks like old_text.");
            } else {
                message.push_str(" Closest lines:\n");
                for (n, line) in &closest {
                    message.push_str(&format!("line {n}: {}\n", quote(line)));
                }
            }
            message.push_str(
                " Copy the text to replace exactly as it is (read_artifact shows it), or send the whole new text in content.",
            );
            Err(message.replace("\n ", "\n"))
        }
        [at] => {
            if old_text == new_text {
                return Err(format!(
                    "old_text and new_text are the same, so nothing in {what} would change. Put the new wording in new_text."
                ));
            }
            let mut text = String::with_capacity(current.len() + new_text.len());
            text.push_str(&current[..*at]);
            text.push_str(new_text);
            text.push_str(&current[at + old_text.len()..]);
            Ok(Replaced {
                text,
                line: line_of(current, *at),
            })
        }
        many => {
            let lines: Vec<&str> = current.lines().collect();
            let mut message = format!(
                "old_text was found {} times in {what}, so it is not clear which one to change. Matches:\n",
                many.len()
            );
            for at in many.iter().take(MAX_MATCHES) {
                let n = line_of(current, *at);
                message.push_str(&format!(
                    "line {n}: {}\n",
                    quote(lines.get(n - 1).copied().unwrap_or(""))
                ));
            }
            if many.len() > MAX_MATCHES {
                message.push_str(&format!("(and {} more)\n", many.len() - MAX_MATCHES));
            }
            message.push_str(
                "Include more of the surrounding text in old_text so that it matches only once, or send the whole new text in content.",
            );
            Err(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "<h1>Tips</h1>\n<p class=\"a\">Total: 0</p>\n<p class=\"b\">Total: 0</p>\n<button>Add</button>\n";

    #[test]
    fn one_match_is_replaced_and_its_line_reported() {
        let done = replace_once(
            PAGE,
            "<button>Add</button>",
            "<button>Add tip</button>",
            "artifact #3",
        )
        .unwrap();
        assert_eq!(done.line, 4);
        assert_eq!(
            done.text,
            "<h1>Tips</h1>\n<p class=\"a\">Total: 0</p>\n<p class=\"b\">Total: 0</p>\n<button>Add tip</button>\n"
        );
        // Text over several lines.
        let done =
            replace_once(PAGE, "</h1>\n<p class=\"a\">", "</h1>\n<p>", "artifact #3").unwrap();
        assert_eq!(done.line, 1);
        assert!(done.text.starts_with("<h1>Tips</h1>\n<p>Total: 0</p>"));
    }

    #[test]
    fn more_than_one_match_lists_each_and_says_how_to_pick() {
        let err = replace_once(PAGE, "Total: 0", "Total: 1", "artifact #3").unwrap_err();
        assert_eq!(
            err,
            "old_text was found 2 times in artifact #3, so it is not clear which one to change. Matches:\n\
             line 2: <p class=\"a\">Total: 0</p>\n\
             line 3: <p class=\"b\">Total: 0</p>\n\
             Include more of the surrounding text in old_text so that it matches only once, or send the whole new text in content."
        );
        // Many matches: ten are listed, the rest counted; long lines are cut.
        let long = format!("{}x = 1;\n", "    ".repeat(40)).repeat(14);
        let err = replace_once(&long, "x = 1;", "x = 2;", "artifact #9").unwrap_err();
        assert!(err.starts_with("old_text was found 14 times in artifact #9"));
        assert_eq!(err.lines().filter(|l| l.starts_with("line ")).count(), 10);
        assert!(err.contains("\n(and 4 more)\n"));
        assert!(err.contains("line 10: x = 1;\n"));
    }

    #[test]
    fn no_match_says_what_differs_and_shows_the_closest_lines() {
        let err = replace_once(PAGE, "<button>add</button>", "x", "artifact #3").unwrap_err();
        assert_eq!(
            err,
            "old_text was not found in artifact #3. The text at line 4 is the same apart from upper and lower case. Closest lines:\n\
             line 4: <button>Add</button>\n\
             Copy the text to replace exactly as it is (read_artifact shows it), or send the whole new text in content."
        );
        let err =
            replace_once(PAGE, "<p  class=\"b\">Total:   0</p>", "x", "artifact #3").unwrap_err();
        assert!(
            err.contains("The text at line 3 is the same apart from spaces or indentation."),
            "{err}"
        );
        assert!(err.contains("line 3: <p class=\"b\">Total: 0</p>\n"));
        let err =
            replace_once(PAGE, "<P  class=\"b\">total: 0</p>", "x", "artifact #3").unwrap_err();
        assert!(
            err.contains("apart from spaces, indentation and upper and lower case"),
            "{err}"
        );
        let err =
            replace_once(PAGE, "nothing like this at all ~~~", "x", "artifact #3").unwrap_err();
        assert_eq!(
            err,
            "old_text was not found in artifact #3. Nothing in it looks like old_text. Copy the text to replace exactly as it is (read_artifact shows it), or send the whole new text in content."
        );
    }

    #[test]
    fn line_endings_are_named_when_they_are_the_only_difference() {
        let crlf = "a\r\nb\r\nc\r\n";
        let err = replace_once(crlf, "a\nb", "x", "artifact #1").unwrap_err();
        assert!(
            err.contains("the artifact uses Windows line endings (\\r\\n) and old_text does not"),
            "{err}"
        );
        let err = replace_once("a\nb\nc\n", "a\r\nb", "x", "artifact #1").unwrap_err();
        assert!(
            err.contains("old_text has Windows line endings (\\r\\n) and the artifact uses \\n"),
            "{err}"
        );
    }

    #[test]
    fn an_edit_that_changes_nothing_or_names_nothing_is_refused() {
        let err = replace_once(
            PAGE,
            "<button>Add</button>",
            "<button>Add</button>",
            "artifact #3",
        )
        .unwrap_err();
        assert!(
            err.starts_with("old_text and new_text are the same"),
            "{err}"
        );
        let err = replace_once(PAGE, "", "x", "artifact #3").unwrap_err();
        assert!(err.starts_with("old_text is empty"), "{err}");
    }

    #[test]
    fn quoted_lines_are_cut_and_made_visible() {
        let text = format!(
            "first {}\u{202e}end\nsecond \u{200b}line\n",
            "y".repeat(200)
        );
        let wrong_case = format!("FIRST {}", "y".repeat(200));
        let err = replace_once(&text, &wrong_case, "x", "artifact #1").unwrap_err();
        let quoted = err.lines().find(|l| l.starts_with("line 1: ")).unwrap();
        assert!(quoted.ends_with('…'));
        assert!(quoted.chars().count() <= "line 1: ".len() + MAX_LINE_CHARS + 1);
        let err = replace_once(&text, "second  line", "x", "artifact #1").unwrap_err();
        assert!(err.contains("line 2: second \\u{200B}line"), "{err}");
        assert!(!err.contains('\u{200b}'));
    }
}
