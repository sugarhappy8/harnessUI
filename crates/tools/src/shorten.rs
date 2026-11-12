//! Shortened views of long tool results (app 0.0.4).
//!
//! A tool sizes what the model sees to the chat's per-result budget and returns the whole
//! text in `ToolOutput::full_text`. The app stores the whole text and appends one line
//! that names it, so the model can ask for more with `read_output`. This module makes the
//! shortened view: the first lines, the last lines and, for command output, the lines in
//! between that look like errors, each with its line number.
//!
//! Line numbers are the numbers `read_output` uses: lines as [`lines`] splits them, counted
//! from 1 in the whole text. [`lines`] is the same rule as `store::output_lines` (a test in
//! the app crate holds the two together).
//!
//! So that the app's line can say how much was shown and which lines to ask for next, a
//! tool that shortens leaves a note in `ToolOutput::ui` under [`HINT_KEY`] ([`Shown`]). The
//! app takes the note out again before the result goes anywhere else.

use mcp_host::{Content, ToolOutput};
use serde_json::{Value, json};

use crate::errors;

/// A line longer than this many characters counts as several lines of at most this length.
pub const MAX_LINE_CHARS: usize = 2000;
/// Bytes of the budget left free for the markers of a view and the line the app appends.
pub const NOTE_ROOM: usize = 400;
/// The key in `ToolOutput::ui` under which a tool says what it showed.
pub const HINT_KEY: &str = "shortened";

/// Characters of one error-looking line quoted in a view.
const ERROR_LINE_CHARS: usize = 200;
/// The fewest bytes a view's lines get, however small the budget.
const MIN_ROOM: usize = 600;
/// Lines a "read next" range covers at most.
const MAX_NEXT_LINES: usize = 200;

/// The bytes a tool result may use: three for each token of the per-result cap, and never
/// fewer than 1,500.
pub fn budget_bytes(result_cap_tokens: usize) -> usize {
    result_cap_tokens.max(500) * 3
}

/// The lines of a text as line numbers count them: split at line breaks like
/// [`str::lines`], and a line longer than [`MAX_LINE_CHARS`] characters is split into
/// pieces of that length.
pub fn lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut rest = line;
        loop {
            match rest.char_indices().nth(MAX_LINE_CHARS) {
                Some((cut, _)) => {
                    out.push(&rest[..cut]);
                    rest = &rest[cut..];
                }
                None => {
                    out.push(rest);
                    break;
                }
            }
        }
    }
    out
}

/// How many numbered lines one line of a text is (see [`lines`]).
pub fn pieces(line: &str) -> usize {
    line.chars().count().max(1).div_ceil(MAX_LINE_CHARS)
}

/// What a shortened result shows of its whole text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    /// Lines of the whole text that are in the view.
    pub lines: usize,
    /// The first lines that are not: a range to ask for next (1-based, inclusive).
    pub next: Option<(usize, usize)>,
}

impl Shown {
    /// A range of about as many lines as were shown, starting at `first` and ending at
    /// `last_possible` at the latest. `None` when there is nothing left to ask for.
    pub fn next_range(first: usize, shown: usize, last_possible: usize) -> Option<(usize, usize)> {
        if first == 0 || first > last_possible {
            return None;
        }
        let span = shown.clamp(20, MAX_NEXT_LINES);
        Some((first, (first + span - 1).min(last_possible)))
    }

    /// Adds the note to a result's `ui` value.
    pub fn attach(self, ui: Option<Value>) -> Option<Value> {
        let note = match self.next {
            Some((a, b)) => json!({ "lines": self.lines, "next": [a, b] }),
            None => json!({ "lines": self.lines }),
        };
        let mut ui = match ui {
            Some(Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        };
        ui.insert(HINT_KEY.into(), note);
        Some(Value::Object(ui))
    }

    /// Takes the note out of a result's `ui` value, if there is one. A `ui` that held
    /// nothing else becomes `None`.
    pub fn take(ui: &mut Option<Value>) -> Option<Shown> {
        let map = ui.as_mut()?.as_object_mut()?;
        let note = map.remove(HINT_KEY)?;
        if map.is_empty() {
            *ui = None;
        }
        let number = |v: &Value| v.as_u64().and_then(|n| usize::try_from(n).ok());
        let lines = note.get("lines").and_then(number)?;
        let next = note
            .get("next")
            .and_then(Value::as_array)
            .and_then(|pair| Some((number(pair.first()?)?, number(pair.get(1)?)?)));
        Some(Shown { lines, next })
    }
}

/// A shortened view and what it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// What the model sees.
    pub text: String,
    /// Which part of the whole text that is.
    pub shown: Shown,
}

