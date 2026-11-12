//! Blanks out comments and string contents, keeping every line and column where it was.
//!
//! The definition finder matches line patterns and counts brackets. Both go wrong on text
//! inside a comment or a string (`// fn old()`, `"class {"`), so this pass replaces such
//! text with spaces first. It is a small state machine per language family, not a lexer:
//! where it cannot tell (a quote with no partner on its line), it leaves the text alone.

/// How a language writes `'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Single {
    /// Not a literal (Swift).
    None,
    /// A string (JavaScript, TypeScript, Python).
    Str,
    /// A character literal (C, C++, Java, C#, Kotlin, Go).
    Char,
    /// A character literal or a lifetime (Rust).
    RustChar,
}

/// How a language writes a backtick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Backtick {
    /// Not a literal (or an escaped identifier: Kotlin, Swift).
    None,
    /// A raw string that may span lines (Go).
    Raw,
    /// A template literal with `${ }` holes (JavaScript, TypeScript).
    Template,
}

/// The lexical rules of one language family.
#[derive(Debug, Clone, Copy)]
pub(super) struct Rules {
    /// `#` starts a comment (Python).
    pub hash_comments: bool,
    /// `//` and `/* */` are comments.
    pub slash_comments: bool,
    /// `/* /* */ */` nests (Rust, Kotlin, Swift).
    pub nested_block: bool,
    pub single: Single,
    pub backtick: Backtick,
    /// `"""` starts a string that may span lines.
    pub triple_double: bool,
    /// `'''` starts a string that may span lines (Python).
    pub triple_single: bool,
    /// A backslash escapes the next character inside a triple-quoted string.
    pub triple_escapes: bool,
    /// `r"…"`, `r#"…"#` (Rust).
    pub rust_raw: bool,
    /// `R"delim(…)delim"` (C++).
    pub cpp_raw: bool,
    /// `@"…"` with `""` for a quote (C#).
    pub verbatim_at: bool,
    /// A line starting with `#` is a preprocessor line (C, C++, C#).
    pub preprocessor: bool,
    /// `/…/` can be a regular expression (JavaScript, TypeScript).
    pub regex_literals: bool,
}

impl Rules {
    pub(super) const C_LIKE: Rules = Rules {
        hash_comments: false,
        slash_comments: true,
        nested_block: false,
        single: Single::Char,
        backtick: Backtick::None,
        triple_double: false,
        triple_single: false,
        triple_escapes: true,
        rust_raw: false,
        cpp_raw: false,
        verbatim_at: false,
        preprocessor: false,
        regex_literals: false,
    };
}

/// The text with comments and string contents blanked, line by line.
#[derive(Debug, Default)]
pub(super) struct Masked {
    /// Each line, the same length in bytes as the original line.
    pub lines: Vec<String>,
    /// The line holds a comment and nothing else.
    pub comment_only: Vec<bool>,
    /// The line starts inside a string or comment that began on an earlier line.
    pub in_literal: Vec<bool>,
    /// `{` depth at the start of each line.
    pub curly: Vec<u32>,
    /// `(` and `[` depth at the start of each line.
    pub round: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Code,
    LineComment,
    BlockComment(u32),
    /// A triple-quoted string; the byte is the quote.
    Triple(u8),
    RustRaw(usize),
    CppRaw(Vec<u8>),
    Verbatim,
    RawBacktick,
    Template,
    Preprocessor,
}

