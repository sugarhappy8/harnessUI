//! Reading style sheets for `browser_capture_site` (ARCHITECTURE.md 9.3): the addresses a
//! sheet names (`url(…)`, `@import`, `image-set(…)`), rewritten to the local copies, and
//! the facts the report and `animations.json` need (`@keyframes`, hover and focus rules,
//! transitions, scroll-driven animations, `@font-face`, custom properties, breakpoints).
//!
//! A small scanner of our own, not a CSS engine: it skips comments and strings, so an
//! address inside either is never touched, and it runs in one pass over the text. Sheet
//! text is page data; nothing here runs or follows anything.

use url::Url;

/// The largest sheet analysed for facts (addresses are rewritten in any sheet).
pub const MAX_ANALYSED: usize = 8 * 1024 * 1024;
/// Rules looked at per sheet at most.
const MAX_RULES: usize = 40_000;
/// Facts of one kind kept per sheet at most.
const MAX_FACTS: usize = 400;

/// One address named in a sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlRef {
    /// Byte range of the whole token to replace: `url(…)`, or the string after `@import`, or
    /// the string inside `image-set(…)`.
    pub start: usize,
    pub end: usize,
    /// The address, made absolute against the sheet's own address (fragment kept).
    pub url: String,
    /// Named by `@import`: another sheet.
    pub import: bool,
}

/// Every address a sheet names, in order. `data:` addresses and fragment-only ones
/// (`url(#gradient)`) are left alone and not listed.
pub fn find_urls(css: &str, base: &Url) -> Vec<UrlRef> {
    let b = css.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    // Depth of an image-set( … ) whose strings are addresses, if inside one.
    let mut image_set: Option<usize> = None;
    let mut depth = 0usize;
    while i < b.len() {
        let c = b[i];
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i = skip_comment(b, i);
            continue;
        }
        if c == b'"' || c == b'\'' {
            let (end, value) = read_string(css, i);
            if image_set == Some(depth)
                && let Some(url) = resolve(&value, base)
            {
                out.push(UrlRef {
                    start: i,
                    end,
                    url,
                    import: false,
                });
            }
            i = end;
            continue;
        }
        if c == b'(' {
            depth += 1;
            i += 1;
            continue;
        }
        if c == b')' {
            if image_set == Some(depth) {
                image_set = None;
            }
            depth = depth.saturating_sub(1);
            i += 1;
            continue;
        }
        if (c == b'u' || c == b'U')
            && starts_with_ci(b, i, b"url(")
            && (i == 0 || !is_ident_byte(b[i - 1]))
        {
            if let Some((end, value)) = read_url_token(css, i) {
                if let Some(url) = resolve(&value, base) {
                    let import = preceded_by_import(b, i);
                    out.push(UrlRef {
                        start: i,
                        end,
                        url,
                        import,
                    });
                }
                i = end;
                continue;
            }
            i += 4;
            continue;
        }
        if c == b'@' && starts_with_ci(b, i, b"@import") {
            let mut j = i + 7;
            loop {
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if b.get(j) == Some(&b'/') && b.get(j + 1) == Some(&b'*') {
                    j = skip_comment(b, j);
                    continue;
                }
                break;
            }
            if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                let (end, value) = read_string(css, j);
                if let Some(url) = resolve(&value, base) {
                    out.push(UrlRef {
                        start: j,
                        end,
                        url,
                        import: true,
                    });
                }
                i = end;
                continue;
            }
            i = j;
            continue;
        }
        if (c == b'i' || c == b'-')
            && (i == 0 || !is_ident_byte(b[i - 1]))
            && (starts_with_ci(b, i, b"image-set(") || starts_with_ci(b, i, b"-webkit-image-set("))
        {
            let open = i + css[i..].find('(').unwrap_or(0);
            depth += 1;
            image_set = Some(depth);
            i = open + 1;
            continue;
        }
        i += 1;
    }
    out
}

/// The sheet with each listed address replaced by what `replace` gives (written as
/// `url("…")`), or left as written when it gives nothing.
pub fn rewrite(css: &str, refs: &[UrlRef], replace: impl Fn(&UrlRef) -> Option<String>) -> String {
    let mut out = String::with_capacity(css.len());
    let mut last = 0;
    for r in refs {
        if r.start < last || r.end > css.len() {
            continue;
        }
        if let Some(new) = replace(r) {
            out.push_str(&css[last..r.start]);
            out.push_str("url(\"");
            out.push_str(&escape_css_string(&new));
            out.push_str("\")");
            last = r.end;
        }
    }
    out.push_str(&css[last..]);
    out
}

