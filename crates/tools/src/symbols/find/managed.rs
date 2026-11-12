//! Java, C#, Kotlin and Swift.
//!
//! Types (`class`, `interface`, `enum`, `record`, `struct`, `object`, `protocol`, `actor`,
//! `extension`) are keyword-led and found at any depth. Kotlin `fun` and Swift `func` and
//! `init` are keyword-led too. Java and C# methods have no keyword, so they are guessed from
//! their shape (`Type name(`, or a constructor `Owner(`) only directly inside a type body,
//! and only when a body follows (C# also takes `=> expression;`).

use super::{Cur, Lang, Scan, ScopeKind, SymbolKind};

const JAVA_MODIFIERS: [&str; 13] = [
    "public",
    "private",
    "protected",
    "static",
    "final",
    "abstract",
    "synchronized",
    "native",
    "default",
    "strictfp",
    "sealed",
    "non",
    "transient",
];

const CSHARP_MODIFIERS: [&str; 17] = [
    "public", "private", "protected", "internal", "static", "readonly", "sealed", "abstract",
    "virtual", "override", "async", "extern", "unsafe", "new", "partial", "file", "required",
];

const KOTLIN_MODIFIERS: [&str; 25] = [
    "public",
    "private",
    "protected",
    "internal",
    "open",
    "override",
    "abstract",
    "final",
    "sealed",
    "data",
    "inner",
    "inline",
    "suspend",
    "operator",
    "infix",
    "tailrec",
    "external",
    "actual",
    "expect",
    "enum",
    "annotation",
    "companion",
    "value",
    "const",
    "noinline",
];

const SWIFT_MODIFIERS: [&str; 21] = [
    "public",
    "private",
    "fileprivate",
    "internal",
    "open",
    "final",
    "static",
    "override",
    "mutating",
    "nonmutating",
    "convenience",
    "required",
    "lazy",
    "weak",
    "unowned",
    "dynamic",
    "optional",
    "indirect",
    "nonisolated",
    "package",
    "distributed",
];

/// Words that are never a method's type or name (statements and expressions).
const NOT_NAMES: [&str; 26] = [
    "return", "new", "throw", "if", "else", "while", "for", "foreach", "switch", "case", "using",
    "lock", "yield", "await", "var", "goto", "do", "try", "catch", "finally", "sizeof", "typeof",
    "assert", "break", "continue", "fixed",
];

pub(super) fn line(s: &mut Scan<'_>, i: usize) {
    match s.lang {
        Lang::Java | Lang::CSharp => java_like(s, i),
        Lang::Kotlin => kotlin(s, i),
        Lang::Swift => swift(s, i),
        _ => {}
    }
}

/// Skips `@Name`, `@Name(...)`, `@a.b:Name` written on the line. False when an annotation
/// does not close on the line.
fn skip_annotations(c: &mut Cur<'_>) -> bool {
    while c.peek() == Some(b'@') && !c.rest().starts_with("@interface") {
        c.pos += 1;
        if c.take_path(".").is_none() {
            return false;
        }
        if c.eat(":") && c.take_path(".").is_none() {
            return false;
        }
        if c.peek() == Some(b'(') && !c.skip_balanced(b'(', b')') {
            return false;
        }
        c.ws();
    }
    true
}

/// Skips C# attributes `[Obsolete]` written before a declaration on the same line.
fn skip_attributes(c: &mut Cur<'_>) -> bool {
    while c.peek() == Some(b'[') {
        if !c.skip_balanced(b'[', b']') {
            return false;
        }
        c.ws();
    }
    true
}

/// Adds a type and notes its body, so that functions in it are its methods.
fn type_decl(s: &mut Scan<'_>, i: usize, name: &str, kind: SymbolKind, name_end: usize) {
    let ext = s.extent(i, name_end, false);
    s.add(name, kind, None, i, &ext);
    s.push_scope(name, ScopeKind::Members, i, &ext);
}

