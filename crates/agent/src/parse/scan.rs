//! Text scanning for the fallback parser: Markdown code regions, JSON values embedded in
//! text, and the effective end of a message.

use std::ops::Range;

use serde_json::Value;

/// End-of-turn tokens a server may leave at the end of the content. They do not count as
/// text after a candidate.
const TRAILING_END_TOKENS: &[&str] = &[
    "<|im_end|>",
    "<|eot_id|>",
    "<|eom_id|>",
    "<|end|>",
    "<|endoftext|>",
    "<|return|>",
    "<end_of_turn>",
    "</s>",
];

/// A fenced code block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fence {
    /// The whole block, fence lines included.
    pub range: Range<usize>,
    /// The info string after the opening fence (`json`), trimmed.
    pub info: String,
    /// The text between the fence lines.
    pub body: Range<usize>,
    /// False when the message ended before a closing fence.
    pub closed: bool,
}

/// The Markdown code regions of a text: fenced blocks and inline code spans. Marker text
/// inside either is an example, not a call (ARCHITECTURE.md 8.3).
#[derive(Debug, Default)]
pub(crate) struct CodeMap {
    fences: Vec<Fence>,
    inline: Vec<Range<usize>>,
}

impl CodeMap {
    pub fn new(text: &str) -> Self {
        let fences = find_fences(text);
        let inline = find_inline_code(text, &fences);
        Self { fences, inline }
    }

    /// True when the byte at `pos` is inside a fenced block or an inline code span.
    pub fn contains(&self, pos: usize) -> bool {
        self.fences.iter().any(|fence| fence.range.contains(&pos))
            || self.inline.iter().any(|span| span.contains(&pos))
    }

    pub fn fences(&self) -> &[Fence] {
        &self.fences
    }
}

fn find_fences(text: &str) -> Vec<Fence> {
    struct Open {
        start: usize,
        body_start: usize,
        marker: char,
        len: usize,
        info: String,
    }
    let mut fences = Vec::new();
    let mut open: Option<Open> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let line_start = offset;
        let line_end = offset + line.len();
        offset = line_end;
        let trimmed = line.trim_start();
        let Some(marker) = trimmed.chars().next().filter(|c| *c == '`' || *c == '~') else {
            continue;
        };
        let run = trimmed.chars().take_while(|c| *c == marker).count();
        if run < 3 {
            continue;
        }
        let rest = &trimmed[run..];
        match &open {
            Some(current) => {
                if marker == current.marker && run >= current.len && rest.trim().is_empty() {
                    fences.push(Fence {
                        range: current.start..line_end,
                        info: current.info.clone(),
                        body: current.body_start..line_start,
                        closed: true,
                    });
                    open = None;
                }
            }
            None => {
                // A backtick fence's info string may not contain backticks (CommonMark).
                if marker == '`' && rest.contains('`') {
                    continue;
                }
                open = Some(Open {
                    start: line_start,
                    body_start: line_end,
                    marker,
                    len: run,
                    info: rest.trim().to_owned(),
                });
            }
        }
    }
    if let Some(current) = open {
        fences.push(Fence {
            range: current.start..text.len(),
            info: current.info,
            body: current.body_start.min(text.len())..text.len(),
            closed: false,
        });
    }
    fences
}

fn find_inline_code(text: &str, fences: &[Fence]) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let in_fence = |pos: usize| fences.iter().any(|fence| fence.range.contains(&pos));
    let run_at = |pos: usize| bytes[pos..].iter().take_while(|b| **b == b'`').count();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'`' || in_fence(i) {
            i += 1;
            continue;
        }
        let len = run_at(i);
        let mut j = i + len;
        let mut closed = None;
        while j < bytes.len() {
            if in_fence(j) {
                break;
            }
            if bytes[j] == b'`' {
                let other = run_at(j);
                if other == len {
                    closed = Some(j + other);
                    break;
                }
                j += other;
            } else {
                j += 1;
            }
        }
        match closed {
            Some(end) => {
                spans.push(i..end);
                i = end;
            }
            None => i += len,
        }
    }
    spans
}