/// The result of a tool that shortened its output: the view for the model, the whole text
/// for the app to store, and the note that says what the view shows.
pub fn shortened(view: View, full: String, is_error: bool, ui: Option<Value>) -> ToolOutput {
    ToolOutput {
        content: vec![Content::Text { text: view.text }],
        is_error,
        full_text: Some(full),
        ui: view.shown.attach(ui),
    }
}

/// `text` cut to at most `max` bytes at a character boundary.
fn cut_to(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The end of `text`, at most `max` bytes, starting at a character boundary.
fn end_of(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

fn not_shown(first: usize, last: usize) -> String {
    if first == last {
        format!("Line {first} is not shown.")
    } else {
        format!("Lines {first}-{last} are not shown.")
    }
}

/// What a view is made from.
struct Shape<'a> {
    /// Share of the room the first lines get, in percent.
    head_percent: usize,
    /// List the lines in between that look like errors.
    errors: bool,
    /// The last lines of a command's error stream, shown when the last lines of the
    /// output do not already hold them.
    stderr_tail: Option<&'a str>,
}

fn shape(full: &str, budget: usize, how: &Shape<'_>) -> Option<View> {
    if full.len() <= budget {
        return None;
    }
    let all = lines(full);
    let total = all.len();
    let mut room = budget.saturating_sub(NOTE_ROOM).max(MIN_ROOM);

    // The last lines of the error stream, when asked for.
    let stderr_tail = how
        .stderr_tail
        .map(str::trim_end)
        .filter(|t| !t.trim().is_empty())
        .map(|t| end_of(t, room / 6));
    if let Some(tail) = stderr_tail {
        room = room.saturating_sub(tail.len());
    }
    let error_room = if how.errors { room / 4 } else { 0 };
    let line_room = room - error_room;
    let head_room = line_room * how.head_percent / 100;
    let tail_room = line_room - head_room;

    // First lines.
    let mut head = String::new();
    let mut head_n = 0;
    for line in &all {
        if head.len() + line.len() + 1 > head_room {
            break;
        }
        head.push_str(line);
        head.push('\n');
        head_n += 1;
    }
    if head_n == 0 && head_room > 0 {
        // One very long line: show its start.
        head.push_str(cut_to(all[0], head_room.saturating_sub(4)));
        head.push_str("…\n");
        head_n = 1;
    }
    // Last lines.
    let mut tail_n = 0;
    let mut tail_bytes = 0;
    for line in all[head_n..].iter().rev() {
        if tail_bytes + line.len() + 1 > tail_room {
            break;
        }
        tail_bytes += line.len() + 1;
        tail_n += 1;
    }
    let mut tail = String::new();
    if tail_n == 0 && tail_room > 0 && head_n < total {
        // One very long line: show its end.
        tail.push('…');
        tail.push_str(end_of(all[total - 1], tail_room.saturating_sub(4)));
        tail.push('\n');
        tail_n = 1;
    } else {
        for line in &all[total - tail_n..] {
            tail.push_str(line);
            tail.push('\n');
        }
    }
    let (first_hidden, last_hidden) = (head_n + 1, total - tail_n);
    let mut text = head;
    if first_hidden <= last_hidden {
        let middle = &all[head_n..total - tail_n];
        let (mut picked, count) = if how.errors {
            errors::error_lines(middle, first_hidden, errors::MAX_ERROR_LINES)
        } else {
            (Vec::new(), 0)
        };
        // Only as many as fit their share.
        let mut used = 0;
        picked.retain(|(_, line)| {
            used += line.len().min(ERROR_LINE_CHARS * 4) + 12;
            used <= error_room.max(ERROR_LINE_CHARS)
        });
        text.push('[');
        text.push_str(&not_shown(first_hidden, last_hidden));
        if picked.is_empty() {
            text.push_str("]\n");
        } else {
            text.push_str(&match (count, picked.len()) {
                (1, _) => " One of them looks like an error:]\n".to_string(),
                (count, shown) if shown < count => {
                    format!(" {count} of them look like errors; the first {shown}:]\n")
                }
                (count, _) => format!(" {count} of them look like errors:]\n"),
            });
            for (number, line) in &picked {
                text.push_str(&format!(
                    "{number}: {}\n",
                    errors::quote(line.trim_end(), ERROR_LINE_CHARS)
                ));
            }
            text.push_str(&format!("[The output continues at line {}:]\n", last_hidden + 1));
        }
    }
    text.push_str(&tail);
    if let Some(stderr) = stderr_tail
        && !tail.contains(stderr.trim())
    {
        text.push_str("[The last lines of its error output (stderr):]\n");
        text.push_str(stderr);
        text.push('\n');
    }
    let text = text.trim_end_matches('\n').to_string();
    Some(View {
        text,
        shown: Shown {
            lines: head_n + tail_n,
            next: Shown::next_range(first_hidden, head_n + tail_n, last_hidden),
        },
    })
}

/// The first and last lines of `full` within `budget` bytes, with a marker in between
/// that says which lines are missing. `head_percent` of the room goes to the first lines.
/// `None` when the whole text fits.
pub fn head_and_tail(full: &str, budget: usize, head_percent: usize) -> Option<View> {
    shape(
        full,
        budget,
        &Shape {
            head_percent: head_percent.min(100),
            errors: false,
            stderr_tail: None,
        },
    )
}

/// The view of a long command output: its first lines, the lines in between that look
/// like errors with their line numbers, and its last lines. `stderr_tail` is the end of
/// the command's error stream; it is added when the last lines do not already show it.
/// `None` when the whole text fits.
pub fn command_view(full: &str, budget: usize, stderr_tail: Option<&str>) -> Option<View> {
    shape(
        full,
        budget,
        &Shape {
            head_percent: 40,
            errors: true,
            stderr_tail,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1,000 numbered lines of build output with three errors in the middle.
    fn build_log() -> String {
        let mut out = String::from("Exit code 101 (12.3 s)\n");
        for i in 2..=1000 {
            out.push_str(&match i {
                412 => "error[E0308]: mismatched types\n".to_string(),
                640 => "test parse::tests::empty ... FAILED\n".to_string(),
                777 => "thread 'main' panicked at src/main.rs:4\n".to_string(),
                i => format!("   Compiling crate-{i} v0.1.0\n"),
            });
        }
        out
    }

    #[test]
    fn a_long_command_output_shows_its_head_its_errors_and_its_tail() {
        let full = build_log();
        let view = command_view(&full, 1500, None).expect("too long to show whole");
        assert_eq!(
            view.text,
            "Exit code 101 (12.3 s)\n\
             \x20  Compiling crate-2 v0.1.0\n\
             \x20  Compiling crate-3 v0.1.0\n\
             \x20  Compiling crate-4 v0.1.0\n\
             \x20  Compiling crate-5 v0.1.0\n\
             \x20  Compiling crate-6 v0.1.0\n\
             \x20  Compiling crate-7 v0.1.0\n\
             \x20  Compiling crate-8 v0.1.0\n\
             \x20  Compiling crate-9 v0.1.0\n\
             \x20  Compiling crate-10 v0.1.0\n\
             \x20  Compiling crate-11 v0.1.0\n\
             [Lines 12-984 are not shown. 3 of them look like errors:]\n\
             412: error[E0308]: mismatched types\n\
             640: test parse::tests::empty ... FAILED\n\
             777: thread 'main' panicked at src/main.rs:4\n\
             [The output continues at line 985:]\n\
             \x20  Compiling crate-985 v0.1.0\n\
             \x20  Compiling crate-986 v0.1.0\n\
             \x20  Compiling crate-987 v0.1.0\n\
             \x20  Compiling crate-988 v0.1.0\n\
             \x20  Compiling crate-989 v0.1.0\n\
             \x20  Compiling crate-990 v0.1.0\n\
             \x20  Compiling crate-991 v0.1.0\n\
             \x20  Compiling crate-992 v0.1.0\n\
             \x20  Compiling crate-993 v0.1.0\n\
             \x20  Compiling crate-994 v0.1.0\n\
             \x20  Compiling crate-995 v0.1.0\n\
             \x20  Compiling crate-996 v0.1.0\n\
             \x20  Compiling crate-997 v0.1.0\n\
             \x20  Compiling crate-998 v0.1.0\n\
             \x20  Compiling crate-999 v0.1.0\n\
             \x20  Compiling crate-1000 v0.1.0"
        );
        assert_eq!(
            view.shown,
            Shown {
                lines: 27,
                next: Some((12, 38))
            }
        );
        // The view and the line the app appends stay inside the budget.
        assert!(view.text.len() + 200 <= 1500, "{}", view.text.len());
        // The line numbers are the numbers of the whole text.
        let all = lines(&full);
        assert_eq!(all[411], "error[E0308]: mismatched types");
        assert_eq!(all[984], "   Compiling crate-985 v0.1.0");
    }

    #[test]
    fn many_error_lines_are_capped_and_counted() {
        let mut full = String::new();
        for i in 1..=600 {
            full.push_str(&if i % 50 == 0 {
                format!("error: step {i} failed\n")
            } else {
                format!("ok {i}\n")
            });
        }
        let view = command_view(&full, 3000, None).expect("shortened");
        let marker = view
            .text
            .lines()
            .find(|l| l.starts_with("[Lines "))
            .expect("a marker");
        assert!(
            marker.ends_with("of them look like errors; the first 5:]"),
            "{marker}"
        );
        let numbered: Vec<&str> = view
            .text
            .lines()
            .filter(|l| l.contains(": error: step"))
            .collect();
        assert_eq!(numbered.len(), 5, "{}", view.text);
        assert_eq!(numbered[0], "150: error: step 150 failed");
        // Each listed number is the line's number in the whole text.
        let all = lines(&full);
        for line in numbered {
            let (number, text) = line.split_once(": ").expect("number: text");
            assert_eq!(all[number.parse::<usize>().expect("a number") - 1], text);
        }
    }

    #[test]
    fn output_without_errors_gets_a_plain_marker() {
        let full: String = (1..=400).map(|i| format!("row {i}\n")).collect();
        let view = command_view(&full, 1500, None).expect("shortened");
        assert!(
            view.text.contains("\n[Lines "),
            "{}",
            view.text
        );
        let marker = view
            .text
            .lines()
            .find(|l| l.starts_with("[Lines "))
            .expect("a marker");
        assert!(marker.ends_with(" are not shown.]"), "{marker}");
        assert!(!view.text.contains("The output continues"));
        assert!(view.text.starts_with("row 1\nrow 2\n"));
        assert!(view.text.ends_with("row 399\nrow 400"));
    }

    #[test]
    fn the_end_of_the_error_stream_is_added_when_the_tail_does_not_show_it() {
        let mut full = String::from("Exit code 2 (0.4 s)\nmake: *** [all] Error 2\n");
        for i in 0..500 {
            full.push_str(&format!("stdout line {i}\n"));
        }
        let stderr = "make: *** [all] Error 2";
        let view = command_view(&full, 1500, Some(stderr)).expect("shortened");
        assert!(
            view.text
                .ends_with("stdout line 499\n[The last lines of its error output (stderr):]\nmake: *** [all] Error 2"),
            "{}",
            view.text
        );
        // Already in the last lines: not repeated.
        let view = command_view(&full, 1500, Some("stdout line 499")).expect("shortened");
        assert!(!view.text.contains("stderr"), "{}", view.text);
    }

    #[test]
    fn text_that_fits_is_left_alone() {
        assert_eq!(command_view("Exit code 0 (0.1 s)\nhello\n", 1500, None), None);
        assert_eq!(head_and_tail(&"x\n".repeat(700), 1500, 50), None);
        assert!(head_and_tail(&"x\n".repeat(800), 1500, 50).is_some());
    }

    #[test]
    fn head_and_tail_split_as_asked() {
        let full: String = (1..=500).map(|i| format!("line {i}\n")).collect();
        let view = head_and_tail(&full, 1500, 75).expect("shortened");
        let (head, tail) = view.text.split_once("\n[Lines ").expect("a marker");
        let tail = tail.split_once("]\n").expect("the marker's end").1;
        assert!(head.lines().count() > 2 * tail.lines().count(), "{}", view.text);
        assert_eq!(view.shown.lines, head.lines().count() + tail.lines().count());
        let first_hidden = head.lines().count() + 1;
        assert_eq!(view.shown.next.map(|(a, _)| a), Some(first_hidden));
        assert!(view.text.len() + NOTE_ROOM / 2 <= 1500);
    }

    #[test]
    fn one_huge_line_is_cut_inside_the_line_and_never_inside_a_character() {
        // 9,000 two-byte characters with no line break: five numbered pieces.
        let full = "é".repeat(9000);
        assert_eq!(lines(&full).len(), 5);
        assert_eq!(pieces(&full), 5);
        let view = head_and_tail(&full, 1500, 50).expect("shortened");
        assert!(view.text.starts_with("ééé") && view.text.ends_with("ééé"));
        assert!(view.text.contains("…\n[Lines 2-4 are not shown.]\n…"), "{}", &view.text[..60]);
        assert!(view.text.len() <= 1500);
        assert_eq!(view.shown.lines, 2);
        assert_eq!(view.shown.next, Some((2, 4)));
    }

    #[test]
    fn lines_split_like_str_lines_and_long_lines_into_pieces() {
        assert_eq!(lines("a\nb\r\n\nc"), ["a", "b", "", "c"]);
        assert!(lines("").is_empty());
        let long = format!("{}\nend", "x".repeat(MAX_LINE_CHARS * 2 + 5));
        let got = lines(&long);
        assert_eq!(got.len(), 4);
        assert_eq!(got[0].len(), MAX_LINE_CHARS);
        assert_eq!(got[2], "xxxxx");
        assert_eq!(got[3], "end");
        assert_eq!(pieces(""), 1);
        assert_eq!(pieces(&"x".repeat(MAX_LINE_CHARS)), 1);
        assert_eq!(pieces(&"x".repeat(MAX_LINE_CHARS + 1)), 2);
    }

    #[test]
    fn the_note_for_the_app_travels_in_ui_and_is_taken_out_again() {
        let shown = Shown {
            lines: 30,
            next: Some((13, 42)),
        };
        let mut ui = shown.attach(Some(json!({"command": "ls", "cwd": "/w"})));
        assert_eq!(
            ui,
            Some(json!({"command": "ls", "cwd": "/w", "shortened": {"lines": 30, "next": [13, 42]}}))
        );
        assert_eq!(Shown::take(&mut ui), Some(shown));
        assert_eq!(ui, Some(json!({"command": "ls", "cwd": "/w"})));
        assert_eq!(Shown::take(&mut ui), None);
        // A ui that held only the note is gone afterwards.
        let nothing_next = Shown {
            lines: 4,
            next: None,
        };
        let mut ui = nothing_next.attach(None);
        assert_eq!(Shown::take(&mut ui), Some(nothing_next));
        assert_eq!(ui, None);
        let mut none = None;
        assert_eq!(Shown::take(&mut none), None);
        // Ranges: about as much again, inside what is left.
        assert_eq!(Shown::next_range(13, 30, 982), Some((13, 42)));
        assert_eq!(Shown::next_range(78, 77, 100), Some((78, 100)));
        assert_eq!(Shown::next_range(5, 1, 900), Some((5, 24)));
        assert_eq!(Shown::next_range(5, 4000, 9000), Some((5, 204)));
        assert_eq!(Shown::next_range(101, 10, 100), None);
    }
}
