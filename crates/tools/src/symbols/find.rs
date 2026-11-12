//! The definition finder: line patterns over masked text (see [`super::lex`]).
//!
//! It is not a parser. Each line is looked at once, on text whose comments and strings
//! were blanked, so `// fn old()` and `"class {"` are never matched. Two kinds of pattern:
//!
//! - a definition that starts with a keyword (`fn`, `class`, `def`, `func`, ...) is taken
//!   at any depth;
//! - a definition that can only be guessed from its shape (`Type name(` in Java, C#, C and
//!   C++) is taken only directly inside a class body or at file or namespace level, never
//!   on a line that starts inside brackets, and only when a body follows.
//!
//! A definition ends where its block ends: the matching brace, or for Python the last line
//! indented deeper than the header. When the brace is never closed, it ends 40 lines on.

use super::lex::{self, Backtick, Masked, Rules, Single};

mod c;
mod go;
mod managed;
mod python;
mod rust;
mod script;
#[cfg(test)]
mod tests;

/// Definitions kept from one file at most.
pub const MAX_SYMBOLS_PER_FILE: usize = 2_000;
/// Lines a definition's header may span before its body or its end is found.
const HEADER_LINES: usize = 30;
/// Lines given to a definition whose block has no end.
const FALLBACK_LINES: usize = 40;
/// Comment and attribute lines counted above a definition at most.
const LEAD_LINES: usize = 50;
/// A name longer than this is not a name.
const MAX_NAME_BYTES: usize = 200;
const NO_LINE: u32 = u32::MAX;

/// A language the finder knows, told by the file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    Java,
    CSharp,
    Kotlin,
    Swift,
    C,
    Cpp,
}

const RUST: Rules = Rules {
    nested_block: true,
    single: Single::RustChar,
    rust_raw: true,
    ..Rules::C_LIKE
};
const SCRIPT: Rules = Rules {
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
const GO: Rules = Rules {
    backtick: Backtick::Raw,
    ..Rules::C_LIKE
};
const JAVA: Rules = Rules {
    triple_double: true,
    ..Rules::C_LIKE
};
const CSHARP: Rules = Rules {
    verbatim_at: true,
    preprocessor: true,
    triple_double: true,
    triple_escapes: false,
    ..Rules::C_LIKE
};
const KOTLIN: Rules = Rules {
    nested_block: true,
    triple_double: true,
    triple_escapes: false,
    ..Rules::C_LIKE
};
const SWIFT: Rules = Rules {
    nested_block: true,
    single: Single::None,
    triple_double: true,
    ..Rules::C_LIKE
};
const C: Rules = Rules {
    preprocessor: true,
    ..Rules::C_LIKE
};
const CPP: Rules = Rules {
    preprocessor: true,
    cpp_raw: true,
    ..Rules::C_LIKE
};

impl Lang {
    /// The language of a file, by its extension. `.h` is read as C++, which also covers C.
    pub fn from_path(path: &str) -> Option<Lang> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let (stem, ext) = name.rsplit_once('.')?;
        if stem.is_empty() {
            return None;
        }
        Some(match ext.to_ascii_lowercase().as_str() {
            "rs" => Lang::Rust,
            "ts" | "tsx" | "mts" | "cts" => Lang::TypeScript,
            "js" | "jsx" | "mjs" | "cjs" => Lang::JavaScript,
            "py" | "pyi" => Lang::Python,
            "go" => Lang::Go,
            "java" => Lang::Java,
            "cs" => Lang::CSharp,
            "kt" | "kts" => Lang::Kotlin,
            "swift" => Lang::Swift,
            "c" => Lang::C,
            "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Lang::Cpp,
            _ => return None,
        })
    }

    /// What joins a type and its member in a name: `Foo::new` or `Foo.new`.
    pub fn member_separator(self) -> &'static str {
        match self {
            Lang::Rust | Lang::C | Lang::Cpp => "::",
            _ => ".",
        }
    }

    fn rules(self) -> Rules {
        match self {
            Lang::Rust => RUST,
            Lang::TypeScript | Lang::JavaScript => SCRIPT,
            Lang::Python => PYTHON,
            Lang::Go => GO,
            Lang::Java => JAVA,
            Lang::CSharp => CSHARP,
            Lang::Kotlin => KOTLIN,
            Lang::Swift => SWIFT,
            Lang::C => C,
            Lang::Cpp => CPP,
        }
    }

    /// A statement ends with its line unless the line visibly goes on (no `;` needed).
    fn newline_ends(self) -> bool {
        matches!(
            self,
            Lang::TypeScript | Lang::JavaScript | Lang::Go | Lang::Kotlin | Lang::Swift
        )
    }

    /// `$` may be part of a name.
    fn dollar_names(self) -> bool {
        matches!(
            self,
            Lang::TypeScript | Lang::JavaScript | Lang::Java | Lang::Kotlin
        )
    }
}

