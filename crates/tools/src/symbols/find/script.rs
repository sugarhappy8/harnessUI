//! TypeScript and JavaScript: `function`, `class`, `interface`, `type`, `enum`,
//! `namespace`, exported or top-level `const` functions, and methods in class bodies.

use super::{Cur, Scan, ScopeKind, SymbolKind};

/// Callees whose argument is a component or function even when it is an arrow.
const WRAPPERS: [&str; 6] = [
    "memo",
    "forwardRef",
    "observer",
    "lazy",
    "debounce",
    "throttle",
];

/// Words that can stand before a member name in a class body.
const MEMBER_MODIFIERS: [&str; 12] = [
    "public",
    "private",
    "protected",
    "static",
    "async",
    "readonly",
    "override",
    "abstract",
    "get",
    "set",
    "declare",
    "accessor",
];

/// Words that are never the name of a method.
const NOT_NAMES: [&str; 12] = [
    "if", "for", "while", "switch", "catch", "return", "new", "function", "do", "else", "with",
    "typeof",
];

pub(super) fn line(s: &mut Scan<'_>, i: usize) {
    let m = s.m;
    let text: &str = &m.lines[i];
    let mut c = Cur::new(text);
    c.ws();
    if s.member_of(i).is_some() && member(s, i, c) {
        return;
    }
    let mut exported = false;
    loop {
        match c.word() {
            Some("export") => exported = true,
            Some("default" | "declare" | "abstract") => {}
            _ => break,
        }
        c.take_word();
        c.ws();
    }
    let top = m.curly[i] == 0 && m.round[i] == 0;
    let Some(keyword) = c.take_word() else { return };
    c.ws();
    match keyword {
        "async" | "function" => {
            if keyword == "async" {
                if c.take_word() != Some("function") {
                    return;
                }
                c.ws();
            }
            if c.eat("*") {
                c.ws();
            }
            let Some(name) = c.take_word() else { return };
            let ext = s.extent(i, c.pos, false);
            // A signature without a body is an overload or a declaration.
            if ext.body.is_some() {
                s.add(name, SymbolKind::Function, None, i, &ext);
            }
        }
        "class" => {
            let Some(name) = c.take_word() else { return };
            let ext = s.extent(i, c.pos, false);
            s.add(name, SymbolKind::Class, None, i, &ext);
            s.push_scope(name, ScopeKind::Members, i, &ext);
        }
        "interface" => {
            let Some(name) = c.take_word() else { return };
            let mut after = c;
            after.ws();
            if !matches!(after.peek(), Some(b'{' | b'<') | None)
                && !after.rest().starts_with("extends")
            {
                return;
            }
            let ext = s.extent(i, c.pos, false);
            s.add(name, SymbolKind::Interface, None, i, &ext);
        }
        "type" => {
            let Some(name) = c.take_word() else { return };
            let mut after = c;
            after.ws();
            if !matches!(after.peek(), Some(b'=' | b'<')) || after.rest().starts_with("==") {
                return;
            }
            let ext = s.extent(i, c.pos, false);
            s.add(name, SymbolKind::Type, None, i, &ext);
        }
        "enum" => enum_decl(s, i, c),
        "namespace" | "module" => {
            let Some(name) = c.take_path(".") else { return };
            let mut after = c;
            after.ws();
            if after.peek() != Some(b'{') {
                return;
            }
            let ext = s.extent(i, c.pos, false);
            s.add(name, SymbolKind::Module, None, i, &ext);
        }
        "const" | "let" | "var" => {
            if keyword == "const" && c.word() == Some("enum") {
                c.take_word();
                c.ws();
                enum_decl(s, i, c);
                return;
            }
            if exported || top {
                const_function(s, i, c);
            }
        }
        _ => {}
    }
}

fn enum_decl(s: &mut Scan<'_>, i: usize, mut c: Cur<'_>) {
    let Some(name) = c.take_word() else { return };
    let mut after = c;
    after.ws();
    if !matches!(after.peek(), Some(b'{') | None) {
        return;
    }
    let ext = s.extent(i, c.pos, false);
    s.add(name, SymbolKind::Enum, None, i, &ext);
}

