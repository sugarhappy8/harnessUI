//! C and C++: `struct`, `union`, `enum`, `class`, `typedef struct ... name;`, `namespace`,
//! `using Name = ...`, and functions by a conservative shape: `type ... name(...)` followed
//! by a body, only at file or namespace level or directly in a class body, never on a line
//! that starts inside brackets. A name with no type before it is taken only when it is
//! qualified (`Cart::add`), a constructor or destructor in its class, or when the line
//! above ends with the return type (`static int\nname(void)\n{`).

use super::{Cur, Lang, Scan, ScopeKind, SymbolKind, is_ident};

/// Words that are never a function's name or part of its return type.
const NOT_NAMES: [&str; 24] = [
    "if",
    "while",
    "for",
    "switch",
    "return",
    "sizeof",
    "catch",
    "defined",
    "else",
    "do",
    "case",
    "goto",
    "new",
    "delete",
    "throw",
    "typedef",
    "using",
    "namespace",
    "template",
    "static_assert",
    "decltype",
    "alignof",
    "co_return",
    "co_await",
];

pub(super) fn line(s: &mut Scan<'_>, i: usize) {
    let text: &str = &s.m.lines[i];
    let mut c = Cur::new(text);
    c.ws();
    match c.word() {
        // `extern "C" {` (the string is blanked).
        Some("extern") => {
            let mut ahead = c;
            ahead.take_word();
            ahead.ws();
            if ahead.peek() == Some(b'{') {
                let ext = s.extent(i, ahead.pos, false);
                s.push_scope("", ScopeKind::Space, i, &ext);
                return;
            }
        }
        Some("namespace") => {
            c.take_word();
            c.ws();
            let name = c.take_path("::").unwrap_or("");
            let mut after = c;
            after.ws();
            if after.peek() != Some(b'{') {
                return;
            }
            let ext = s.extent(i, c.pos, false);
            if !name.is_empty() {
                s.add(name, SymbolKind::Module, None, i, &ext);
            }
            s.push_scope(name, ScopeKind::Space, i, &ext);
            return;
        }
        Some("template") => {
            c.take_word();
            c.ws();
            if !c.skip_angles() {
                return;
            }
            c.ws();
            if c.peek().is_none() {
                return;
            }
        }
        Some("typedef") => {
            c.take_word();
            c.ws();
            typedef(s, i, c);
            return;
        }
        Some("using") => {
            c.take_word();
            c.ws();
            let Some(name) = c.take_word() else { return };
            let mut after = c;
            after.ws();
            if after.peek() == Some(b'=') && !after.rest().starts_with("==") {
                let ext = s.extent(i, c.pos, false);
                s.add(name, SymbolKind::Type, None, i, &ext);
            }
            return;
        }
        _ => {}
    }
    if matches!(c.word(), Some("struct" | "class" | "union" | "enum")) && record(s, i, c) {
        return;
    }
    if s.m.round[i] == 0 && s.at_scope(i) {
        function(s, i, c);
    }
}

/// A `struct`, `class`, `union` or `enum` with a body. False when the line is something
/// else that starts with the keyword (`struct point *make(...)`, `struct node *next;`).
fn record(s: &mut Scan<'_>, i: usize, mut c: Cur<'_>) -> bool {
    let keyword = c.take_word().unwrap_or("");
    c.ws();
    if keyword == "enum" && matches!(c.word(), Some("class" | "struct")) {
        c.take_word();
        c.ws();
    }
    let Some(name) = c.take_word() else {
        return false;
    };
    let name_end = c.pos;
    c.ws();
    if c.peek() == Some(b'<') {
        if !c.skip_angles() {
            return false;
        }
        c.ws();
    }
    if c.word() == Some("final") {
        c.take_word();
        c.ws();
    }
    let opens = match c.peek() {
        Some(b'{') => true,
        Some(b':') => !c.rest().starts_with("::"),
        // The body on the next line.
        None => s.m.lines.get(i + 1).is_some_and(|l| l.trim_start().starts_with('{')),
        _ => false,
    };
    if !opens {
        return false;
    }
    let ext = s.extent(i, name_end, false);
    if ext.body.is_none() {
        return true;
    }
    let kind = match keyword {
        "class" => SymbolKind::Class,
        "enum" => SymbolKind::Enum,
        _ => SymbolKind::Struct,
    };
    s.add(name, kind, None, i, &ext);
    if s.lang == Lang::Cpp && kind != SymbolKind::Enum {
        s.push_scope(name, ScopeKind::Members, i, &ext);
    }
    true
}