/// The byte offset where the message content effectively ends: trailing whitespace and
/// end-of-turn tokens are not content.
pub(crate) fn content_end(text: &str) -> usize {
    let mut end = text.trim_end().len();
    loop {
        let current = &text[..end];
        match TRAILING_END_TOKENS
            .iter()
            .find(|token| current.ends_with(*token))
        {
            Some(token) => end = current[..current.len() - token.len()].trim_end().len(),
            None => return end,
        }
    }
}

/// Parses one JSON value starting at byte `start` (leading whitespace allowed) and returns it
/// with the byte offset just after it. Reads at most `limit` bytes.
pub(crate) fn json_value_at(text: &str, start: usize, limit: usize) -> Option<(Value, usize)> {
    let end = floor_boundary(text, start.saturating_add(limit).min(text.len()));
    let slice = text.get(start..end)?;
    let mut stream = serde_json::Deserializer::from_str(slice).into_iter::<Value>();
    match stream.next() {
        Some(Ok(value)) => Some((value, start + stream.byte_offset())),
        _ => None,
    }
}

/// The largest char boundary at or below `index`.
pub(crate) fn floor_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// The offset of the first non-whitespace character at or after `from`, or the length.
pub(crate) fn skip_ws(text: &str, from: usize) -> usize {
    text.get(from..)
        .and_then(|rest| rest.find(|c: char| !c.is_whitespace()).map(|i| from + i))
        .unwrap_or(text.len())
}

/// Occurrences of `needle` in `text` that are not inside code.
pub(crate) fn find_outside_code<'t>(
    text: &'t str,
    needle: &'t str,
    code: &'t CodeMap,
) -> impl Iterator<Item = usize> + 't {
    text.match_indices(needle)
        .map(|(pos, _)| pos)
        .filter(move |pos| !code.contains(*pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_and_inline_code() {
        let text = "a `x <tool_call>` b\n```json\n{\"a\":1}\n```\nafter ``two `ticks` here``\n";
        let code = CodeMap::new(text);
        let inline_pos = text.find("<tool_call>").unwrap();
        assert!(code.contains(inline_pos));
        let fence = &code.fences()[0];
        assert_eq!(fence.info, "json");
        assert!(fence.closed);
        assert_eq!(&text[fence.body.clone()], "{\"a\":1}\n");
        assert!(code.contains(text.find("ticks").unwrap()));
        assert!(!code.contains(text.find("after").unwrap()));
        assert!(!code.contains(0));
    }

    #[test]
    fn unclosed_fence_runs_to_the_end() {
        let text = "x\n~~~\n<tool_call>{}</tool_call>";
        let code = CodeMap::new(text);
        assert!(!code.fences()[0].closed);
        assert!(code.contains(text.find("<tool_call>").unwrap()));
    }

    #[test]
    fn lone_backtick_is_not_code() {
        let text = "it's a ` and then <tool_call>";
        let code = CodeMap::new(text);
        assert!(!code.contains(text.find("<tool_call>").unwrap()));
    }

    #[test]
    fn content_end_skips_end_tokens() {
        assert_eq!(content_end("abc <|eot_id|>\n "), 3);
        assert_eq!(content_end("abc</s><|im_end|>"), 3);
        assert_eq!(content_end("  "), 0);
    }

    #[test]
    fn json_values_in_text() {
        let text = "x {\"a\": [1, 2]} tail";
        let (value, end) = json_value_at(text, 2, 1000).unwrap();
        assert_eq!(value, serde_json::json!({"a": [1, 2]}));
        assert_eq!(&text[end..], " tail");
        assert!(json_value_at(text, 2, 5).is_none());
        assert_eq!(floor_boundary("é", 1), 0);
    }
}