/// A value safe inside a double-quoted CSS string.
pub fn escape_css_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\22 "),
            '\\' => out.push_str("\\5c "),
            '\n' => out.push_str("\\a "),
            '\r' => out.push_str("\\d "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn resolve(raw: &str, base: &Url) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() || t.starts_with('#') {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("data:") || lower.starts_with("blob:") || lower.starts_with("about:") {
        return None;
    }
    let u = base.join(t).ok()?;
    matches!(u.scheme(), "http" | "https").then(|| u.to_string())
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c >= 0x80
}

fn starts_with_ci(b: &[u8], i: usize, pat: &[u8]) -> bool {
    b.len() >= i + pat.len() && b[i..i + pat.len()].eq_ignore_ascii_case(pat)
}

fn skip_comment(b: &[u8], i: usize) -> usize {
    let mut j = i + 2;
    while j + 1 < b.len() {
        if b[j] == b'*' && b[j + 1] == b'/' {
            return j + 2;
        }
        j += 1;
    }
    b.len()
}

/// Reads the string starting at `i` (a quote): its end (just past the closing quote, or at
/// a line break or the end for an unclosed one) and its value with escapes undone.
fn read_string(css: &str, i: usize) -> (usize, String) {
    let b = css.as_bytes();
    let q = b[i];
    let mut j = i + 1;
    let mut raw_end = b.len();
    let mut end = b.len();
    while j < b.len() {
        if b[j] == b'\\' {
            j += 2;
            continue;
        }
        if b[j] == q {
            raw_end = j;
            end = j + 1;
            break;
        }
        if b[j] == b'\n' {
            raw_end = j;
            end = j;
            break;
        }
        j += 1;
    }
    let raw_end = raw_end.min(b.len());
    let start = (i + 1).min(raw_end);
    let raw = css.get(start..raw_end).unwrap_or("");
    (end.min(b.len()), unescape(raw))
}

/// Reads a `url(` token at `i`: its end (past the `)`) and the address. `None` when it is
/// not a well-formed token (then it is left alone).
fn read_url_token(css: &str, i: usize) -> Option<(usize, String)> {
    let b = css.as_bytes();
    let mut j = i + 4;
    while j < b.len() && b[j].is_ascii_whitespace() {
        j += 1;
    }
    if j >= b.len() {
        return None;
    }
    let value;
    if b[j] == b'"' || b[j] == b'\'' {
        let (end, v) = read_string(css, j);
        value = v;
        j = end;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        // `url("x" format(…))` is not a url token; leave it.
        if b.get(j) != Some(&b')') {
            return None;
        }
        return Some((j + 1, value));
    }
    let start = j;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b')' => break,
            c if c.is_ascii_whitespace() => break,
            b'"' | b'\'' | b'(' => return None,
            _ => j += 1,
        }
    }
    let raw = css.get(start..j.min(b.len()))?;
    while j < b.len() && b[j].is_ascii_whitespace() {
        j += 1;
    }
    if b.get(j) != Some(&b')') {
        return None;
    }
    Some((j + 1, unescape(raw)))
}

fn preceded_by_import(b: &[u8], i: usize) -> bool {
    let mut j = i;
    while j > 0 && b[j - 1].is_ascii_whitespace() {
        j -= 1;
    }
    j >= 7 && b[j - 7..j].eq_ignore_ascii_case(b"@import")
}

/// Undoes CSS escapes: `\` + up to six hex digits (and one space), or `\` + a character.
fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let mut hex = String::new();
        while hex.len() < 6 {
            match chars.peek() {
                Some(h) if h.is_ascii_hexdigit() => {
                    hex.push(*h);
                    chars.next();
                }
                _ => break,
            }
        }
        if hex.is_empty() {
            match chars.next() {
                Some('\n') | None => {}
                Some(other) => out.push(other),
            }
        } else {
            if matches!(chars.peek(), Some(' ' | '\t' | '\n')) {
                chars.next();
            }
            let cp = u32::from_str_radix(&hex, 16).unwrap_or(0xFFFD);
            out.push(
                char::from_u32(cp)
                    .filter(|c| *c != '\0')
                    .unwrap_or('\u{FFFD}'),
            );
        }
    }
    out
}

