//! The prompt as the pinned engine reads it, after `run_job` writes it to the prompt's file.
//!
//! - [`strip_prompt_markers`] removes the markers the engine acts on from typed text: LoRA tags
//!   (`<lora:NAME:W>`, [`super::lora_tags`]) and image markers (`<image:PATH>`), whole or left
//!   open. Studio passes LoRAs as its own tags and pictures with `-r`, never through the
//!   person's text: a typed image marker would make `sd-cli` load any file the path names
//!   (`examples/cli/main.cpp:965-976`), and one left open makes it throw
//!   (`src/conditioning/prompt_input.cpp:90-91`).
//! - [`parse_prompt_attention`] is the engine's own reading of the weighting syntax (`(word)`,
//!   `[word]`, `(word:1.2)`, `BREAK`), ported from `src/core/util.cpp:909-1019` at the pinned
//!   commit. Every text encoder runs the prompt through it (`conditioner.hpp:403`, `:819`,
//!   `:1232`, `:1505`, `:2212`, `:3935`), and it drops brackets and `:<number>)` and joins
//!   segments of equal weight, so `se()xy` reaches the model as "sexy". [`attention_text`]
//!   gives the words the model reads, for the prompt check.

/// The start of the engine's image marker (`parse_prompt_input`,
/// `src/conditioning/prompt_input.cpp:69-105`): `<image:PATH>`, the path running to the next
/// `>`.
const IMAGE_MARKER_OPEN: &str = "<image:";

/// The weight `(` gives what it holds (`round_bracket_multiplier`, util.cpp:914).
const ROUND_BRACKET_MULTIPLIER: f32 = 1.1;

/// The text of a `BREAK` segment, whose weight is -1 (util.cpp:984-985).
const BREAK: &str = "BREAK";

/// `prompt` without the markers the engine acts on: every LoRA tag the engine's pattern
/// `<lora:([^:>]+):([^>]+)>` matches, as it finds them (it removes LoRA tags first,
/// `examples/common/common.cpp:2352`); then every image marker `<image:PATH>` with a path
/// that is not empty, as `parse_prompt_input` finds them; then every `<lora:` and `<image:`
/// left, including one that removing another joins together (`<ima<lora:ge:`). An image
/// marker is removed even when a backslash escapes it, as Studio never sends one.
///
/// The text that comes back holds neither `<lora:` nor `<image:`, so the engine finds only
/// the LoRA tags Studio appends ([`super::args::prompt_file_text`]), no image, and nothing
/// to throw on. A marker left open would otherwise make one with what follows it
/// (`<lora:b:3` and ` <lora:x:1>` read as `b` at 3). Each pass is linear in the prompt's
/// length.
pub fn strip_prompt_markers(prompt: &str) -> String {
    let spans_removed = remove_image_spans(&super::lora_tags::remove_lora_spans(prompt));
    let mut kept = String::with_capacity(spans_removed.len());
    for c in spans_removed.chars() {
        kept.push(c);
        for open in [super::lora_tags::LORA_TAG_OPEN, IMAGE_MARKER_OPEN] {
            if kept.ends_with(open) {
                kept.truncate(kept.len() - open.len());
            }
        }
    }
    kept
}

/// `text` without the image markers `parse_prompt_input` finds: from each `<image:`, the
/// leftmost first, to the next `>`, when the path between is not empty. No marker ends after
/// the last `>`, so the search stops there, and a marker found is removed whole, so each byte
/// is read a bounded number of times.
fn remove_image_spans(text: &str) -> String {
    let Some(last_close) = text.rfind('>') else {
        return text.to_owned();
    };
    let mut kept = String::with_capacity(text.len());
    let (mut copied, mut from) = (0, 0);
    while let Some(found) = text
        .get(from..last_close)
        .and_then(|before| before.find(IMAGE_MARKER_OPEN))
    {
        let path_at = from + found + IMAGE_MARKER_OPEN.len();
        match text[path_at..].find('>') {
            Some(path_len) if path_len > 0 => {
                kept.push_str(&text[copied..from + found]);
                copied = path_at + path_len + 1;
                from = copied;
            }
            _ => from = path_at,
        }
    }
    kept.push_str(&text[copied..]);
    kept
}