/// What a definition is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    /// A Rust `impl` block or a Swift `extension`.
    Impl,
    Module,
    Const,
    Type,
}

impl SymbolKind {
    const ALL: [SymbolKind; 11] = [
        SymbolKind::Function,
        SymbolKind::Method,
        SymbolKind::Class,
        SymbolKind::Struct,
        SymbolKind::Enum,
        SymbolKind::Trait,
        SymbolKind::Interface,
        SymbolKind::Impl,
        SymbolKind::Module,
        SymbolKind::Const,
        SymbolKind::Type,
    ];

    /// The word shown to the user and sent to the webview.
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Class => "class",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Interface => "interface",
            SymbolKind::Impl => "impl",
            SymbolKind::Module => "module",
            SymbolKind::Const => "const",
            SymbolKind::Type => "type",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// One definition found in a file. Lines are 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: Box<str>,
    pub kind: SymbolKind,
    /// For a method, the type it belongs to. For a Rust trait impl, the trait.
    pub container: Option<Box<str>>,
    /// The line the definition starts on.
    pub line: u32,
    /// The first of the comment and attribute lines directly above it (`line` when none).
    pub lead: u32,
    /// The last line of its block.
    pub end_line: u32,
}

impl Symbol {
    /// The name with its type in front for a method: `Point::new`, `Store.create`.
    pub fn qualified(&self, lang: Lang) -> String {
        match (&self.container, self.kind) {
            (Some(container), SymbolKind::Method) => {
                format!("{container}{}{}", lang.member_separator(), self.name)
            }
            _ => self.name.to_string(),
        }
    }
}

/// The definitions in `text`, in the order they start.
pub fn find_symbols(text: &str, lang: Lang) -> Vec<Symbol> {
    let mut masked = lex::mask(text, &lang.rules());
    let (all_braces, depths) = braces(&masked);
    // The `{` depth of each line as the brace table counts it, so that scopes (from the
    // table) and lines (from this) agree after a `}` in the first column.
    masked.curly = depths;
    let mut scan = Scan::new(lang, &masked, text, all_braces);
    for i in 0..masked.lines.len() {
        if scan.out.len() >= MAX_SYMBOLS_PER_FILE {
            break;
        }
        scan.pop_ended(i);
        if masked.lines[i].trim().is_empty() {
            continue;
        }
        match lang {
            Lang::Rust => rust::line(&mut scan, i),
            Lang::TypeScript | Lang::JavaScript => script::line(&mut scan, i),
            Lang::Python => python::line(&mut scan, i),
            Lang::Go => go::line(&mut scan, i),
            Lang::Java | Lang::CSharp | Lang::Kotlin | Lang::Swift => managed::line(&mut scan, i),
            Lang::C | Lang::Cpp => c::line(&mut scan, i),
        }
    }
    scan.out.truncate(MAX_SYMBOLS_PER_FILE);
    scan.out
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$' || b >= 0x80
}

fn is_ident(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit()
}

/// Whether `text` starts with the whole word `word`.
fn starts_with_word(text: &str, word: &str) -> bool {
    text.strip_prefix(word)
        .is_some_and(|rest| !rest.bytes().next().is_some_and(is_ident))
}

/// The identifier `text` ends with (possibly empty), and where it starts.
fn trailing_word(text: &str) -> (usize, &str) {
    let start = text
        .bytes()
        .rposition(|b| !is_ident(b))
        .map_or(0, |p| p + 1);
    (start, &text[start..])
}