fn java_like(s: &mut Scan<'_>, i: usize) {
    let csharp = s.lang == Lang::CSharp;
    let text: &str = &s.m.lines[i];
    let mut c = Cur::new(text);
    c.ws();
    let ok = if csharp {
        skip_attributes(&mut c)
    } else {
        skip_annotations(&mut c)
    };
    if !ok {
        return;
    }
    let modifiers: &[&str] = if csharp {
        &CSHARP_MODIFIERS
    } else {
        &JAVA_MODIFIERS
    };
    loop {
        match c.word() {
            Some(w) if modifiers.contains(&w) => {
                c.take_word();
                // `non-sealed`
                c.eat("-");
            }
            _ => break,
        }
        c.ws();
        if !csharp && !skip_annotations(&mut c) {
            return;
        }
    }
    if !csharp && c.eat("@interface") {
        c.ws();
        if let Some(name) = c.take_word() {
            type_decl(s, i, name, SymbolKind::Interface, c.pos);
        }
        return;
    }
    let mut ahead = c;
    let keyword = ahead.take_word();
    ahead.ws();
    let kind = match keyword {
        Some("class") => Some(SymbolKind::Class),
        Some("interface") => Some(SymbolKind::Interface),
        Some("enum") => Some(SymbolKind::Enum),
        Some("record") => {
            // C# `record class Order`, `record struct Point`.
            if matches!(ahead.word(), Some("class" | "struct")) && csharp {
                ahead.take_word();
                ahead.ws();
            }
            Some(SymbolKind::Class)
        }
        Some("struct") if csharp => Some(SymbolKind::Struct),
        Some("namespace") if csharp => {
            if let Some(name) = ahead.take_path(".") {
                let ext = s.extent(i, ahead.pos, false);
                s.add(name, SymbolKind::Module, None, i, &ext);
                s.push_scope(name, ScopeKind::Space, i, &ext);
            }
            return;
        }
        _ => None,
    };
    if let Some(kind) = kind {
        if let Some(name) = ahead.take_word() {
            let mut after = ahead;
            after.ws();
            // `class Foo` in an expression (`Foo.class`) never starts a line; a name must be
            // followed by what a declaration has.
            if matches!(after.peek(), None | Some(b'{' | b'<' | b'(' | b':' | b';'))
                || after.word().is_some()
            {
                type_decl(s, i, name, kind, ahead.pos);
            }
        }
        return;
    }
    if let Some(owner) = s.member_of(i) {
        method_guess(s, i, c, &owner);
    }
}

/// `Type name(...) {`, `<T> Type name(...)`, `Owner(...) {` directly inside a type body.
fn method_guess(s: &mut Scan<'_>, i: usize, mut c: Cur<'_>, owner: &str) {
    let csharp = s.lang == Lang::CSharp;
    if c.peek() == Some(b'<') {
        if !c.skip_angles() {
            return;
        }
        c.ws();
    }
    let Some(first) = c.take_path(".") else {
        return;
    };
    if NOT_NAMES.contains(&first) {
        return;
    }
    let mut after = c;
    after.ws();
    let (name, name_end) = if after.peek() == Some(b'(') {
        // A constructor.
        if first != owner {
            return;
        }
        (first, c.pos)
    } else {
        if c.peek() == Some(b'<') && !c.skip_angles() {
            return;
        }
        c.eat("?");
        while c.eat("[]") {}
        c.eat("?");
        c.ws();
        let Some(name) = c.take_word() else { return };
        if NOT_NAMES.contains(&name) {
            return;
        }
        let end = c.pos;
        c.ws();
        if csharp && c.peek() == Some(b'<') && !c.skip_angles() {
            return;
        }
        c.ws();
        if c.peek() != Some(b'(') {
            return;
        }
        (name, end)
    };
    let mut paren = Cur::new(c.s);
    paren.pos = name_end;
    paren.ws();
    if paren.peek() == Some(b'<') {
        paren.skip_angles();
        paren.ws();
    }
    let ext = s.extent(i, name_end, false);
    if ext.body.is_some() || (csharp && s.arrow_after_params(i, paren.pos)) {
        s.add(name, SymbolKind::Method, Some(owner), i, &ext);
    }
}