/// The words the model reads from `text` (after [`strip_prompt_markers`]): the texts of the
/// segments of [`parse_prompt_attention`], joined in order whatever their weights, with each
/// `BREAK` read as a space. CLIP pads to its next chunk at a `BREAK`, and the other encoders
/// read each segment on their own, so the words beside it never join. Joining across weights
/// reads more joined words than the engine's merge, which matters only for text built to
/// trick the check.
///
/// Where the engine joins `BREAK` segments of equal weight into text (`BREAK:1)BREAK` reads
/// `BREAKBREAK`), this reads a space for each: a word can be lost only if it holds "break",
/// and the prompt check's lists hold none (its test `no_listed_word_holds_break`).
///
/// This walks the same tokens as [`parse_prompt_attention`] without its weights, so it is
/// linear in the text's length; the engine's weighting is not, with many brackets.
pub fn attention_text(text: &str) -> String {
    let mut read = String::with_capacity(text.len());
    for event in AttentionEvents::new(text) {
        match event {
            AttentionEvent::Text(segment) => read.push_str(segment),
            AttentionEvent::Break => read.push(' '),
            AttentionEvent::Open(_) | AttentionEvent::Close(_) | AttentionEvent::Weight(_) => {}
        }
    }
    read
}

/// The engine's `parse_prompt_attention` (`src/core/util.cpp:909-1019`): `text` as segments
/// of text, each with its weight, in order.
///
/// - `(…)` multiplies the weight of what it holds by 1.1, `[…]` by 1/1.1, and `(…:W)` by
///   `W`; the brackets and `:W)` go. A `:W)` with no `(` open goes too (util.cpp:962-971).
///   An unmatched `)` or `]` is text; a bracket left open counts at the end.
/// - `\(` is the text `(`. `\)`, `\[`, `\]`, `\\` and a lone `\` are text as written, their
///   backslash kept: the code does not do what the comment above it says for those
///   (util.cpp:983).
/// - `BREAK`, as a whole word, is a segment of its own with weight -1 (times any bracket's
///   weight around it). The engine searches each step's remaining text afresh, so the word
///   boundary before it always holds: `xBREAK y` breaks after `x`.
/// - Neighbouring segments of equal weight are joined (util.cpp:1008-1015), in one pass here
///   rather than by erasing, with the same result. An empty `text` is one empty segment of
///   weight 1.
///
/// The engine also splits runs of plain text at 1,024 bytes, for its regex's stack; the
/// pieces always have equal weights and are joined back, so this reads them whole. Like the
/// engine, it multiplies every segment a bracket holds when the bracket closes, so its time
/// grows with the segments times the brackets; [`attention_text`] reads the same text in
/// linear time.
pub fn parse_prompt_attention(text: &str) -> Vec<(String, f32)> {
    let mut res: Vec<(String, f32)> = Vec::new();
    let mut round_brackets: Vec<usize> = Vec::new();
    let mut square_brackets: Vec<usize> = Vec::new();
    let square_bracket_multiplier = 1.0 / ROUND_BRACKET_MULTIPLIER;
    let multiply_range = |res: &mut Vec<(String, f32)>, start: usize, multiplier: f32| {
        for segment in &mut res[start..] {
            segment.1 *= multiplier;
        }
    };
    for event in AttentionEvents::new(text) {
        match event {
            AttentionEvent::Weight(weight) => {
                if let Some(start) = round_brackets.pop() {
                    multiply_range(&mut res, start, weight);
                }
            }
            AttentionEvent::Open(Bracket::Round) => round_brackets.push(res.len()),
            AttentionEvent::Open(Bracket::Square) => square_brackets.push(res.len()),
            AttentionEvent::Close(Bracket::Round) => {
                if let Some(start) = round_brackets.pop() {
                    multiply_range(&mut res, start, ROUND_BRACKET_MULTIPLIER);
                }
            }
            AttentionEvent::Close(Bracket::Square) => {
                if let Some(start) = square_brackets.pop() {
                    multiply_range(&mut res, start, square_bracket_multiplier);
                }
            }
            AttentionEvent::Text(segment) => res.push((segment.to_owned(), 1.0)),
            AttentionEvent::Break => res.push((BREAK.to_owned(), -1.0)),
        }
    }
    for &start in &round_brackets {
        multiply_range(&mut res, start, ROUND_BRACKET_MULTIPLIER);
    }
    for &start in &square_brackets {
        multiply_range(&mut res, start, square_bracket_multiplier);
    }
    if res.is_empty() {
        res.push((String::new(), 1.0));
    }
    let mut merged: Vec<(String, f32)> = Vec::with_capacity(res.len());
    for (segment, weight) in res {
        match merged.last_mut() {
            Some(last) if last.1 == weight => last.0.push_str(&segment),
            _ => merged.push((segment, weight)),
        }
    }
    merged
}