/// A reading position in one masked line.
#[derive(Debug, Clone, Copy)]
struct Cur<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Cur<'a> {
    fn new(s: &'a str) -> Self {
        Self { s, pos: 0 }
    }

    fn rest(&self) -> &'a str {
        &self.s[self.pos..]
    }

    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.pos).copied()
    }

    /// Skips spaces and tabs.
    fn ws(&mut self) {
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    /// The identifier here, if any.
    fn word(&self) -> Option<&'a str> {
        let bytes = self.s.as_bytes();
        if !self.peek().is_some_and(is_ident_start) {
            return None;
        }
        let mut end = self.pos;
        while end < bytes.len() && is_ident(bytes[end]) {
            end += 1;
        }
        Some(&self.s[self.pos..end])
    }

    fn take_word(&mut self) -> Option<&'a str> {
        let word = self.word()?;
        self.pos += word.len();
        Some(word)
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.rest().starts_with(lit) {
            self.pos += lit.len();
            true
        } else {
            false
        }
    }

    /// Takes `a.b.c` (or `a::b`) and returns it whole.
    fn take_path(&mut self, sep: &str) -> Option<&'a str> {
        let start = self.pos;
        self.take_word()?;
        loop {
            let before = *self;
            if !(self.eat(sep) && self.take_word().is_some()) {
                *self = before;
                break;
            }
        }
        Some(&self.s[start..self.pos])
    }

    /// At `open`: moves past the matching `close` on this line. False when it is not here.
    fn skip_balanced(&mut self, open: u8, close: u8) -> bool {
        let bytes = self.s.as_bytes();
        if self.peek() != Some(open) {
            return false;
        }
        let mut depth = 0u32;
        for (i, &b) in bytes.iter().enumerate().skip(self.pos) {
            if b == open {
                depth += 1;
            } else if b == close {
                depth -= 1;
                if depth == 0 {
                    self.pos = i + 1;
                    return true;
                }
            }
        }
        false
    }

    /// At `<`: moves past the matching `>` on this line, reading `->` and `=>` as arrows.
    fn skip_angles(&mut self) -> bool {
        let bytes = self.s.as_bytes();
        if self.peek() != Some(b'<') {
            return false;
        }
        let mut depth = 0u32;
        for (i, &b) in bytes.iter().enumerate().skip(self.pos) {
            match b {
                b'<' => depth += 1,
                b'>' if i > 0 && matches!(bytes[i - 1], b'-' | b'=') => {}
                b'>' => {
                    depth -= 1;
                    if depth == 0 {
                        self.pos = i + 1;
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }
}

/// One `{` and where it closes.
#[derive(Debug, Clone, Copy)]
struct Brace {
    line: u32,
    col: u32,
    close_line: u32,
    close_col: u32,
    /// The `{` depth inside it, counted as the masking pass counts it.
    depth: u32,
}

/// Every `{` of the masked text with its `}`, in the order they open, and the number of
/// blocks open at the start of each line.
///
/// A `}` in the first column heals a drifted count (an `#ifdef` with two opening lines, a
/// brace the masking pass misread): it closes every open block whose opening line was
/// indented, then one whose opening line starts in the first column. So the class closed by
/// `};` in an unindented C++ `namespace` closes the class only, not the namespace.
fn braces(m: &Masked) -> (Vec<Brace>, Vec<u32>) {
    let mut all: Vec<Brace> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut indented: Vec<bool> = Vec::new();
    let mut depths: Vec<u32> = Vec::with_capacity(m.lines.len());
    for (li, line) in m.lines.iter().enumerate() {
        depths.push(open.len() as u32);
        let line_indented = line.starts_with([' ', '\t']);
        for (ci, b) in line.bytes().enumerate() {
            match b {
                b'{' => {
                    open.push(all.len());
                    indented.push(line_indented);
                    all.push(Brace {
                        line: li as u32,
                        col: ci as u32,
                        close_line: NO_LINE,
                        close_col: 0,
                        depth: open.len() as u32,
                    });
                }
                b'}' => {
                    let drifted = if ci == 0 {
                        indented.iter().rev().take_while(|&&ind| ind).count()
                    } else {
                        0
                    };
                    let closes = (drifted + 1).min(open.len());
                    for _ in 0..closes {
                        if let Some(k) = open.pop() {
                            indented.pop();
                            all[k].close_line = li as u32;
                            all[k].close_col = ci as u32;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    (all, depths)
}

/// The block of a definition. Lines are indexes (0-based).
#[derive(Debug, Clone, Copy)]
struct Body {
    close: usize,
    close_col: usize,
    /// The `{` depth inside the block.
    depth: u32,
}

/// Where a definition ends, and its block when it has one.
#[derive(Debug, Clone, Copy)]
struct Extent {
    end: usize,
    body: Option<Body>,
}

/// What the members of an open block are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    /// A class, struct, interface, trait or impl block: functions in it are methods.
    Members,
    /// A namespace or an `extern "C"` block: the same as file level.
    Space,
    /// A Python function: a `def` in it is a plain function.
    Function,
}

#[derive(Debug)]
struct Scope {
    name: Box<str>,
    kind: ScopeKind,
    /// The `{` depth inside the block (for Python, the indentation of its header).
    depth: u32,
    end: usize,
}

struct Scan<'a> {
    lang: Lang,
    m: &'a Masked,
    /// The lines as written, for what masking hides (what a line ends with).
    orig: Vec<&'a str>,
    braces: Vec<Brace>,
    out: Vec<Symbol>,
    scopes: Vec<Scope>,
}

impl<'a> Scan<'a> {
    fn new(lang: Lang, m: &'a Masked, text: &'a str, braces: Vec<Brace>) -> Self {
        let mut orig: Vec<&str> = text.split('\n').collect();
        if orig.last() == Some(&"") {
            orig.pop();
        }
        Self {
            lang,
            m,
            orig,
            braces,
            out: Vec::new(),
            scopes: Vec::new(),
        }
    }

    fn original(&self, i: usize) -> &'a str {
        self.orig.get(i).copied().unwrap_or("")
    }

    fn pop_ended(&mut self, i: usize) {
        while self.scopes.last().is_some_and(|s| s.end < i) {
            self.scopes.pop();
        }
    }

    /// The block line `i` is directly inside, if it is one that was noted.
    fn inside(&self, i: usize) -> Option<&Scope> {
        self.scopes.last().filter(|s| s.depth == self.m.curly[i])
    }

    /// The type whose body line `i` is directly inside.
    fn member_of(&self, i: usize) -> Option<Box<str>> {
        self.inside(i)
            .filter(|s| s.kind == ScopeKind::Members)
            .map(|s| s.name.clone())
    }

    /// Whether line `i` is at file level or directly inside a noted block.
    fn at_scope(&self, i: usize) -> bool {
        match self.scopes.last() {
            None => self.m.curly[i] == 0,
            Some(s) => s.depth == self.m.curly[i],
        }
    }

    fn push_scope(&mut self, name: &str, kind: ScopeKind, i: usize, ext: &Extent) {
        if let Some(body) = ext.body
            && body.close > i
        {
            self.scopes.push(Scope {
                name: name.into(),
                kind,
                depth: body.depth,
                end: body.close,
            });
        }
    }

    fn add(
        &mut self,
        name: &str,
        kind: SymbolKind,
        container: Option<&str>,
        i: usize,
        ext: &Extent,
    ) {
        self.add_from(name, kind, container, i, i, ext);
    }

    /// Adds a definition whose first line is `first` and whose name is on line `i`.
    fn add_from(
        &mut self,
        name: &str,
        kind: SymbolKind,
        container: Option<&str>,
        first: usize,
        i: usize,
        ext: &Extent,
    ) {
        if name.is_empty()
            || name.len() > MAX_NAME_BYTES
            || name.as_bytes()[0].is_ascii_digit()
            || (name.contains('$') && !self.lang.dollar_names())
        {
            return;
        }
        self.out.push(Symbol {
            name: name.into(),
            kind,
            container: container.map(Into::into),
            line: i as u32 + 1,
            lead: self.lead(first) as u32 + 1,
            end_line: ext.end.max(i) as u32 + 1,
        });
    }

    /// The first of the comment and attribute lines directly above line `i`.
    fn lead(&self, i: usize) -> usize {
        let mut first = i;
        while first > 0 && i - first < LEAD_LINES {
            let above = first - 1;
            if self.original(above).trim().is_empty() {
                break;
            }
            let masked = self.m.lines[above].trim();
            let attribute = match self.lang {
                Lang::Rust => masked.starts_with("#["),
                Lang::CSharp => masked.starts_with('[') && masked.ends_with(']'),
                Lang::C | Lang::Cpp => starts_with_word(masked, "template"),
                Lang::Go => false,
                _ => masked.starts_with('@') && !masked.starts_with("@interface"),
            };
            if !(self.m.comment_only[above] || attribute) {
                break;
            }
            first = above;
        }
        first
    }

    fn brace_at(&self, line: usize, col: usize) -> Option<usize> {
        let key = (line as u32, col as u32);
        let k = self.braces.partition_point(|b| (b.line, b.col) < key);
        self.braces
            .get(k)
            .filter(|b| (b.line, b.col) == key)
            .map(|_| k)
    }

    /// The extent of a definition whose block was never closed.
    fn fallback(&self, line: usize) -> Extent {
        Extent {
            end: (line + FALLBACK_LINES - 1).min(self.m.lines.len().saturating_sub(1)),
            body: None,
        }
    }

    /// The block opening at `{` (`line`, `col`). When another block opens after it closes,
    /// on the same line, that one is the body: `(): { a: number } {`, `interface{} {`,
    /// `: a_{1} {}`. `None` when the block is never closed.
    fn block_from(&self, line: usize, col: usize) -> Option<Extent> {
        let mut k = self.brace_at(line, col)?;
        loop {
            let b = self.braces[k];
            if b.close_line == NO_LINE {
                return None;
            }
            let close = b.close_line as usize;
            let close_col = b.close_col as usize;
            let rest = &self.m.lines[close].as_bytes()[close_col + 1..];
            let mut next = None;
            for (off, ch) in rest.iter().enumerate() {
                match ch {
                    b'{' => {
                        next = Some(close_col + 1 + off);
                        break;
                    }
                    b';' | b'}' => break,
                    _ => {}
                }
            }
            match next.and_then(|c| self.brace_at(close, c)) {
                Some(n) => k = n,
                None => {
                    return Some(Extent {
                        end: close,
                        body: Some(Body {
                            close,
                            close_col,
                            depth: b.depth,
                        }),
                    });
                }
            }
        }
    }

    /// Where the definition whose header continues at (`line`, `col`) ends: at the block
    /// that follows, at a `;`, or where its line ends in a language without `;`.
    /// `comma_ends` is for C and C++ function guesses: a `,` after the parameters, before
    /// any `:`, means a declaration (`Foo a(1), b{2};`).
    fn extent(&self, line: usize, col: usize, comma_ends: bool) -> Extent {
        let lines = &self.m.lines;
        let bodiless = |l: usize| Extent {
            end: if l > line { l - 1 } else { line },
            body: None,
        };
        let mut depth = 0i32;
        let mut seen_colon = false;
        let (mut l, mut c) = (line, col);
        while l < lines.len() && l - line <= HEADER_LINES {
            let bytes = lines[l].as_bytes();
            while c < bytes.len() {
                match bytes[c] {
                    b'(' | b'[' => depth += 1,
                    b')' | b']' => {
                        depth -= 1;
                        if depth < 0 {
                            return bodiless(l);
                        }
                    }
                    b';' if depth == 0 => return Extent { end: l, body: None },
                    b',' if depth == 0 && comma_ends && !seen_colon => {
                        return Extent { end: l, body: None };
                    }
                    b':' if depth == 0 => seen_colon = true,
                    b'{' if depth == 0 => {
                        return self.block_from(l, c).unwrap_or(self.fallback(line));
                    }
                    b'}' if depth == 0 => return bodiless(l),
                    _ => {}
                }
                c += 1;
            }
            if depth == 0 && self.lang.newline_ends() && !self.continues(l) {
                return Extent { end: l, body: None };
            }
            l += 1;
            c = 0;
        }
        if l >= lines.len() {
            return Extent {
                end: lines.len().saturating_sub(1).max(line),
                body: None,
            };
        }
        self.fallback(line)
    }

    /// Whether the statement on line `l` goes on in the next line (languages without `;`).
    fn continues(&self, l: usize) -> bool {
        let cur = self.m.lines[l].trim_end();
        let orig = self.original(l).trim_end();
        // A line that ends with a string is complete, whatever stands before the string.
        let ends_with_literal = orig.len() > cur.len() && orig.ends_with(['"', '\'', '`']);
        if !ends_with_literal {
            if cur.ends_with("=>") || cur.ends_with("->") {
                return true;
            }
            if cur
                .as_bytes()
                .last()
                .is_some_and(|b| b",([=:|&<.\\".contains(b))
            {
                return true;
            }
            let (_, word) = trailing_word(cur);
            if matches!(
                word,
                "extends" | "implements" | "where" | "throws" | "in" | "is" | "as" | "by"
            ) {
                return true;
            }
        }
        let Some(next) = self.m.lines.get(l + 1) else {
            return false;
        };
        let next = next.trim_start();
        next.starts_with(['{', '.', '?', ':', '|', '&', '='])
            || next.starts_with("->")
            || ["extends", "implements", "where", "throws"]
                .iter()
                .any(|w| starts_with_word(next, w))
    }

    /// The `)` matching the `(` at (`line`, `col`), within the header limit.
    fn close_paren(&self, line: usize, col: usize) -> Option<(usize, usize)> {
        let lines = &self.m.lines;
        let mut depth = 0i32;
        let (mut l, mut c) = (line, col);
        while l < lines.len() && l - line <= HEADER_LINES {
            let bytes = lines[l].as_bytes();
            while c < bytes.len() {
                match bytes[c] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some((l, c));
                        }
                    }
                    _ => {}
                }
                c += 1;
            }
            l += 1;
            c = 0;
        }
        None
    }

    /// Whether `=>` follows the parameter list opening at (`line`, `col`).
    fn arrow_after_params(&self, line: usize, col: usize) -> bool {
        self.close_paren(line, col)
            .is_some_and(|(l, c)| self.m.lines[l][c + 1..].trim_start().starts_with("=>"))
    }
}