// ---- facts ------------------------------------------------------------------------------

/// One `@font-face` rule.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FontFace {
    pub family: String,
    pub weight: String,
    pub style: String,
    /// Absolute addresses of its `src` files.
    pub sources: Vec<String>,
}

/// What a sheet says about the look and motion of the page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SheetFacts {
    /// `@keyframes` rules: name and full text.
    pub keyframes: Vec<(String, String)>,
    /// Preludes of `@media` rules.
    pub media: Vec<String>,
    /// Rules for hover, focus and active states: selector, declarations, media.
    pub states: Vec<(String, String, String)>,
    /// `transition` declarations: selector and value.
    pub transitions: Vec<(String, String)>,
    /// Rules that use scroll-driven animations: selector and the declarations about it.
    pub scroll_driven: Vec<(String, String)>,
    pub font_faces: Vec<FontFace>,
    /// Custom properties set on `:root`, `html` or `body`: name and value.
    pub custom_properties: Vec<(String, String)>,
    /// Style rules seen.
    pub rules: usize,
    /// The sheet was too large or had too many rules to be read to its end.
    pub cut: bool,
}

/// Reads the facts of one sheet. `base` resolves `@font-face` addresses.
pub fn facts(css: &str, base: &Url) -> SheetFacts {
    let mut f = SheetFacts::default();
    let text = if css.len() > MAX_ANALYSED {
        f.cut = true;
        let mut cut = MAX_ANALYSED;
        while !css.is_char_boundary(cut) {
            cut -= 1;
        }
        &css[..cut]
    } else {
        css
    };
    let mut p = Parser {
        css: text,
        b: text.as_bytes(),
        i: 0,
        facts: &mut f,
        base,
    };
    p.rules("", "", 0);
    f
}

struct Parser<'a> {
    css: &'a str,
    b: &'a [u8],
    i: usize,
    facts: &'a mut SheetFacts,
    base: &'a Url,
}