/// One level of a template literal: the literal itself, or a `${ }` hole with its brace depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frame {
    Template,
    Hole(u32),
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// Length in bytes of the UTF-8 character starting with `lead`.
fn char_len(lead: u8) -> usize {
    match lead {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

/// The index of the closing `quote` on the same line, honouring backslash escapes.
fn closing_on_line(src: &[u8], from: usize, quote: u8, limit: usize) -> Option<usize> {
    let mut i = from;
    let end = src.len().min(from.saturating_add(limit));
    while i < end {
        match src[i] {
            b'\n' => return None,
            b'\\' => i += 2,
            c if c == quote => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// The closing `/` of a regular expression literal starting after `from`, on the same line.
fn closing_regex(src: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    let mut class = false;
    while i < src.len() {
        match src[i] {
            b'\n' => return None,
            b'\\' => i += 1,
            b'[' => class = true,
            b']' => class = false,
            b'/' if !class => return (i > from).then_some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// Whether a `/` at this point starts a regular expression: it follows an operator, an
/// opening bracket, `return` or nothing, where a value is expected.
fn regex_can_start(line_so_far: &[u8]) -> bool {
    let trimmed = line_so_far.trim_ascii_end();
    let Some(&last) = trimmed.last() else {
        return true;
    };
    if b"(,=:[!&|?{};".contains(&last) {
        return true;
    }
    let word_start = trimmed
        .iter()
        .rposition(|b| !is_ident(*b))
        .map_or(0, |p| p + 1);
    matches!(&trimmed[word_start..], b"return" | b"typeof" | b"case")
}

/// Blanks comments and string contents in `text`.
pub(super) fn mask(text: &str, rules: &Rules) -> Masked {
    let src = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(src.len());
    let mut masked = Masked::default();

    let mut state = State::Code;
    let mut frames: Vec<Frame> = Vec::new();
    let mut curly: u32 = 0;
    let mut round: u32 = 0;
    // Per line.
    let mut line_start = 0usize;
    let mut saw_code = false;
    let mut saw_comment = false;
    let mut saw_string = false;
    let mut first_non_space = true;

    masked.in_literal.push(false);
    masked.curly.push(0);
    masked.round.push(0);

    let mut i = 0usize;
    while i < src.len() {
        let b = src[i];
        if b == b'\n' {
            // A preprocessor line continues after a backslash; a line comment never does.
            let continued = i > 0 && src[i - 1] == b'\\';
            match state {
                State::LineComment => state = State::Code,
                State::Preprocessor if !continued => state = State::Code,
                _ => {}
            }
            masked
                .lines
                .push(String::from_utf8_lossy(&out[line_start..]).into_owned());
            // An empty line in the middle of a block comment belongs to the comment.
            let in_block = matches!(state, State::BlockComment(_));
            masked
                .comment_only
                .push(!saw_code && !saw_string && (saw_comment || in_block));
            out.push(b'\n');
            line_start = out.len();
            masked
                .in_literal
                .push(state != State::Code || !frames.is_empty());
            masked.curly.push(curly);
            masked.round.push(round);
            saw_code = false;
            saw_comment = false;
            saw_string = false;
            first_non_space = true;
            i += 1;
            continue;
        }
        if b == b'\r' {
            out.push(b' ');
            i += 1;
            continue;
        }
        match &state {
            State::LineComment | State::Preprocessor => {
                saw_comment = true;
                out.push(b' ');
                i += 1;
            }
            State::BlockComment(depth) => {
                let depth = *depth;
                saw_comment = true;
                if b == b'*' && src.get(i + 1) == Some(&b'/') {
                    out.extend_from_slice(b"  ");
                    i += 2;
                    state = if depth > 1 {
                        State::BlockComment(depth - 1)
                    } else {
                        State::Code
                    };
                } else if rules.nested_block && b == b'/' && src.get(i + 1) == Some(&b'*') {
                    out.extend_from_slice(b"  ");
                    i += 2;
                    state = State::BlockComment(depth + 1);
                } else {
                    out.push(b' ');
                    i += 1;
                }
            }
            State::Triple(quote) => {
                let quote = *quote;
                saw_string = true;
                if rules.triple_escapes && b == b'\\' && src.get(i + 1).is_some_and(|n| *n != b'\n')
                {
                    out.extend_from_slice(b"  ");
                    i += 2;
                } else if b == quote
                    && src.get(i + 1) == Some(&quote)
                    && src.get(i + 2) == Some(&quote)
                {
                    out.extend_from_slice(b"   ");
                    i += 3;
                    state = State::Code;
                } else {
                    out.push(b' ');
                    i += 1;
                }
            }
            State::RustRaw(hashes) => {
                let hashes = *hashes;
                saw_string = true;
                let closes = b == b'"'
                    && src.len() >= i + 1 + hashes
                    && src[i + 1..i + 1 + hashes].iter().all(|h| *h == b'#');
                if closes {
                    out.extend(std::iter::repeat_n(b' ', 1 + hashes));
                    i += 1 + hashes;
                    state = State::Code;
                } else {
                    out.push(b' ');
                    i += 1;
                }
            }
            State::CppRaw(delim) => {
                saw_string = true;
                // `)delim"`
                let closes = b == b')'
                    && src[i + 1..].starts_with(delim)
                    && src.get(i + 1 + delim.len()) == Some(&b'"');
                if closes {
                    let n = delim.len() + 2;
                    out.extend(std::iter::repeat_n(b' ', n));
                    i += n;
                    state = State::Code;
                } else {
                    out.push(b' ');
                    i += 1;
                }
            }
            State::Verbatim => {
                saw_string = true;
                if b == b'"' && src.get(i + 1) == Some(&b'"') {
                    out.extend_from_slice(b"  ");
                    i += 2;
                } else if b == b'"' {
                    out.push(b' ');
                    i += 1;
                    state = State::Code;
                } else {
                    out.push(b' ');
                    i += 1;
                }
            }
            State::RawBacktick => {
                saw_string = true;
                out.push(b' ');
                i += 1;
                if b == b'`' {
                    state = State::Code;
                }
            }
            State::Template => {
                saw_string = true;
                if b == b'\\' && src.get(i + 1).is_some_and(|n| *n != b'\n') {
                    out.extend_from_slice(b"  ");
                    i += 2;
                } else if b == b'`' {
                    out.push(b' ');
                    i += 1;
                    frames.pop();
                    state = State::Code;
                } else if b == b'$' && src.get(i + 1) == Some(&b'{') {
                    out.extend_from_slice(b"  ");
                    i += 2;
                    frames.push(Frame::Hole(0));
                    state = State::Code;
                } else {
                    out.push(b' ');
                    i += 1;
                }
            }
            State::Code => {
                // Inside a `${ }` hole everything is blanked, but still read as code so the
                // end of the hole and of the template are found.
                let hidden = !frames.is_empty();
                let was_first = first_non_space;
                if !b.is_ascii_whitespace() {
                    first_non_space = false;
                }
                let next = src.get(i + 1).copied();

                if rules.preprocessor && was_first && b == b'#' && !hidden {
                    state = State::Preprocessor;
                    saw_comment = true;
                    out.push(b' ');
                    i += 1;
                    continue;
                }
                if rules.slash_comments && b == b'/' && next == Some(b'/') {
                    state = State::LineComment;
                    saw_comment = true;
                    out.push(b' ');
                    i += 1;
                    continue;
                }
                if rules.slash_comments && b == b'/' && next == Some(b'*') {
                    state = State::BlockComment(1);
                    saw_comment = true;
                    out.extend_from_slice(b"  ");
                    i += 2;
                    continue;
                }
                if rules.hash_comments && b == b'#' {
                    state = State::LineComment;
                    saw_comment = true;
                    out.push(b' ');
                    i += 1;
                    continue;
                }
                // Not inside a `${ }` hole: there a division is likelier, and a wrong guess
                // would swallow the end of the hole.
                if rules.regex_literals
                    && !hidden
                    && b == b'/'
                    && regex_can_start(&out[line_start..])
                    && let Some(close) = closing_regex(src, i + 1)
                {
                    saw_string = true;
                    out.extend(std::iter::repeat_n(b' ', close + 1 - i));
                    i = close + 1;
                    continue;
                }
                let prev_ident = i > 0 && is_ident(src[i - 1]);
                if rules.rust_raw && (b == b'r' || b == b'b') && !prev_ident {
                    // r"…", r#"…"#, br"…", br#"…"#
                    let mut j = i;
                    if src[j] == b'b' {
                        j += 1;
                    }
                    if src.get(j) == Some(&b'r') {
                        j += 1;
                        let hashes = src[j..].iter().take_while(|h| **h == b'#').count();
                        if src.get(j + hashes) == Some(&b'"') {
                            saw_string = true;
                            out.extend(std::iter::repeat_n(b' ', j + hashes + 1 - i));
                            i = j + hashes + 1;
                            state = State::RustRaw(hashes);
                            continue;
                        }
                    }
                }
                if rules.cpp_raw && b == b'R' && next == Some(b'"') && !prev_ident {
                    let rest = &src[i + 2..];
                    let open = rest.iter().take(17).position(|c| *c == b'(');
                    if let Some(open) = open
                        && !rest[..open].contains(&b'\n')
                    {
                        saw_string = true;
                        let delim = rest[..open].to_vec();
                        out.extend(std::iter::repeat_n(b' ', open + 3));
                        i += open + 3;
                        state = State::CppRaw(delim);
                        continue;
                    }
                }
                if rules.verbatim_at && b == b'@' && next == Some(b'"') {
                    saw_string = true;
                    out.extend_from_slice(b"  ");
                    i += 2;
                    state = State::Verbatim;
                    continue;
                }
                if b == b'"' || (b == b'\'' && rules.single == Single::Str) {
                    let triple =
                        (b == b'"' && rules.triple_double) || (b == b'\'' && rules.triple_single);
                    if triple && next == Some(b) && src.get(i + 2) == Some(&b) {
                        saw_string = true;
                        out.extend_from_slice(b"   ");
                        i += 3;
                        state = State::Triple(b);
                        continue;
                    }
                    // A string closes on its own line; a quote with no partner is left alone.
                    if let Some(close) = closing_on_line(src, i + 1, b, usize::MAX) {
                        saw_string = true;
                        out.extend(std::iter::repeat_n(b' ', close + 1 - i));
                        i = close + 1;
                        continue;
                    }
                }
                if b == b'\'' {
                    let literal = match rules.single {
                        Single::Char => closing_on_line(src, i + 1, b'\'', 12),
                        Single::RustChar => match next {
                            Some(b'\\') => closing_on_line(src, i + 1, b'\'', 12),
                            // 'x' is a character; 'a without a closing quote is a lifetime.
                            Some(c) => {
                                let close = i + 1 + char_len(c);
                                (src.get(close) == Some(&b'\'')).then_some(close)
                            }
                            None => None,
                        },
                        Single::None | Single::Str => None,
                    };
                    if let Some(close) = literal {
                        saw_string = true;
                        out.extend(std::iter::repeat_n(b' ', close + 1 - i));
                        i = close + 1;
                        continue;
                    }
                }
                if b == b'`' {
                    match rules.backtick {
                        Backtick::Raw => {
                            saw_string = true;
                            out.push(b' ');
                            i += 1;
                            state = State::RawBacktick;
                            continue;
                        }
                        Backtick::Template => {
                            saw_string = true;
                            out.push(b' ');
                            i += 1;
                            frames.push(Frame::Template);
                            state = State::Template;
                            continue;
                        }
                        Backtick::None => {}
                    }
                }
                if hidden {
                    saw_string = true;
                    if let Some(Frame::Hole(depth)) = frames.last_mut() {
                        match b {
                            b'{' => *depth += 1,
                            b'}' if *depth == 0 => {
                                frames.pop();
                                state = State::Template;
                            }
                            b'}' => *depth -= 1,
                            _ => {}
                        }
                    }
                    out.push(b' ');
                    i += 1;
                    continue;
                }
                match b {
                    b'{' => curly += 1,
                    b'}' => {
                        curly = curly.saturating_sub(1);
                        // A closing brace in the first column ends a top-level block. If the
                        // count drifted (an `#if` with half a block in each branch), this
                        // puts it right again.
                        if i == 0 || src[i - 1] == b'\n' {
                            curly = 0;
                            round = 0;
                        }
                    }
                    b'(' | b'[' => round += 1,
                    b')' | b']' => round = round.saturating_sub(1),
                    _ => {}
                }
                if !b.is_ascii_whitespace() {
                    saw_code = true;
                }
                out.push(b);
                i += 1;
            }
        }
    }
    // The last line, when the text does not end with a newline.
    if line_start < out.len() {
        masked
            .lines
            .push(String::from_utf8_lossy(&out[line_start..]).into_owned());
        masked
            .comment_only
            .push(saw_comment && !saw_code && !saw_string);
    }
    let n = masked.lines.len();
    masked.in_literal.truncate(n);
    masked.curly.truncate(n);
    masked.round.truncate(n);
    masked
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST: Rules = Rules {
        nested_block: true,
        single: Single::RustChar,
        rust_raw: true,
        ..Rules::C_LIKE
    };
    const JS: Rules = Rules {
        single: Single::Str,
        backtick: Backtick::Template,
        regex_literals: true,
        ..Rules::C_LIKE
    };
    const PYTHON: Rules = Rules {
        hash_comments: true,
        slash_comments: false,
        single: Single::Str,
        triple_double: true,
        triple_single: true,
        ..Rules::C_LIKE
    };

    fn lines(text: &str, rules: &Rules) -> Vec<String> {
        mask(text, rules).lines
    }

    #[test]
    fn every_line_keeps_its_length_and_the_text_keeps_its_lines() {
        let text = "a // b\n\"c\" d /* e\nf */ g\n\nh";
        let m = mask(text, &Rules::C_LIKE);
        let original: Vec<&str> = text.split('\n').collect();
        assert_eq!(m.lines.len(), original.len());
        for (masked, line) in m.lines.iter().zip(&original) {
            assert_eq!(masked.len(), line.len(), "{masked:?} / {line:?}");
        }
        assert_eq!(m.lines, vec!["a     ", "    d     ", "     g", "", "h"]);
        assert_eq!(m.in_literal, vec![false, false, true, false, false]);
        assert_eq!(m.comment_only, vec![false, false, false, false, false]);
        // A trailing newline does not add an empty line.
        assert_eq!(mask("a\nb\n", &Rules::C_LIKE).lines, vec!["a", "b"]);
        assert!(mask("", &Rules::C_LIKE).lines.is_empty());
    }

    #[test]
    fn comment_only_lines_are_marked() {
        let m = mask(
            "/// doc\nfn a() {} // tail\n/* one\n   two */\n\"s\"\n",
            &RUST,
        );
        assert_eq!(m.comment_only, vec![true, false, true, true, false]);
    }

    #[test]
    fn brackets_in_strings_and_comments_do_not_count() {
        let m = mask(
            "fn a() {\n    let s = \"}{(\"; // }\n    /* { */\n}\nfn b() {}\n",
            &RUST,
        );
        assert_eq!(m.curly, vec![0, 1, 1, 1, 0]);
        assert_eq!(m.round, vec![0, 0, 0, 0, 0]);
        let m = mask("f(\n  a,\n  [b,\n  c],\n)\n", &Rules::C_LIKE);
        assert_eq!(m.round, vec![0, 1, 1, 2, 1]);
    }

    #[test]
    fn rust_lifetimes_are_not_character_literals() {
        let l = lines(
            "fn a<'a>(x: &'a str) -> char { '{' }\nlet c = '\\'';\nlet d = b'x';\n",
            &RUST,
        );
        assert_eq!(l[0], "fn a<'a>(x: &'a str) -> char {     }");
        assert_eq!(l[1], "let c =     ;");
        assert_eq!(l[2], "let d = b   ;");
    }

    #[test]
    fn rust_raw_strings_and_nested_comments() {
        let l = lines(
            "let a = r#\"fn x() { \"quoted\" }\"#;\nlet b = r\"\\\";\n/* a /* b */ fn c() */ fn d() {}\n",
            &RUST,
        );
        assert_eq!(l[0].trim_end_matches(';').trim_end(), "let a =");
        assert!(l[0].ends_with(';'), "{:?}", l[0]);
        // r"\" is four characters: a backslash does not escape in a raw string.
        assert_eq!(l[1], format!("let b = {};", " ".repeat(4)));
        assert_eq!(l[2].trim(), "fn d() {}");
        // A raw string over several lines.
        let m = mask("let s = r#\"\nfn fake() {\n\"#;\nfn real() {}\n", &RUST);
        assert_eq!(m.lines[1].trim(), "");
        assert_eq!(m.in_literal, vec![false, true, true, false]);
        assert_eq!(m.lines[3], "fn real() {}");
    }

    #[test]
    fn a_quote_with_no_partner_on_its_line_is_left_alone() {
        // JSX text with an apostrophe: the brace after it still counts.
        let m = mask("<p>Don't {\n  x\n}</p>\nconst a = 'it''s';\n", &JS);
        assert_eq!(m.lines[0], "<p>Don't {");
        assert_eq!(m.curly, vec![0, 1, 1, 0]);
        // 'it' and 's' are two strings: seven characters blanked.
        assert_eq!(m.lines[3], format!("const a = {};", " ".repeat(7)));
    }

    #[test]
    fn template_literals_with_holes_and_nesting() {
        let text = "const a = `x ${b ? `c{` : `d}`} } y`;\nfunction f() {}\nconst m = `line\nfunction fake() {\n${ { a: 1 }.a }`;\nclass K {}\n";
        let m = mask(text, &JS);
        assert_eq!(m.lines[0].trim_end_matches(';').trim(), "const a =");
        assert_eq!(m.lines[1], "function f() {}");
        assert_eq!(m.lines[3].trim(), "");
        assert_eq!(m.lines[4].trim(), ";");
        assert_eq!(m.lines[5], "class K {}");
        assert_eq!(m.curly, vec![0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn regex_literals_are_blanked_only_where_a_value_can_start() {
        let l = lines(
            "const re = /[\"'/]{2}/g;\nconst half = a / b / c;\nif (/^\\/\\//.test(x)) {\nreturn /\\{/.test(y);\n",
            &JS,
        );
        assert_eq!(l[0], "const re =           g;");
        assert_eq!(l[1], "const half = a / b / c;");
        assert_eq!(l[2], "if (       .test(x)) {");
        assert_eq!(l[3], "return     .test(y);");
    }

    #[test]
    fn python_strings_comments_and_docstrings() {
        let text = "def a():\n    \"\"\"Doc.\n\n    def fake(): pass\n    \"\"\"\n    x = '#not a comment'  # def also_fake():\n    y = '''z\n    class Fake:\n    '''\n";
        let m = mask(text, &PYTHON);
        assert_eq!(m.lines[0], "def a():");
        assert_eq!(m.lines[3].trim(), "");
        assert_eq!(m.lines[5].trim_end(), "    x =");
        assert_eq!(m.lines[7].trim(), "");
        assert_eq!(
            m.in_literal,
            vec![false, false, true, true, true, false, false, true, true]
        );
    }

    #[test]
    fn preprocessor_lines_and_their_continuations_are_blanked() {
        let rules = Rules {
            preprocessor: true,
            cpp_raw: true,
            ..Rules::C_LIKE
        };
        let m = mask(
            "#define OPEN {\n#define TWO(a) \\\n    { a }\nint f(void) { return '{'; }\nconst char *s = R\"x(}{)x\";\n",
            &rules,
        );
        assert_eq!(m.lines[0].trim(), "");
        assert_eq!(m.lines[2].trim(), "");
        assert_eq!(m.lines[3], "int f(void) { return    ; }");
        // The raw string is nine characters long.
        assert_eq!(m.lines[4], format!("const char *s = {};", " ".repeat(9)));
        assert_eq!(m.curly, vec![0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_closing_brace_in_the_first_column_resets_a_drifted_count() {
        let rules = Rules {
            preprocessor: true,
            ..Rules::C_LIKE
        };
        // Both branches open the block; only one closing brace follows.
        let text = "int f(int a)\n{\n#ifdef A\n    if (a) {\n#else\n    if (!a) {\n#endif\n        g();\n    }\n}\nint h(void) { return 1; }\n";
        let m = mask(text, &rules);
        assert_eq!(*m.curly.last().unwrap(), 0);
    }

    #[test]
    fn go_raw_strings_and_csharp_verbatim_strings_span_lines() {
        let go = Rules {
            backtick: Backtick::Raw,
            ..Rules::C_LIKE
        };
        let m = mask("var q = `\nfunc fake() {\n`\nfunc real() {}\n", &go);
        assert_eq!(m.lines[1].trim(), "");
        assert_eq!(m.lines[3], "func real() {}");
        let cs = Rules {
            verbatim_at: true,
            ..Rules::C_LIKE
        };
        let m = mask(
            "var s = @\"a \"\"b\"\"\nclass Fake {\n\";\nclass Real {}\n",
            &cs,
        );
        assert_eq!(m.lines[1].trim(), "");
        assert_eq!(m.lines[2].trim(), ";");
        assert_eq!(m.lines[3], "class Real {}");
    }

    #[test]
    fn text_that_is_not_ascii_keeps_valid_lines() {
        let m = mask("let s = \"héllo → wörld\"; // ünïcode\nlet é = 1;\n", &RUST);
        assert_eq!(m.lines[1], "let é = 1;");
        assert!(m.lines[0].starts_with("let s ="));
        assert_eq!(
            m.lines[0].len(),
            "let s = \"héllo → wörld\"; // ünïcode".len()
        );
    }
}
