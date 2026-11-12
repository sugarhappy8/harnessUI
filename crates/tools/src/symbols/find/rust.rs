//! Rust: `fn`, `struct`, `enum`, `trait`, `impl`, `mod`, `const`, `static`, `type`.

use super::{Cur, HEADER_LINES, Scan, ScopeKind, SymbolKind, is_ident, starts_with_word};

pub(super) fn line(s: &mut Scan<'_>, i: usize) {
    let m = s.m;
    let text: &str = &m.lines[i];
    let mut c = Cur::new(text);
    c.ws();
    // Attributes written on the same line.
    while c.rest().starts_with("#[") || c.rest().starts_with("#![") {
        c.pos += c.rest().find('[').unwrap_or(0);
        if !c.skip_balanced(b'[', b']') {
            return;
        }
        c.ws();
    }
    loop {
        match c.word() {
            Some("pub") => {
                c.take_word();
                c.ws();
                if c.peek() == Some(b'(') && !c.skip_balanced(b'(', b')') {
                    return;
                }
            }
            Some("async" | "unsafe" | "default" | "extern") => {
                c.take_word();
            }
            // `const fn`, `const unsafe fn`; otherwise it starts a constant.
            Some("const") => {
                let mut ahead = c;
                ahead.take_word();
                ahead.ws();
                if !matches!(ahead.word(), Some("fn" | "unsafe" | "async" | "extern")) {
                    break;
                }
                c = ahead;
            }
            _ => break,
        }
        c.ws();
    }
    let Some(keyword) = c.take_word() else {
        return;
    };
    c.ws();
    match keyword {
        "fn" => {
            let Some(name) = c.take_word() else { return };
            let mut after = c;
            after.ws();
            if !matches!(after.peek(), Some(b'(' | b'<')) {
                return;
            }
            let ext = s.extent(i, c.pos, false);
            match s.member_of(i) {
                Some(owner) => s.add(name, SymbolKind::Method, Some(&*owner), i, &ext),
                None => s.add(name, SymbolKind::Function, None, i, &ext),
            }
        }
        "struct" | "union" | "enum" | "trait" => {
            let Some(name) = c.take_word() else { return };
            let mut after = c;
            after.ws();
            let follows = matches!(after.peek(), None | Some(b'{' | b'<' | b'(' | b';' | b':'))
                || starts_with_word(after.rest(), "where");
            if !follows {
                return;
            }
            let ext = s.extent(i, c.pos, false);
            let kind = match keyword {
                "enum" => SymbolKind::Enum,
                "trait" => SymbolKind::Trait,
                _ => SymbolKind::Struct,
            };
            s.add(name, kind, None, i, &ext);
            if keyword == "trait" {
                s.push_scope(name, ScopeKind::Members, i, &ext);
            }
        }
        // `mod name;` only points at a file.
        "mod" => {
            let Some(name) = c.take_word() else { return };
            let ext = s.extent(i, c.pos, false);
            if ext.body.is_some() {
                s.add(name, SymbolKind::Module, None, i, &ext);
            }
        }
        "const" | "static" => {
            if c.word() == Some("mut") {
                c.take_word();
                c.ws();
            }
            let Some(name) = c.take_word() else { return };
            let mut after = c;
            after.ws();
            if after.peek() != Some(b':') || name == "_" {
                return;
            }
            let ext = s.extent(i, c.pos, false);
            s.add(name, SymbolKind::Const, None, i, &ext);
        }
        // An associated type inside a trait or impl is part of that block.
        "type" if s.member_of(i).is_none() => {
            let Some(name) = c.take_word() else { return };
            let mut after = c;
            after.ws();
            if !matches!(after.peek(), Some(b'=' | b'<' | b':' | b';')) {
                return;
            }
            let ext = s.extent(i, c.pos, false);
            s.add(name, SymbolKind::Type, None, i, &ext);
        }
        "impl" => impl_block(s, i, c.pos),
        _ => {}
    }
}

