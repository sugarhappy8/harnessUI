//! Go: `func` (with a receiver, a method of that type) and `type`, also inside `type ( )`.

use super::{Cur, Scan, Scope, ScopeKind, SymbolKind};

/// The name of the scope noted for a `type ( ... )` group.
const GROUP: &str = "type (";

pub(super) fn line(s: &mut Scan<'_>, i: usize) {
    let m = s.m;
    let text: &str = &m.lines[i];
    let mut c = Cur::new(text);
    c.ws();
    let in_group = s.scopes.last().is_some_and(|sc| {
        &*sc.name == GROUP && sc.depth == m.curly[i] && m.round[i] == 1
    });
    if in_group {
        type_spec(s, i, c);
        return;
    }
    match c.take_word() {
        Some("func") => {
            c.ws();
            func(s, i, c);
        }
        Some("type") => {
            c.ws();
            if c.peek() == Some(b'(') {
                if let Some((close, _)) = s.close_paren(i, c.pos)
                    && close > i
                {
                    s.scopes.push(Scope {
                        name: GROUP.into(),
                        kind: ScopeKind::Space,
                        depth: m.curly[i],
                        end: close,
                    });
                }
                return;
            }
            type_spec(s, i, c);
        }
        _ => {}
    }
}

fn func(s: &mut Scan<'_>, i: usize, mut c: Cur<'_>) {
    let mut receiver = None;
    if c.peek() == Some(b'(') {
        let start = c.pos + 1;
        if !c.skip_balanced(b'(', b')') {
            return;
        }
        receiver = receiver_type(&c.s[start..c.pos - 1]);
        if receiver.is_none() {
            return;
        }
        c.ws();
    }
    let Some(name) = c.take_word() else { return };
    let mut after = c;
    after.ws();
    if !matches!(after.peek(), Some(b'(' | b'[')) {
        return;
    }
    let ext = s.extent(i, c.pos, false);
    match receiver {
        Some(owner) => s.add(name, SymbolKind::Method, Some(owner), i, &ext),
        None => s.add(name, SymbolKind::Function, None, i, &ext),
    }
}

/// `p *Point` → `Point`, `s Stack[T]` → `Stack`, `Point` → `Point`.
fn receiver_type(text: &str) -> Option<&str> {
    let ty = text.split_whitespace().last()?;
    let ty = ty.trim_start_matches('*');
    let ty = ty.split('[').next().unwrap_or(ty);
    let mut c = Cur::new(ty);
    let word = c.take_word()?;
    (c.pos == ty.len()).then_some(word)
}

/// `Name struct {`, `Name interface {`, `Name[T any] ...`, `Name = Other`, `Name int`.
fn type_spec(s: &mut Scan<'_>, i: usize, mut c: Cur<'_>) {
    let Some(name) = c.take_word() else { return };
    let name_end = c.pos;
    c.ws();
    if c.peek() == Some(b'[') {
        if !c.skip_balanced(b'[', b']') {
            return;
        }
        c.ws();
    }
    c.eat("=");
    c.ws();
    if c.peek().is_none() {
        return;
    }
    let kind = match c.word() {
        Some("struct") => SymbolKind::Struct,
        Some("interface") => SymbolKind::Interface,
        _ => SymbolKind::Type,
    };
    let ext = s.extent(i, name_end, false);
    s.add(name, kind, None, i, &ext);
}