impl Parser<'_> {
    /// Reads rules until a `}` at this level or the end. `parent` is the selector of an
    /// enclosing style rule (CSS nesting), `media` the enclosing `@media` prelude.
    fn rules(&mut self, parent: &str, media: &str, level: usize) {
        while self.i < self.b.len() {
            if self.facts.rules >= MAX_RULES {
                self.facts.cut = true;
                self.i = self.b.len();
                return;
            }
            self.skip_space_and_comments();
            if self.i >= self.b.len() {
                return;
            }
            match self.b[self.i] {
                b'}' => {
                    self.i += 1;
                    return;
                }
                b';' => {
                    self.i += 1;
                }
                b'@' => self.at_rule(parent, media, level),
                _ => {
                    let prelude_start = self.i;
                    let Some(open) = self.find_top(b"{;}") else {
                        self.i = self.b.len();
                        return;
                    };
                    if self.b[open] != b'{' {
                        // A declaration where a rule was expected (nesting): skip it.
                        self.i = open + usize::from(self.b[open] == b';');
                        continue;
                    }
                    let selector = squash(&self.css[prelude_start..open]);
                    self.i = open + 1;
                    let full = nest(parent, &selector);
                    self.style_block(&full, media, level);
                }
            }
        }
    }

    fn at_rule(&mut self, parent: &str, media: &str, level: usize) {
        let start = self.i;
        let name_end = self.b[start + 1..]
            .iter()
            .position(|c| !(c.is_ascii_alphanumeric() || *c == b'-' || *c == b'_'))
            .map(|p| start + 1 + p)
            .unwrap_or(self.b.len());
        let name = self.css[start + 1..name_end].to_ascii_lowercase();
        self.i = name_end;
        let Some(stop) = self.find_top(b"{;}") else {
            self.i = self.b.len();
            return;
        };
        let prelude = squash(&self.css[name_end..stop]);
        if self.b[stop] != b'{' {
            self.i = stop + usize::from(self.b[stop] == b';');
            return;
        }
        let body_start = stop + 1;
        match name.as_str() {
            "keyframes" | "-webkit-keyframes" | "-moz-keyframes" | "-o-keyframes" => {
                let end = self.block_end(body_start);
                if self.facts.keyframes.len() < MAX_FACTS {
                    let text = &self.css[start..end.min(self.css.len())];
                    let text = if text.len() > 6000 {
                        cut_at(text, 6000)
                    } else {
                        text.to_string()
                    };
                    let name = prelude.trim_matches(|c| c == '"' || c == '\'').to_string();
                    self.facts.keyframes.push((name, text));
                }
                self.i = end;
            }
            "font-face" => {
                let end = self.block_end(body_start);
                let body = &self.css[body_start..end.saturating_sub(1).max(body_start)];
                self.font_face(body);
                self.i = end;
            }
            "media" => {
                if self.facts.media.len() < MAX_FACTS {
                    self.facts.media.push(prelude.clone());
                }
                self.i = body_start;
                let m = if media.is_empty() {
                    prelude
                } else {
                    format!("{media} and {prelude}")
                };
                self.rules(parent, &m, level + 1);
            }
            "supports" | "layer" | "container" | "document" | "-moz-document" | "scope"
            | "starting-style" => {
                self.i = body_start;
                self.rules(parent, media, level + 1);
            }
            _ => {
                // @page, @property, @counter-style, @font-feature-values…: skipped.
                self.i = self.block_end(body_start);
            }
        }
    }

    /// The declarations (and nested rules) of a style rule whose `{` was just read.
    fn style_block(&mut self, selector: &str, media: &str, level: usize) {
        self.facts.rules += 1;
        let mut decls: Vec<(String, String)> = Vec::new();
        loop {
            self.skip_space_and_comments();
            if self.i >= self.b.len() {
                break;
            }
            if self.b[self.i] == b'}' {
                self.i += 1;
                break;
            }
            if self.b[self.i] == b';' {
                self.i += 1;
                continue;
            }
            let start = self.i;
            let Some(stop) = self.find_top(b"{;}") else {
                self.i = self.b.len();
                break;
            };
            if self.b[stop] == b'{' {
                // A nested rule (or a nested at-rule).
                if self.b[start] == b'@' {
                    self.i = start;
                    self.at_rule(selector, media, level + 1);
                } else {
                    let inner = squash(&self.css[start..stop]);
                    self.i = stop + 1;
                    let full = nest(selector, &inner);
                    self.style_block(&full, media, level + 1);
                }
                continue;
            }
            let decl = &self.css[start..stop];
            if let Some(colon) = decl.find(':') {
                // Custom property names keep their case (`--Brand` is not `--brand`).
                let prop = decl[..colon].trim();
                let prop = if prop.starts_with("--") {
                    prop.to_string()
                } else {
                    prop.to_ascii_lowercase()
                };
                let value = squash(&decl[colon + 1..]);
                if !prop.is_empty() {
                    decls.push((prop, value));
                }
            }
            self.i = stop + usize::from(self.b[stop] == b';');
        }
        self.record(selector, media, &decls);
    }

    fn record(&mut self, selector: &str, media: &str, decls: &[(String, String)]) {
        let lower = selector.to_ascii_lowercase();
        let joined = || {
            let s = decls
                .iter()
                .map(|(p, v)| format!("{p}: {v}"))
                .collect::<Vec<_>>()
                .join("; ");
            cut_at(&s, 600)
        };
        if [":hover", ":focus", ":active"]
            .iter()
            .any(|s| lower.contains(s))
            && self.facts.states.len() < MAX_FACTS
            && !decls.is_empty()
        {
            self.facts
                .states
                .push((cut_at(selector, 300), joined(), cut_at(media, 200)));
        }
        for (p, v) in decls {
            if (p == "transition" || p == "transition-property" || p == "-webkit-transition")
                && self.facts.transitions.len() < MAX_FACTS
                && !v.eq_ignore_ascii_case("none")
            {
                self.facts
                    .transitions
                    .push((cut_at(selector, 300), cut_at(v, 300)));
            }
        }
        let scroll: Vec<&(String, String)> = decls
            .iter()
            .filter(|(p, v)| {
                p == "animation-timeline"
                    || p == "view-timeline"
                    || p == "view-timeline-name"
                    || p == "scroll-timeline"
                    || p == "scroll-timeline-name"
                    || p == "animation-range"
                    || (p == "animation" && (v.contains("scroll(") || v.contains("view(")))
            })
            .collect();
        if !scroll.is_empty() && self.facts.scroll_driven.len() < MAX_FACTS {
            let s = scroll
                .iter()
                .map(|(p, v)| format!("{p}: {v}"))
                .collect::<Vec<_>>()
                .join("; ");
            self.facts
                .scroll_driven
                .push((cut_at(selector, 300), cut_at(&s, 400)));
        }
        let root = lower
            .split(',')
            .any(|s| matches!(s.trim(), ":root" | "html" | "body" | ":host"));
        if root {
            for (p, v) in decls {
                if p.starts_with("--") && self.facts.custom_properties.len() < MAX_FACTS {
                    self.facts
                        .custom_properties
                        .push((cut_at(p, 80), cut_at(v, 200)));
                }
            }
        }
    }

    fn font_face(&mut self, body: &str) {
        let mut ff = FontFace::default();
        for decl in split_top(body, b';') {
            let Some(colon) = decl.find(':') else {
                continue;
            };
            let prop = decl[..colon].trim().to_ascii_lowercase();
            let value = squash(&decl[colon + 1..]);
            match prop.as_str() {
                "font-family" => {
                    ff.family = cut_at(value.trim_matches(|c| c == '"' || c == '\''), 100)
                }
                "font-weight" => ff.weight = cut_at(&value, 40),
                "font-style" => ff.style = cut_at(&value, 40),
                "src" => {
                    ff.sources = find_urls(&value, self.base)
                        .into_iter()
                        .map(|r| r.url)
                        .take(12)
                        .collect()
                }
                _ => {}
            }
        }
        if !ff.family.is_empty() && self.facts.font_faces.len() < MAX_FACTS {
            self.facts.font_faces.push(ff);
        }
    }

    fn skip_space_and_comments(&mut self) {
        loop {
            while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
            if self.b.get(self.i) == Some(&b'/') && self.b.get(self.i + 1) == Some(&b'*') {
                self.i = skip_comment(self.b, self.i);
                continue;
            }
            // Old HTML comment markers some sheets still carry.
            if self.css[self.i..].starts_with("<!--") {
                self.i += 4;
                continue;
            }
            if self.css[self.i..].starts_with("-->") {
                self.i += 3;
                continue;
            }
            break;
        }
    }

    /// The first of `stops` at this level from the current position, outside strings,
    /// comments and brackets.
    fn find_top(&self, stops: &[u8]) -> Option<usize> {
        let mut j = self.i;
        let mut paren = 0usize;
        let mut square = 0usize;
        while j < self.b.len() {
            let c = self.b[j];
            match c {
                b'/' if self.b.get(j + 1) == Some(&b'*') => {
                    j = skip_comment(self.b, j);
                    continue;
                }
                b'"' | b'\'' => {
                    j = read_string(self.css, j).0;
                    continue;
                }
                b'\\' => {
                    j += 2;
                    continue;
                }
                b'(' => paren += 1,
                b')' => paren = paren.saturating_sub(1),
                b'[' => square += 1,
                b']' => square = square.saturating_sub(1),
                _ if paren == 0 && square == 0 && stops.contains(&c) => return Some(j),
                _ => {}
            }
            j += 1;
        }
        None
    }

    /// Just past the `}` that closes the block whose content starts at `start`.
    fn block_end(&self, start: usize) -> usize {
        let mut j = start;
        let mut depth = 1usize;
        while j < self.b.len() {
            match self.b[j] {
                b'/' if self.b.get(j + 1) == Some(&b'*') => {
                    j = skip_comment(self.b, j);
                    continue;
                }
                b'"' | b'\'' => {
                    j = read_string(self.css, j).0;
                    continue;
                }
                b'\\' => {
                    j += 2;
                    continue;
                }
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return j + 1;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        self.b.len()
    }
}

/// Splits at `sep` outside strings, comments and brackets.
fn split_top(s: &str, sep: u8) -> Vec<&str> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut j = 0;
    let mut paren = 0usize;
    while j < b.len() {
        match b[j] {
            b'/' if b.get(j + 1) == Some(&b'*') => {
                j = skip_comment(b, j);
                continue;
            }
            b'"' | b'\'' => {
                j = read_string(s, j).0;
                continue;
            }
            b'(' => paren += 1,
            b')' => paren = paren.saturating_sub(1),
            c if c == sep && paren == 0 => {
                out.push(&s[start..j]);
                start = j + 1;
            }
            _ => {}
        }
        j += 1;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// A nested selector joined to its parent (`&` replaced, or a descendant otherwise).
fn nest(parent: &str, inner: &str) -> String {
    if parent.is_empty() {
        return inner.to_string();
    }
    if inner.contains('&') {
        inner.replace('&', parent)
    } else {
        format!("{parent} {inner}")
    }
}

/// Whitespace collapsed to single spaces and comments dropped.
fn squash(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut j = 0;
    let mut space = false;
    while j < b.len() {
        if b[j] == b'/' && b.get(j + 1) == Some(&b'*') {
            j = skip_comment(b, j);
            space = true;
            continue;
        }
        let ch_len = match b[j] {
            0x00..=0x7F => 1,
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            _ => 4,
        };
        let piece = &s[j..(j + ch_len).min(s.len())];
        if piece.chars().all(char::is_whitespace) {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push_str(piece);
        }
        j += ch_len;
    }
    out
}

fn cut_at(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// The breakpoints media rules use: `(min-width: 768px)` and the like, with how often.
pub fn breakpoints<'a>(preludes: impl Iterator<Item = &'a String>) -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for p in preludes {
        let lower = p.to_ascii_lowercase();
        let mut rest = lower.as_str();
        while let Some(pos) = rest.find("width") {
            let before = &rest[..pos];
            let kind = if before.ends_with("min-") {
                "min-width"
            } else if before.ends_with("max-") {
                "max-width"
            } else {
                "width"
            };
            let after = rest[pos + 5..].trim_start();
            let value: String = after
                .trim_start_matches([':', '<', '>', '=', ' '])
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '.')
                .collect();
            if !value.is_empty() && value.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                *counts.entry(format!("{kind}: {value}")).or_default() += 1;
            }
            rest = &rest[pos + 5..];
        }
    }
    let mut v: Vec<(String, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://cdn.example.com/css/site.css?v=3").unwrap()
    }

    fn urls(css: &str) -> Vec<String> {
        find_urls(css, &base()).into_iter().map(|r| r.url).collect()
    }

    #[test]
    fn every_way_of_naming_an_address_is_found() {
        let css = r#"
            a { background: url(img/a.png) }
            b { background: url( "../img/b.png" ) }
            c { background: URL('/img/c.png') }
            d { background: url(https://other.example/d.png) }
            e { background: url(//static.example/e.png) }
            f { background: url(data:image/png;base64,AAAA) }
            g { background: url(#gradient) }
            h { background: url("x\"y.png") url(z\).png) }
            i { background: image-set("i1.png" 1x, url(i2.png) 2x) }
            j { background: -webkit-image-set('j.png' 1x) }
            k { content: "url(not-an-address.png)" }
            /* l { background: url(commented.png) } */
            m { background: myurl(nope.png) }
            n { mask: url(sprite.svg#icon) }
        "#;
        assert_eq!(
            urls(css),
            vec![
                "https://cdn.example.com/css/img/a.png",
                "https://cdn.example.com/img/b.png",
                "https://cdn.example.com/img/c.png",
                "https://other.example/d.png",
                "https://static.example/e.png",
                "https://cdn.example.com/css/x%22y.png",
                "https://cdn.example.com/css/z).png",
                "https://cdn.example.com/css/i1.png",
                "https://cdn.example.com/css/i2.png",
                "https://cdn.example.com/css/j.png",
                "https://cdn.example.com/css/sprite.svg#icon",
            ]
        );
    }

    #[test]
    fn imports_in_both_forms_are_marked() {
        let css = "@import \"reset.css\";\n@import url(theme.css) screen;\n@import /* c */ 'print.css' print;\nbody{}";
        let refs = find_urls(css, &base());
        assert_eq!(refs.len(), 3);
        assert!(refs.iter().all(|r| r.import), "{refs:?}");
        assert_eq!(refs[1].url, "https://cdn.example.com/css/theme.css");
    }

    #[test]
    fn rewriting_replaces_only_the_tokens_and_quotes_safely() {
        let css = "a{background:url(img/a.png) no-repeat}@import 'b.css';c{x:url(data:image/gif;base64,R0)}d{y:url(missing.png)}";
        let refs = find_urls(css, &base());
        let out = rewrite(css, &refs, |r| {
            if r.url.ends_with("a.png") {
                Some("../assets/a.png".into())
            } else if r.url.ends_with("b.css") {
                Some("b.css".into())
            } else {
                None
            }
        });
        assert_eq!(
            out,
            "a{background:url(\"../assets/a.png\") no-repeat}@import url(\"b.css\");c{x:url(data:image/gif;base64,R0)}d{y:url(missing.png)}"
        );
        assert_eq!(escape_css_string("a\"b\\c\nd"), "a\\22 b\\5c c\\a d");
    }

    #[test]
    fn hostile_text_cannot_break_out_of_a_rewritten_address() {
        let css = "a{background:url(\"x.png\\\" ) } body { color: red\")}";
        let refs = find_urls(css, &base());
        let out = rewrite(css, &refs, |_| Some("../assets/x.png".into()));
        // The whole string token was one address and is replaced as one.
        assert!(
            out.starts_with("a{background:url(\"../assets/x.png\")"),
            "{out}"
        );
    }

    #[test]
    fn facts_cover_keyframes_states_transitions_fonts_and_breakpoints() {
        let css = r#"
            @font-face { font-family: "Studio Sans"; src: url("../fonts/s.woff2") format("woff2"), url(../fonts/s.woff) format("woff"); font-weight: 400 700; font-style: normal; }
            :root { --brand: #3355ff; --radius: 12px; }
            .cta { transition: background-color .2s ease, transform .2s ease; }
            .cta:hover { background-color: #2244dd; transform: translateY(-2px); }
            a:focus-visible { outline: 2px solid var(--brand) }
            @keyframes fade-up { from { opacity: 0 } to { opacity: 1 } }
            @-webkit-keyframes spin { to { transform: rotate(360deg) } }
            @media (min-width: 768px) { .grid { display: grid } .grid:hover { gap: 2px } }
            @media screen and (max-width: 600px) { .hero { padding: 16px } }
            .bar { animation: grow linear; animation-timeline: scroll(root); }
            .card { &:hover { scale: 1.02 } .title { color: red } }
            @supports (display: grid) { .x:active { color: blue } }
        "#;
        let f = facts(css, &base());
        assert_eq!(
            f.keyframes.iter().map(|k| k.0.as_str()).collect::<Vec<_>>(),
            ["fade-up", "spin"]
        );
        assert!(
            f.keyframes[0].1.starts_with("@keyframes fade-up {"),
            "{:?}",
            f.keyframes
        );
        assert!(f.keyframes[0].1.ends_with('}'));
        let states: Vec<&str> = f.states.iter().map(|s| s.0.as_str()).collect();
        assert_eq!(
            states,
            [
                ".cta:hover",
                "a:focus-visible",
                ".grid:hover",
                ".card:hover",
                ".x:active"
            ]
        );
        assert_eq!(f.states[2].2, "(min-width: 768px)");
        assert!(f.states[0].1.contains("background-color: #2244dd"));
        assert_eq!(
            f.transitions,
            vec![(
                ".cta".into(),
                "background-color .2s ease, transform .2s ease".into()
            )]
        );
        assert_eq!(f.scroll_driven.len(), 1);
        assert!(
            f.scroll_driven[0]
                .1
                .contains("animation-timeline: scroll(root)")
        );
        assert_eq!(f.font_faces.len(), 1);
        assert_eq!(f.font_faces[0].family, "Studio Sans");
        assert_eq!(f.font_faces[0].weight, "400 700");
        assert_eq!(
            f.font_faces[0].sources,
            [
                "https://cdn.example.com/fonts/s.woff2",
                "https://cdn.example.com/fonts/s.woff"
            ]
        );
        assert_eq!(
            f.custom_properties,
            vec![
                ("--brand".into(), "#3355ff".into()),
                ("--radius".into(), "12px".into())
            ]
        );
        let bp = breakpoints(f.media.iter());
        assert_eq!(
            bp,
            vec![
                ("max-width: 600px".into(), 1),
                ("min-width: 768px".into(), 1)
            ]
        );
        assert!(!f.cut);
    }

    #[test]
    fn broken_and_huge_sheets_end_without_panicking() {
        for css in [
            "",
            "{",
            "}",
            "a{",
            "@media",
            "@media (",
            "a{b:url(",
            "a{b:\"unclosed",
            "/* open",
            "@keyframes x { from { opacity: 0 }",
            "a{b:c}}}}",
            "@import",
            "url(",
            "\\",
            "@font-face{src:url(\\",
        ] {
            let _ = facts(css, &base());
            let _ = find_urls(css, &base());
        }
        let big = "a{color:red}".repeat(50_000);
        let f = facts(&big, &base());
        assert!(
            f.cut,
            "a sheet with more rules than the limit says it was cut"
        );
        assert_eq!(f.rules, MAX_RULES);
    }
}