/// `impl<T> Trait for Type<T> where ... {`. The type is the name; a trait impl carries the
/// trait as its container. `impl Trait` in a parameter or return type is not a block: it
/// is followed by `,`, `)`, `>` or `;` before any `{`.
fn impl_block(s: &mut Scan<'_>, i: usize, col: usize) {
    let m = s.m;
    let mut header: Vec<u8> = Vec::new();
    let (mut angle, mut round) = (0i32, 0i32);
    let mut seen_where = false;
    let mut open = None;
    let (mut l, mut c) = (i, col);
    'lines: while l < m.lines.len() && l - i <= HEADER_LINES {
        let bytes = m.lines[l].as_bytes();
        while c < bytes.len() {
            let b = bytes[c];
            match b {
                b'<' => angle += 1,
                b'>' if c > 0 && matches!(bytes[c - 1], b'-' | b'=') => {}
                b'>' => {
                    angle -= 1;
                    if angle < 0 {
                        return;
                    }
                }
                b'(' | b'[' => round += 1,
                b')' | b']' => {
                    round -= 1;
                    if round < 0 {
                        return;
                    }
                }
                b',' if angle == 0 && round == 0 && !seen_where => return,
                b';' | b'=' if angle == 0 && round == 0 => return,
                b'{' if angle == 0 && round == 0 => {
                    open = Some((l, c));
                    break 'lines;
                }
                b'w' if (c == 0 || !is_ident(bytes[c - 1]))
                    && starts_with_word(&m.lines[l][c..], "where") =>
                {
                    seen_where = true;
                }
                _ => {}
            }
            header.push(b);
            c += 1;
        }
        header.push(b' ');
        l += 1;
        c = 0;
    }
    let Some((open_line, open_col)) = open else {
        return;
    };
    let header = String::from_utf8_lossy(&header);
    let Some((trait_name, self_name)) = impl_names(&header) else {
        return;
    };
    let ext = s
        .block_from(open_line, open_col)
        .unwrap_or_else(|| s.fallback(i));
    s.add(&self_name, SymbolKind::Impl, trait_name.as_deref(), i, &ext);
    s.push_scope(&self_name, ScopeKind::Members, i, &ext);
}

/// The trait (if any) and the type named in an impl header (the text between `impl` and
/// the block).
fn impl_names(header: &str) -> Option<(Option<String>, String)> {
    let mut c = Cur::new(header);
    c.ws();
    if c.peek() == Some(b'<') && !c.skip_angles() {
        return None;
    }
    let rest = c.rest();
    let rest = match find_top_word(rest, "where") {
        Some(p) => &rest[..p],
        None => rest,
    };
    // `for` splits the trait from the type, except in `for<'a> Fn(&'a T)`.
    let split = find_top_word(rest, "for").filter(|p| !rest[p + 3..].trim_start().starts_with('<'));
    match split {
        Some(p) => {
            let trait_name = type_name(&rest[..p])?;
            let self_name = type_name(&rest[p + 3..])?;
            Some((Some(trait_name), self_name))
        }
        None => Some((None, type_name(rest)?)),
    }
}

/// The position of the whole word `word` outside `<...>`.
fn find_top_word(text: &str, word: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut angle = 0i32;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'<' => angle += 1,
            b'>' if i > 0 && matches!(bytes[i - 1], b'-' | b'=') => {}
            b'>' => angle -= 1,
            _ if angle == 0
                && (i == 0 || !is_ident(bytes[i - 1]))
                && starts_with_word(&text[i..], word) =>
            {
                return Some(i);
            }
            _ => {}
        }
    }
    None
}

/// The last segment of a type path, without its generic arguments: `a::b::Foo<T>` → `Foo`.
fn type_name(text: &str) -> Option<String> {
    let mut t = text.trim().trim_start_matches(['!', '&', '*']).trim_start();
    // `&'a mut dyn Trait`
    loop {
        let before = t;
        if let Some(rest) = t.strip_prefix('\'') {
            t = rest.trim_start_matches(is_ident_char).trim_start();
        }
        for word in ["mut", "dyn", "const"] {
            if starts_with_word(t, word) {
                t = t[word.len()..].trim_start();
            }
        }
        if t == before {
            break;
        }
    }
    let path = t.split('<').next().unwrap_or(t).trim();
    let name = path.rsplit("::").next().unwrap_or(path).trim();
    (!name.is_empty() && name.bytes().all(is_ident) && !name.as_bytes()[0].is_ascii_digit())
        .then(|| name.to_string())
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