fn kotlin(s: &mut Scan<'_>, i: usize) {
    let text: &str = &s.m.lines[i];
    let mut c = Cur::new(text);
    c.ws();
    if !skip_annotations(&mut c) {
        return;
    }
    let (mut is_enum, mut companion) = (false, false);
    loop {
        match c.word() {
            Some(w) if KOTLIN_MODIFIERS.contains(&w) => {
                is_enum |= w == "enum";
                companion |= w == "companion";
                c.take_word();
            }
            _ => break,
        }
        c.ws();
        if !skip_annotations(&mut c) {
            return;
        }
    }
    let Some(keyword) = c.take_word() else { return };
    c.ws();
    match keyword {
        "fun" if c.word() == Some("interface") => {
            c.take_word();
            c.ws();
            if let Some(name) = c.take_word() {
                type_decl(s, i, name, SymbolKind::Interface, c.pos);
            }
        }
        "fun" => {
            if c.peek() == Some(b'<') {
                if !c.skip_angles() {
                    return;
                }
                c.ws();
            }
            // The receiver type comes before the name: `fun <T> List<T>.second()`.
            let (name, name_end) = loop {
                let name = if c.peek() == Some(b'`') {
                    let start = c.pos + 1;
                    let Some(len) = c.s[start..].find('`') else {
                        return;
                    };
                    c.pos = start + len + 1;
                    &c.s[start..start + len]
                } else {
                    let Some(word) = c.take_word() else { return };
                    word
                };
                let end = c.pos;
                if c.peek() == Some(b'<') && !c.skip_angles() {
                    return;
                }
                c.eat("?");
                if !c.eat(".") {
                    break (name, end);
                }
            };
            c.ws();
            if c.peek() != Some(b'(') {
                return;
            }
            let ext = s.extent(i, name_end, false);
            match s.member_of(i) {
                Some(owner) => s.add(name, SymbolKind::Method, Some(&*owner), i, &ext),
                None => s.add(name, SymbolKind::Function, None, i, &ext),
            }
        }
        "class" | "interface" | "object" => {
            let Some(name) = c.take_word() else {
                // `companion object {`: its functions belong to the enclosing class.
                if companion
                    && keyword == "object"
                    && let Some(owner) = s.member_of(i)
                {
                    let ext = s.extent(i, c.pos, false);
                    s.push_scope(&owner, ScopeKind::Members, i, &ext);
                }
                return;
            };
            if companion {
                let owner = s.member_of(i).unwrap_or_else(|| name.into());
                let ext = s.extent(i, c.pos, false);
                s.push_scope(&owner, ScopeKind::Members, i, &ext);
                return;
            }
            let kind = match keyword {
                "interface" => SymbolKind::Interface,
                _ if is_enum => SymbolKind::Enum,
                _ => SymbolKind::Class,
            };
            type_decl(s, i, name, kind, c.pos);
        }
        "typealias" => {
            if let Some(name) = c.take_word() {
                let ext = s.extent(i, c.pos, false);
                s.add(name, SymbolKind::Type, None, i, &ext);
            }
        }
        _ => {}
    }
}

fn swift(s: &mut Scan<'_>, i: usize) {
    let text: &str = &s.m.lines[i];
    let mut c = Cur::new(text);
    c.ws();
    if !skip_annotations(&mut c) {
        return;
    }
    loop {
        let Some(w) = c.word() else { break };
        let modifier = SWIFT_MODIFIERS.contains(&w) || {
            // `class func`, `class var`: a modifier, not a class.
            let mut ahead = c;
            ahead.take_word();
            ahead.ws();
            w == "class"
                && matches!(
                    ahead.word(),
                    Some("func" | "var" | "let" | "subscript" | "override" | "final" | "static")
                )
        };
        if !modifier {
            break;
        }
        c.take_word();
        // `private(set)`
        if c.peek() == Some(b'(') && !c.skip_balanced(b'(', b')') {
            return;
        }
        c.ws();
        if !skip_annotations(&mut c) {
            return;
        }
    }
    let Some(keyword) = c.take_word() else { return };
    let keyword_end = c.pos;
    c.ws();
    match keyword {
        "func" => {
            let Some(name) = c.take_word() else { return };
            let name_end = c.pos;
            c.ws();
            if c.peek() == Some(b'<') {
                if !c.skip_angles() {
                    return;
                }
                c.ws();
            }
            if c.peek() != Some(b'(') {
                return;
            }
            let ext = s.extent(i, name_end, false);
            match s.member_of(i) {
                Some(owner) => s.add(name, SymbolKind::Method, Some(&*owner), i, &ext),
                None => s.add(name, SymbolKind::Function, None, i, &ext),
            }
        }
        "init" => {
            c.eat("?");
            c.eat("!");
            if c.peek() == Some(b'<') && !c.skip_angles() {
                return;
            }
            c.ws();
            if c.peek() != Some(b'(') {
                return;
            }
            if let Some(owner) = s.member_of(i) {
                let ext = s.extent(i, keyword_end, false);
                s.add("init", SymbolKind::Method, Some(&*owner), i, &ext);
            }
        }
        "class" | "struct" | "enum" | "protocol" | "actor" | "extension" => {
            let Some(path) = c.take_path(".") else { return };
            let name = path.rsplit('.').next().unwrap_or(path);
            let kind = match keyword {
                "struct" => SymbolKind::Struct,
                "enum" => SymbolKind::Enum,
                "protocol" => SymbolKind::Interface,
                "extension" => SymbolKind::Impl,
                _ => SymbolKind::Class,
            };
            type_decl(s, i, name, kind, c.pos);
        }
        "typealias" => {
            if let Some(name) = c.take_word() {
                let ext = s.extent(i, c.pos, false);
                s.add(name, SymbolKind::Type, None, i, &ext);
            }
        }
        _ => {}
    }
}