/// A round `(` or square `[` bracket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bracket {
    Round,
    Square,
}

/// What one token of the engine's loop does (util.cpp:957-991).
#[derive(Debug, Clone, Copy, PartialEq)]
enum AttentionEvent<'a> {
    /// A bracket opens.
    Open(Bracket),
    /// A bracket that is open closes; a closer with none open is [`AttentionEvent::Text`].
    Close(Bracket),
    /// `:W)`: the innermost round bracket, if one is open, closes with weight `W`.
    Weight(f32),
    /// Text, a segment of weight 1.
    Text(&'a str),
    /// The word `BREAK`, a segment of weight -1.
    Break,
}

/// The engine's loop over `re_attention`'s tokens (util.cpp:957-991), as events. It counts
/// the open brackets itself, so an unmatched closer is text as in the engine.
struct AttentionEvents<'a> {
    remaining: &'a str,
    round_open: usize,
    square_open: usize,
}

impl<'a> AttentionEvents<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            remaining: text,
            round_open: 0,
            square_open: 0,
        }
    }
}

impl<'a> Iterator for AttentionEvents<'a> {
    type Item = AttentionEvent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }
        let token = attention_token(self.remaining);
        let suffix = &self.remaining[token.len()..];
        self.remaining = suffix;
        if token == ":"
            && let Some((weight, weight_len)) = lex_weight(suffix)
        {
            self.round_open = self.round_open.saturating_sub(1);
            self.remaining = &suffix[weight_len..];
            return Some(AttentionEvent::Weight(weight));
        }
        Some(match token {
            "(" => {
                self.round_open += 1;
                AttentionEvent::Open(Bracket::Round)
            }
            "[" => {
                self.square_open += 1;
                AttentionEvent::Open(Bracket::Square)
            }
            ")" if self.round_open > 0 => {
                self.round_open -= 1;
                AttentionEvent::Close(Bracket::Round)
            }
            "]" if self.square_open > 0 => {
                self.square_open -= 1;
                AttentionEvent::Close(Bracket::Square)
            }
            "\\(" => AttentionEvent::Text("("),
            BREAK => AttentionEvent::Break,
            text => AttentionEvent::Text(text),
        })
    }
}

/// The token `re_attention` (util.cpp:921-922) matches at the start of `remaining`, trying
/// its alternatives in order: `\(` `\)` `\[` `\]` `\\`, a lone `\`, `(` `[` `)` `]`, the word
/// `BREAK`, a run of anything but `\()[]:B`, `:`, and a lone `B`. Something always matches
/// at the start, so the engine's search never skips a character.
fn attention_token(remaining: &str) -> &str {
    const ESCAPES: [&str; 5] = ["\\(", "\\)", "\\[", "\\]", "\\\\"];
    const NOT_PLAIN: &[u8] = b"\\()[]:B";
    if let Some(escape) = ESCAPES
        .iter()
        .find(|escape| remaining.starts_with(**escape))
    {
        return &remaining[..escape.len()];
    }
    let bytes = remaining.as_bytes();
    if matches!(bytes[0], b'\\' | b'(' | b'[' | b')' | b']') {
        return &remaining[..1];
    }
    // `\bBREAK\b`: the boundary before holds at the start of the text searched; the one
    // after needs the end, or a character that is not a word character.
    if remaining.starts_with(BREAK) && !bytes.get(BREAK.len()).is_some_and(|&b| is_word(b)) {
        return &remaining[..BREAK.len()];
    }
    let plain = bytes
        .iter()
        .position(|b| NOT_PLAIN.contains(b))
        .unwrap_or(bytes.len());
    // A run of plain text, or else `:` or `B`, one byte each.
    &remaining[..plain.max(1)]
}

