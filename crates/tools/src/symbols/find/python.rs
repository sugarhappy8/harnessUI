//! Python: `def`, `async def` and `class`. A block ends with the last line indented deeper
//! than its header; lines inside a multi-line string or inside brackets continue the block
//! whatever their indentation.

use super::{Cur, Extent, HEADER_LINES, Scan, Scope, ScopeKind, SymbolKind};

pub(super) fn line(s: &mut Scan<'_>, i: usize) {
    let m = s.m;
    if m.in_literal[i] || m.round[i] > 0 {
        return;
    }
    let text: &str = &m.lines[i];
    let mut c = Cur::new(text);
    c.ws();
    let indent = indent_of(s.original(i));
    if c.word() == Some("async") {
        c.take_word();
        c.ws();
    }
    let keyword = match c.take_word() {
        Some(k @ ("def" | "class")) => k,
        _ => return,
    };
    c.ws();
    let Some(name) = c.take_word() else { return };
    let mut after = c;
    after.ws();
    let ok = match keyword {
        "def" => matches!(after.peek(), Some(b'(' | b'[')),
        _ => matches!(after.peek(), Some(b'(' | b'[' | b':')),
    };
    if !ok {
        return;
    }
    let Some(header_end) = header_end(s, i, c.pos) else {
        return;
    };
    let end = block_end(s, header_end, indent);
    let ext = Extent { end, body: None };
    let owner = s.scopes.last().map(|sc| (sc.kind, sc.name.clone()));
    if keyword == "class" {
        s.add(name, SymbolKind::Class, None, i, &ext);
    } else {
        match owner {
            Some((ScopeKind::Members, owner)) => {
                s.add(name, SymbolKind::Method, Some(&*owner), i, &ext)
            }
            _ => s.add(name, SymbolKind::Function, None, i, &ext),
        }
    }
    if end > i {
        s.scopes.push(Scope {
            name: name.into(),
            kind: if keyword == "class" {
                ScopeKind::Members
            } else {
                ScopeKind::Function
            },
            depth: indent,
            end,
        });
    }
}

/// Indentation width, a tab reaching the next multiple of eight.
fn indent_of(line: &str) -> u32 {
    let mut width = 0u32;
    for b in line.bytes() {
        match b {
            b' ' => width += 1,
            b'\t' => width = (width / 8 + 1) * 8,
            _ => break,
        }
    }
    width
}

/// The line holding the `:` that ends the header starting at (`line`, `col`).
fn header_end(s: &Scan<'_>, line: usize, col: usize) -> Option<usize> {
    let lines = &s.m.lines;
    let mut depth = 0i32;
    let (mut l, mut c) = (line, col);
    while l < lines.len() && l - line <= HEADER_LINES {
        let bytes = lines[l].as_bytes();
        while c < bytes.len() {
            match bytes[c] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b':' if depth == 0 => return Some(l),
                _ => {}
            }
            c += 1;
        }
        l += 1;
        c = 0;
    }
    None
}

/// The last line of the block whose header ends on `header_end` and is indented `indent`.
fn block_end(s: &Scan<'_>, header_end: usize, indent: u32) -> usize {
    let m = s.m;
    let mut end = header_end;
    for l in header_end + 1..m.lines.len() {
        let orig = s.original(l);
        if orig.trim().is_empty() {
            continue;
        }
        if m.in_literal[l] || m.round[l] > 0 {
            end = l;
            continue;
        }
        if m.comment_only[l] {
            continue;
        }
        if indent_of(orig) <= indent {
            break;
        }
        end = l;
    }
    end
}