/// `const name = (...) => ...`, `const name = function ...`, `const name = memo(...)`.
fn const_function(s: &mut Scan<'_>, i: usize, mut c: Cur<'_>) {
    let Some(name) = c.take_word() else { return };
    let name_end = c.pos;
    c.ws();
    // A type annotation: `const Foo: React.FC<Props> = ...`.
    if c.peek() == Some(b':') {
        let rest = c.rest().as_bytes();
        let Some(off) = (0..rest.len()).find(|&k| {
            rest[k] == b'='
                && !matches!(rest.get(k + 1), Some(b'>' | b'='))
                && (k == 0 || !matches!(rest[k - 1], b'=' | b'!' | b'<' | b'>'))
        }) else {
            return;
        };
        c.pos += off;
    }
    if !c.eat("=") || matches!(c.peek(), Some(b'=' | b'>')) {
        return;
    }
    c.ws();
    if !is_function_value(s, i, c) {
        return;
    }
    let ext = s.extent(i, name_end, false);
    s.add(name, SymbolKind::Function, None, i, &ext);
}

/// Whether the value starting here is a function: an arrow, a `function` expression, or a
/// call of a known wrapper (or any call whose argument is a `function` expression).
fn is_function_value(s: &Scan<'_>, i: usize, mut c: Cur<'_>) -> bool {
    if c.word() == Some("async") {
        let mut ahead = c;
        ahead.take_word();
        ahead.ws();
        if matches!(ahead.peek(), Some(b'(' | b'<')) || ahead.word().is_some() {
            c = ahead;
        }
    }
    if c.word() == Some("function") {
        return true;
    }
    if c.peek() == Some(b'<') {
        if !c.skip_angles() {
            return false;
        }
        c.ws();
    }
    if c.peek() == Some(b'(') {
        return arrow_after(s, i, c.pos);
    }
    let Some(callee) = c.take_path(".") else {
        return false;
    };
    let mut after = c;
    after.ws();
    if after.rest().starts_with("=>") {
        // `x => ...`, a single parameter.
        return !callee.contains('.');
    }
    if after.peek() != Some(b'(') {
        return false;
    }
    let last = callee.rsplit('.').next().unwrap_or(callee);
    if WRAPPERS.contains(&last) {
        return true;
    }
    let mut arg = after;
    arg.pos += 1;
    arg.ws();
    arg.word() == Some("function")
}

/// Whether `=>` follows the parameters opening at (`line`, `col`), allowing a return type
/// annotation in between: `(n: number): Promise<number> => {`.
fn arrow_after(s: &Scan<'_>, line: usize, col: usize) -> bool {
    let Some((l, c)) = s.close_paren(line, col) else {
        return false;
    };
    let rest = s.m.lines[l][c + 1..].trim_start();
    rest.starts_with("=>") || (rest.starts_with(':') && rest.contains("=>"))
}

/// A method, getter, or arrow-function property directly in a class body. True when the
/// line was a member (found or not), so nothing else is tried.
fn member(s: &mut Scan<'_>, i: usize, mut c: Cur<'_>) -> bool {
    let Some(owner) = s.member_of(i) else {
        return false;
    };
    // Decorators on the same line.
    while c.peek() == Some(b'@') {
        c.pos += 1;
        if c.take_path(".").is_none() {
            return false;
        }
        if c.peek() == Some(b'(') && !c.skip_balanced(b'(', b')') {
            return false;
        }
        c.ws();
    }
    loop {
        match c.word() {
            Some(w) if MEMBER_MODIFIERS.contains(&w) => {
                let mut ahead = c;
                ahead.take_word();
                ahead.ws();
                // `get() {}` is a method called `get`.
                if ahead.word().is_none() && !matches!(ahead.peek(), Some(b'*' | b'#' | b'[')) {
                    break;
                }
                c = ahead;
            }
            _ => break,
        }
    }
    if c.eat("*") {
        c.ws();
    }
    c.eat("#");
    let Some(name) = c.take_word() else {
        return false;
    };
    if NOT_NAMES.contains(&name) {
        return true;
    }
    let name_end = c.pos;
    c.ws();
    if matches!(c.peek(), Some(b'?' | b'!')) {
        c.pos += 1;
        c.ws();
    }
    if c.peek() == Some(b'<') {
        if !c.skip_angles() {
            return true;
        }
        c.ws();
    }
    match c.peek() {
        Some(b'(') => {
            let ext = s.extent(i, name_end, false);
            if ext.body.is_some() {
                s.add(name, SymbolKind::Method, Some(&*owner), i, &ext);
            }
        }
        Some(b'=' | b':') => {
            // `onClick = (e) => {`, `handler: Fn = async () => {`
            let rest = c.rest();
            let Some(eq) = rest.find('=') else {
                return true;
            };
            if rest[eq + 1..].starts_with(['=', '>']) {
                return true;
            }
            let mut v = c;
            v.pos += eq + 1;
            v.ws();
            if is_function_value(s, i, v) {
                let ext = s.extent(i, name_end, false);
                s.add(name, SymbolKind::Method, Some(&*owner), i, &ext);
            }
        }
        _ => {}
    }
    true
}