/// `typedef struct [tag] { ... } name;`: the name after the block, or the tag.
fn typedef(s: &mut Scan<'_>, i: usize, mut c: Cur<'_>) {
    let kind = match c.take_word() {
        Some("struct" | "union") => SymbolKind::Struct,
        Some("enum") => SymbolKind::Enum,
        _ => return,
    };
    c.ws();
    let tag = c.take_word();
    let ext = s.extent(i, c.pos, false);
    let Some(body) = ext.body else { return };
    let after = &s.m.lines[body.close][body.close_col + 1..];
    let alias = after.split(';').next().unwrap_or("").trim().trim_start_matches('*');
    let alias = (!alias.is_empty() && alias.bytes().all(is_ident)).then_some(alias);
    if let Some(name) = alias.or(tag) {
        s.add(name, kind, None, i, &ext);
    }
}

/// A function definition by its shape.
fn function(s: &mut Scan<'_>, i: usize, mut c: Cur<'_>) {
    let mut types = 0usize;
    let (path, name_end) = loop {
        c.ws();
        match c.peek() {
            Some(b'*' | b'&') => {
                c.pos += 1;
                continue;
            }
            Some(b) if b == b'~' || b == b':' || super::is_ident_start(b) => {}
            _ => return,
        }
        let start = c.pos;
        if !name_path(&mut c) {
            return;
        }
        let path = &c.s[start..c.pos];
        let end = c.pos;
        c.ws();
        if c.peek() == Some(b'(') {
            break (path, end);
        }
        if NOT_NAMES.contains(&path) {
            return;
        }
        types += 1;
    };
    let segments: Vec<&str> = split_path(path);
    let Some(&name) = segments.last() else { return };
    if NOT_NAMES.contains(&name) {
        return;
    }
    let qualifier = (segments.len() > 1).then(|| segments[segments.len() - 2]);
    let owner = s.member_of(i);
    let mut first = i;
    if types == 0 {
        let special = owner
            .as_deref()
            .is_some_and(|o| name == o || name.strip_prefix('~') == Some(o));
        if qualifier.is_none() && !special {
            match type_above(s, i) {
                Some(above) => first = above,
                None => return,
            }
        }
    }
    let ext = s.extent(i, name_end, true);
    if ext.body.is_none() {
        return;
    }
    match (owner, qualifier) {
        (Some(owner), _) => s.add_from(name, SymbolKind::Method, Some(&*owner), first, i, &ext),
        (None, Some(q)) => s.add_from(name, SymbolKind::Method, Some(q), first, i, &ext),
        (None, None) => s.add_from(name, SymbolKind::Function, None, first, i, &ext),
    }
}

/// Reads `a::b<T>::~c` or `operator==`. False when nothing name-like is here.
fn name_path(c: &mut Cur<'_>) -> bool {
    c.eat("::");
    let mut any = false;
    loop {
        c.eat("~");
        let Some(word) = c.take_word() else {
            return any;
        };
        any = true;
        if word == "operator" {
            c.ws();
            if c.eat("()") {
                return true;
            }
            let start = c.pos;
            while c.peek().is_some_and(|b| b"+-*/%^&|~!=<>,[]".contains(&b)) {
                c.pos += 1;
            }
            return c.pos > start || c.word().is_some();
        }
        if c.peek() == Some(b'<') && !c.skip_angles() {
            return false;
        }
        if !c.eat("::") {
            return true;
        }
    }
}

/// The segments of a name path without template arguments: `a::b<T>::c` → `[a, b, c]`.
fn split_path(path: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    let bytes = path.as_bytes();
    let mut k = 0usize;
    let mut seg_end = None;
    while k < bytes.len() {
        match bytes[k] {
            b'<' if !path[start..k].ends_with("operator") && depth >= 0 => {
                if depth == 0 {
                    seg_end.get_or_insert(k);
                }
                depth += 1;
            }
            b'>' if depth > 0 => depth -= 1,
            b':' if depth == 0 && bytes.get(k + 1) == Some(&b':') => {
                let end = seg_end.take().unwrap_or(k);
                if end > start {
                    out.push(&path[start..end]);
                }
                k += 2;
                start = k;
                continue;
            }
            _ => {}
        }
        k += 1;
    }
    let end = seg_end.unwrap_or(bytes.len());
    if end > start {
        out.push(&path[start..end]);
    }
    out
}

/// The line above `i` when it holds only the return type of a function whose name starts
/// line `i` (`static inline unsigned long` above `hash(const char *s)`).
fn type_above(s: &Scan<'_>, i: usize) -> Option<usize> {
    let m = s.m;
    let above = i.checked_sub(1)?;
    if m.comment_only[above]
        || m.in_literal[above]
        || m.round[above] != 0
        || m.curly[above] != m.curly[i]
    {
        return None;
    }
    let text = m.lines[above].trim();
    let last = *text.as_bytes().last()?;
    if !(is_ident(last) || last == b'*' || last == b'&') {
        return None;
    }
    let mut c = Cur::new(text);
    let first = c.take_word()?;
    let (_, word) = super::trailing_word(text);
    if NOT_NAMES.contains(&first) || NOT_NAMES.contains(&word) {
        return None;
    }
    Some(above)
}