/// A regex word character, as libstdc++ reads one in the "C" locale: an ASCII letter or
/// digit, or `_`.
fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The engine's `lex_weight` (util.cpp:932-954): after a `:`, an optional sign, digits and
/// dots, then `)`; the weight and how many bytes it takes with its `)`. The number must read
/// whole as a finite float (so `.`, `+.` and `1.2.3` are no weight).
fn lex_weight(s: &str) -> Option<(f32, usize)> {
    let bytes = s.as_bytes();
    let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    while bytes
        .get(end)
        .is_some_and(|&b| b.is_ascii_digit() || b == b'.')
    {
        end += 1;
    }
    if bytes.get(end) != Some(&b')') {
        return None;
    }
    let weight: f32 = s[..end].parse().ok()?;
    weight.is_finite().then_some((weight, end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The segments' texts and weights, the weights to 6 places, as the engine logs them.
    fn parsed(text: &str) -> Vec<(String, String)> {
        parse_prompt_attention(text)
            .into_iter()
            .map(|(segment, weight)| (segment, format!("{weight:.6}")))
            .collect()
    }

    fn segments(list: &[(&str, f64)]) -> Vec<(String, String)> {
        list.iter()
            .map(|&(segment, weight)| (segment.to_owned(), format!("{:.6}", weight as f32)))
            .collect()
    }

    /// The examples in the comment above the engine's function (util.cpp:889-908), with the
    /// one the code reads otherwise: `\]` keeps its backslash (only `\(` loses it).
    #[test]
    fn the_engines_own_examples_parse_as_its_code_parses_them() {
        assert_eq!(parsed("normal text"), segments(&[("normal text", 1.0)]));
        assert_eq!(
            parsed("an (important) word"),
            segments(&[("an ", 1.0), ("important", 1.1), (" word", 1.0)])
        );
        assert_eq!(parsed("(unbalanced"), segments(&[("unbalanced", 1.1)]));
        // the comment says `(literal]`; the code keeps the backslash of `\]`
        assert_eq!(parsed("\\(literal\\]"), segments(&[("(literal\\]", 1.0)]));
        assert_eq!(
            parsed("(unnecessary)(parens)"),
            segments(&[("unnecessaryparens", 1.1)])
        );
        assert_eq!(
            parsed("a (((house:1.3)) [on] a (hill:0.5), sun, (((sky)))."),
            segments(&[
                ("a ", 1.0),
                ("house", 1.5730000000000004),
                (" ", 1.1),
                ("on", 1.0),
                (" a ", 1.1),
                ("hill", 0.55),
                (", sun, ", 1.1),
                ("sky", 1.4641000000000006),
                (".", 1.1),
            ])
        );
    }

    /// Cases checked against a verbatim copy of the engine's function compiled on its own
    /// (the reviewer's oracle, Task S2 fix round 2): the weights' bits are equal.
    #[test]
    fn the_edges_parse_as_the_engine_parses_them() {
        let exact = |text: &str| -> Vec<(String, u32)> {
            parse_prompt_attention(text)
                .into_iter()
                .map(|(segment, weight)| (segment, weight.to_bits()))
                .collect()
        };
        let one = 1.0f32.to_bits();
        assert_eq!(exact(""), [(String::new(), one)]);
        // the word boundary before BREAK holds after a plain run; the one after does not
        assert_eq!(
            exact("xBREAK y"),
            [
                ("x".to_owned(), one),
                ("BREAK".to_owned(), (-1.0f32).to_bits()),
                (" y".to_owned(), one)
            ]
        );
        assert_eq!(exact("BREAKfast"), [("BREAKfast".to_owned(), one)]);
        assert_eq!(
            exact("a\\\\(b"),
            [
                ("a\\\\".to_owned(), one),
                ("b".to_owned(), 1.1f32.to_bits())
            ]
        );
        // `:W)` with no bracket open is dropped; a `:` that is no weight is text
        assert_eq!(exact("se:1)xy"), [("sexy".to_owned(), one)]);
        assert_eq!(
            exact("a:b) c:.) d:1.2.3)"),
            [("a:b) c:.) d:1.2.3)".to_owned(), one)]
        );
        // an unmatched closer is text; BREAK takes the weight of a bracket around it
        assert_eq!(
            exact("x) y] (BREAK)"),
            [
                ("x) y] ".to_owned(), one),
                // -1 times 1.1, exactly -1.1
                ("BREAK".to_owned(), (-1.1f32).to_bits())
            ]
        );
        assert_eq!(
            exact("[(a:+.5)]"),
            [("a".to_owned(), (0.5f32 / 1.1).to_bits())]
        );
    }

    #[test]
    fn the_model_reads_the_words_the_brackets_join() {
        for (typed, read) in [
            ("se()xy chil()dren", "sexy children"),
            ("se[]xy chil[]dren", "sexy children"),
            ("se:1)xy chil:1)dren", "sexy children"),
            ("se(xy:1) chil(dren:1)", "sexy children"),
            ("(se)(xy) (chil)(dren)", "sexy children"),
            ("sexy little(:1) girl", "sexy little girl"),
            ("a (red:1.2) car on a [wet] road", "a red car on a wet road"),
            (
                "portrait, (soft light), BREAK, city at night",
                "portrait, soft light,  , city at night",
            ),
        ] {
            assert_eq!(attention_text(typed), read, "{typed:?}");
        }
    }

    #[test]
    fn typed_image_markers_never_reach_the_engine() {
        for (typed, kept) in [
            ("a fox <image:/etc/passwd> in snow", "a fox  in snow"),
            ("a fox <image:x", "a fox x"),
            ("<image:>", ">"),
            ("\\<image:C:\\x.png>", "\\"),
            // a marker inside a LoRA tag, and a LoRA tag inside a marker
            ("<lora:<image:/etc/passwd>:1>", ":1>"),
            ("<lora:a:<image:x>1>", "1>"),
            ("<image:/etc/<lora:a:1>passwd>", ""),
            ("<image:<lora:a:1>>", ">"),
            // removing one marker joins another
            ("<ima<lora:ge:/etc/passwd", "/etc/passwd"),
            ("<lo<image:x>ra:evil:3>", "evil:3>"),
            ("<IMAGE:x>", "<IMAGE:x>"),
        ] {
            let stripped = strip_prompt_markers(typed);
            assert_eq!(stripped, kept, "{typed:?}");
            assert!(
                !stripped.contains("<image:") && !stripped.contains("<lora:"),
                "{typed:?}"
            );
        }
    }

    /// The image-marker pass is linear too: openers that never close, empty paths, and one
    /// `>` at the very end (the engine's path runs from the first `<image:` to it: one
    /// marker).
    #[test]
    fn a_pathological_marker_prompt_strips_in_linear_time() {
        let n = 100_000;
        assert_eq!(strip_prompt_markers(&"<image:a".repeat(n)), "a".repeat(n));
        assert_eq!(strip_prompt_markers(&"<image:>".repeat(n)), ">".repeat(n));
        let one = format!("{}>", "<image:".repeat(n));
        assert_eq!(strip_prompt_markers(&one), "");
        let nested = format!("{}<image:a>{}", "<ima".repeat(n), "ge:".repeat(n));
        assert_eq!(strip_prompt_markers(&nested), "");
        let brackets = "se()xy(".repeat(n);
        assert_eq!(attention_text(&brackets), "sexy".repeat(n));
    }

    /// The engine finds every span its pattern matches, so no `<lora:` the person typed may
    /// be left, alone or joined with what is around it or with the tags Studio appends.
    #[test]
    fn no_typed_lora_tag_survives_the_strip() {
        for (typed, kept) in [
            ("<lora:a:1>", ""),
            ("x <lora:a:1><lora:b:2> y", "x  y"),
            // a span removed joins its two sides into a new opener
            ("<lo<lora:a:1>ra:evil:3>", "evil:3>"),
            ("<lo<lora:ra:x", "x"),
            ("<lo<lora:ra:x:1>", "<lo"),
            ("<lora:<lora:a:1>b:1>", "b:1>"),
            // openers without a whole tag: an appended tag would close them
            ("a fox <lora:evil:3", "a fox evil:3"),
            ("a fox <lora:evil", "a fox evil"),
            ("<lora:>x", ">x"),
            ("<lora::1>", ":1>"),
            // the weight may hold anything but `>`, a line break too
            ("<lora:a:\n1>z", "z"),
            ("<LORA:a:1>", "<LORA:a:1>"),
            ("風 <lora:日本:1> 景", "風  景"),
        ] {
            let stripped = strip_prompt_markers(typed);
            assert_eq!(stripped, kept, "{typed:?}");
            assert!(!stripped.contains("<lora:"), "{typed:?}");
        }
    }

    /// A prompt built to make a repeat-until-stable strip, or a scan to the end for each
    /// `<lora:`, take quadratic time is stripped in one pass each (a few milliseconds here;
    /// minutes if either came back).
    #[test]
    fn a_pathological_prompt_strips_in_linear_time() {
        let n = 100_000;
        // openers that never close, each name ended by the next opener's colon
        let open = "<lora:a:".repeat(n);
        assert_eq!(strip_prompt_markers(&open), "a:".repeat(n));
        // openers with empty names, one `>` at the very end
        let empty = format!("{}>", "<lora::".repeat(n));
        assert_eq!(strip_prompt_markers(&empty), format!("{}>", ":".repeat(n)));
        // nested joins, each removal making a new opener
        let nested = format!("{}<lora:a:1>{}", "<lo".repeat(n), "ra:".repeat(n));
        assert_eq!(strip_prompt_markers(&nested), "");
    }
}
