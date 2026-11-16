//! Reading style sheets for `browser_capture_site` (ARCHITECTURE.md 9.3): the addresses a
//! sheet names (`url(…)`, `src(…)`, `@import`, `image-set(…)`), rewritten to the local
//! copies, and the facts the report and `animations.json` need (`@keyframes`, hover and
//! focus rules, transitions, scroll-driven animations, `@font-face`, custom properties,
//! breakpoints).
//!
//! Addresses are found with the CSS tokenizer browsers' engines share (`cssparser`, the one
//! Servo and Firefox use), so a name written with escapes (`u\72l(…)`, `@\69mport`) or an
//! address inside a comment or a string is seen exactly as a browser sees it. [`confine`]
//! is the rule for the copy: an address is one of the copy's own files, a fragment or
//! `data:`, or it becomes `about:invalid`. The facts come from a small scanner of our own
//! that skips comments and strings. Sheet text is page data; nothing here runs or follows
//! anything.

use cssparser::{ParseError, Parser as CssParser, ParserInput, Token};
use url::Url;

/// The largest sheet analysed for facts (addresses are rewritten in any sheet).
pub const MAX_ANALYSED: usize = 8 * 1024 * 1024;
/// Rules looked at per sheet at most.
const MAX_RULES: usize = 40_000;
/// Facts of one kind kept per sheet at most.
const MAX_FACTS: usize = 400;
/// Blocks inside blocks followed at most; text nested deeper cannot be confined.
const MAX_NESTING: usize = 64;
/// What an address the copy may not keep becomes.
pub const INERT_URL: &str = "about:invalid";

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

/// One address-bearing token as written, of any kind (`data:`, fragments, any scheme).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRef {
    /// Byte range of the whole token, as for [`UrlRef`].
    pub start: usize,
    pub end: usize,
    /// The address with CSS escapes undone, not resolved.
    pub raw: String,
    /// Named by `@import`.
    pub import: bool,
    /// A malformed `url(` token, which browsers ignore.
    pub bad: bool,
}

/// Every address-bearing token of a text, and whether all of it could be read.
#[derive(Debug, Default)]
pub struct RawUrls {
    pub refs: Vec<RawRef>,
    /// False when blocks are nested deeper than [`MAX_NESTING`]: tokens deeper than that
    /// were not looked at.
    pub complete: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    None,
    Import,
    Namespace,
}

/// Functions that make a picture out of what they hold. Inside them a string is an address
/// (`image-set("a.png" 1x)`), and a `var()`, `env()` or `attr()` could put any string there
/// at the time the page is shown, out of sight of anything reading the text; so their
/// contents are held to [`INSIDE_IMAGES`].
const IMAGE_FUNCTIONS: &[&str] = &[
    "image-set",
    "-webkit-image-set",
    "image",
    "cross-fade",
    "-webkit-cross-fade",
];

/// The only functions an image function may hold besides addresses (`url()`, `src()`) and
/// other image functions: the type of an `image-set()` option, gradients and what their
/// colours and positions are written with. Anything else in an image function (`var()`,
/// `env()`, `attr()`, any other name) makes the whole function `url("about:invalid")`.
const INSIDE_IMAGES: &[&str] = &[
    "type",
    "linear-gradient",
    "radial-gradient",
    "conic-gradient",
    "repeating-linear-gradient",
    "repeating-radial-gradient",
    "repeating-conic-gradient",
    "-webkit-linear-gradient",
    "-webkit-radial-gradient",
    "-webkit-repeating-linear-gradient",
    "-webkit-repeating-radial-gradient",
    "-webkit-gradient",
    "from",
    "to",
    "color-stop",
    "rgb",
    "rgba",
    "hsl",
    "hsla",
    "hwb",
    "lab",
    "lch",
    "oklab",
    "oklch",
    "color",
    "color-mix",
    "calc",
    "min",
    "max",
    "clamp",
];

/// Every token of `css` that names an address: `url(…)` in either form (and malformed
/// ones), `src(…)`, the string or `url(…)` after `@import`, and the strings inside image
/// functions ([`IMAGE_FUNCTIONS`]). An image function holding anything outside
/// [`INSIDE_IMAGES`] is listed whole, as a malformed token. The address of `@namespace` is
/// not one (nothing is fetched) and is left out.
pub fn find_raw_urls(css: &str) -> RawUrls {
    let mut input = ParserInput::new(css);
    let mut parser = CssParser::new(&mut input);
    let mut out = RawUrls {
        refs: Vec::new(),
        complete: true,
    };
    walk(&mut parser, &mut out, 0);
    out
}

fn walk<'i>(p: &mut CssParser<'i, '_>, out: &mut RawUrls, depth: usize) {
    let mut pending = Pending::None;
    loop {
        let start = p.position().byte_index();
        let token = match p.next_including_whitespace_and_comments() {
            Ok(t) => t.clone(),
            Err(_) => return,
        };
        match token {
            Token::WhiteSpace(_) | Token::Comment(_) => {}
            Token::UnquotedUrl(value) => {
                if pending != Pending::Namespace {
                    out.refs.push(RawRef {
                        start,
                        end: p.position().byte_index(),
                        raw: value.to_string(),
                        import: pending == Pending::Import,
                        bad: false,
                    });
                }
                pending = Pending::None;
            }
            Token::BadUrl(value) => {
                out.refs.push(RawRef {
                    start,
                    end: p.position().byte_index(),
                    raw: value.to_string(),
                    import: pending == Pending::Import,
                    bad: true,
                });
                pending = Pending::None;
            }
            Token::QuotedString(value) => {
                if pending == Pending::Import {
                    out.refs.push(RawRef {
                        start,
                        end: p.position().byte_index(),
                        raw: value.to_string(),
                        import: true,
                        bad: false,
                    });
                }
                pending = Pending::None;
            }
            Token::Function(name) => {
                let lname = name.to_ascii_lowercase();
                if lname == "url" || lname == "src" {
                    let raw = address_function(p);
                    if pending != Pending::Namespace {
                        out.refs.push(RawRef {
                            start,
                            end: p.position().byte_index(),
                            bad: raw.is_none(),
                            raw: raw.unwrap_or_default(),
                            import: pending == Pending::Import,
                        });
                    }
                } else if IMAGE_FUNCTIONS.contains(&lname.as_str()) {
                    let mut inner: Vec<RawRef> = Vec::new();
                    let ok = p
                        .parse_nested_block(|q| -> Result<bool, ParseError<'i, ()>> {
                            Ok(image_walk(q, &mut inner, depth + 1, true))
                        })
                        .unwrap_or(false);
                    if ok {
                        out.refs.extend(inner);
                    } else {
                        // Held to nothing: the whole function becomes `about:invalid`.
                        out.refs.push(RawRef {
                            start,
                            end: p.position().byte_index(),
                            raw: String::new(),
                            import: false,
                            bad: true,
                        });
                    }
                } else if depth >= MAX_NESTING {
                    // The block is skipped by the next token read; what it holds is unknown.
                    out.complete = false;
                } else {
                    let _ = p.parse_nested_block(|q| -> Result<(), ParseError<'i, ()>> {
                        walk(q, out, depth + 1);
                        Ok(())
                    });
                }
                pending = Pending::None;
            }
            Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock => {
                if depth >= MAX_NESTING {
                    out.complete = false;
                } else {
                    let _ = p.parse_nested_block(|q| -> Result<(), ParseError<'i, ()>> {
                        walk(q, out, depth + 1);
                        Ok(())
                    });
                }
                pending = Pending::None;
            }
            Token::AtKeyword(keyword) => {
                pending = if keyword.eq_ignore_ascii_case("import") {
                    Pending::Import
                } else if keyword.eq_ignore_ascii_case("namespace") {
                    Pending::Namespace
                } else {
                    Pending::None
                };
            }
            // `@namespace svg url(…)`: the prefix comes first.
            Token::Ident(_) if pending == Pending::Namespace => {}
            _ => pending = Pending::None,
        }
    }
}

/// The address in `url("…")` or `src("…")`, the parser just past the function's name: one
/// string and nothing else (spaces and comments aside). `None` for anything more or less
/// (`src(var(--a))`), which is then treated as malformed.
fn address_function(p: &mut CssParser<'_, '_>) -> Option<String> {
    p.parse_nested_block(|q| -> Result<Option<String>, ParseError<'_, ()>> {
        let mut raw: Option<String> = None;
        while let Ok(t) = q.next_including_whitespace_and_comments() {
            match t {
                Token::WhiteSpace(_) | Token::Comment(_) => {}
                Token::QuotedString(s) if raw.is_none() => raw = Some(s.to_string()),
                _ => return Ok(None),
            }
        }
        Ok(raw)
    })
    .ok()
    .flatten()
}

/// The inside of an image function: addresses go to `refs` (strings too where
/// `strings_are_urls`); `false` as soon as anything outside the allow-list shows up.
fn image_walk(p: &mut CssParser<'_, '_>, refs: &mut Vec<RawRef>, depth: usize, strings_are_urls: bool) -> bool {
    if depth >= MAX_NESTING {
        return false;
    }
    loop {
        let start = p.position().byte_index();
        let token = match p.next_including_whitespace_and_comments() {
            Ok(t) => t.clone(),
            Err(_) => return true,
        };
        match token {
            Token::WhiteSpace(_)
            | Token::Comment(_)
            | Token::Comma
            | Token::Number { .. }
            | Token::Percentage { .. }
            | Token::Dimension { .. }
            | Token::Ident(_)
            | Token::Hash(_)
            | Token::IDHash(_) => {}
            Token::Delim(c) if matches!(c, '/' | '+' | '-' | '*') => {}
            Token::QuotedString(value) => {
                if strings_are_urls {
                    refs.push(RawRef {
                        start,
                        end: p.position().byte_index(),
                        raw: value.to_string(),
                        import: false,
                        bad: false,
                    });
                }
            }
            Token::UnquotedUrl(value) => refs.push(RawRef {
                start,
                end: p.position().byte_index(),
                raw: value.to_string(),
                import: false,
                bad: false,
            }),
            Token::Function(name) => {
                let lname = name.to_ascii_lowercase();
                if lname == "url" || lname == "src" {
                    let Some(raw) = address_function(p) else {
                        return false;
                    };
                    refs.push(RawRef {
                        start,
                        end: p.position().byte_index(),
                        raw,
                        import: false,
                        bad: false,
                    });
                } else {
                    let image = IMAGE_FUNCTIONS.contains(&lname.as_str());
                    if !image && !INSIDE_IMAGES.contains(&lname.as_str()) {
                        return false;
                    }
                    let ok = p
                        .parse_nested_block(|q| -> Result<bool, ParseError<'_, ()>> {
                            Ok(image_walk(q, refs, depth + 1, image))
                        })
                        .unwrap_or(false);
                    if !ok {
                        return false;
                    }
                }
            }
            _ => return false,
        }
    }
}

/// CSS text as the copy writes it, so a browser reads it exactly as it was checked: no byte
/// order mark or leading `@charset` rule (a saved sheet starts with a UTF-8 byte order
/// mark instead, which wins over any `@charset`), and no control characters but tab, line
/// feed, form feed and carriage return (an escape byte can change how a legacy encoding
/// reads what follows).
pub fn plain_text(css: &str) -> String {
    let kept: String = css
        .trim_start_matches('\u{feff}')
        .chars()
        .filter(|c| !(c.is_control() && !matches!(c, '\t' | '\n' | '\u{c}' | '\r')) || !c.is_ascii())
        .collect();
    let lower_start: String = kept.chars().take(9).collect::<String>().to_ascii_lowercase();
    if lower_start.starts_with("@charset") {
        // The rule ends at its first `;` (a string with a `;` in it is not an encoding name).
        if let Some(end) = kept.find(';') {
            return kept[end + 1..].to_string();
        }
        return String::new();
    }
    kept
}

/// A UTF-8 byte order mark, written first in every saved sheet.
pub const BOM: &[u8] = b"\xEF\xBB\xBF";

/// Every address a sheet names, made absolute (only `http` and `https` ones), in order.
/// `data:` addresses, fragment-only ones (`url(#gradient)`) and malformed tokens are left
/// alone and not listed.
pub fn find_urls(css: &str, base: &Url) -> Vec<UrlRef> {
    find_raw_urls(css)
        .refs
        .into_iter()
        .filter(|r| !r.bad)
        .filter_map(|r| {
            let url = resolve(&r.raw, base)?;
            Some(UrlRef {
                start: r.start,
                end: r.end,
                url,
                import: r.import,
            })
        })
        .collect()
}

/// The sheet with each listed address replaced by what `replace` gives (written as
/// `url("…")`), or left as written when it gives nothing.
pub fn rewrite(css: &str, refs: &[UrlRef], replace: impl Fn(&UrlRef) -> Option<String>) -> String {
    rewrite_spans(css, refs.iter().map(|r| (r.start, r.end, replace(r))))
}

/// [`rewrite`] for the tokens of [`find_raw_urls`].
pub fn rewrite_raw(
    css: &str,
    refs: &[RawRef],
    replace: impl Fn(&RawRef) -> Option<String>,
) -> String {
    rewrite_spans(css, refs.iter().map(|r| (r.start, r.end, replace(r))))
}

fn rewrite_spans(css: &str, spans: impl Iterator<Item = (usize, usize, Option<String>)>) -> String {
    let mut out = String::with_capacity(css.len());
    let mut last = 0;
    for (start, end, new) in spans {
        if start < last
            || end > css.len()
            || !css.is_char_boundary(start)
            || !css.is_char_boundary(end)
        {
            continue;
        }
        if let Some(new) = new {
            out.push_str(&css[last..start]);
            out.push_str("url(\"");
            out.push_str(&escape_css_string(&new));
            out.push_str("\")");
            last = end;
        }
    }
    out.push_str(&css[last..]);
    out
}

/// The text of an address as the URL parser reads it: tabs and line breaks dropped,
/// spaces and control characters trimmed at both ends.
pub fn url_text(raw: &str) -> String {
    let kept: String = raw
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    kept.trim_matches(|c: char| c <= ' ').to_string()
}

/// `css` as [`plain_text`], with every address the copy may not keep replaced by
/// [`INERT_URL`]: kept are fragments (`url(#clip)`), `data:` addresses and addresses for
/// which `is_local` says the text names one of the copy's own files; an `@import` is kept
/// only when it names one of those, and an image function holding anything but literal
/// addresses, gradients and their arguments becomes [`INERT_URL`] whole. `None` when the
/// text is nested too deeply to be read whole.
pub fn confine(css: &str, is_local: impl Fn(&str) -> bool) -> Option<String> {
    let css = plain_text(css);
    let found = find_raw_urls(&css);
    if !found.complete {
        return None;
    }
    Some(rewrite_raw(&css, &found.refs, |r| {
        let t = url_text(&r.raw);
        let lower = t.to_ascii_lowercase();
        let keep = !r.bad
            && !t.is_empty()
            && if r.import {
                is_local(&t)
            } else {
                t.starts_with('#') || lower.starts_with("data:") || is_local(&t)
            };
        (!keep).then(|| INERT_URL.to_string())
    }))
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
    fn names_written_with_escapes_are_addresses_too() {
        // A browser reads `u\72l(` as `url(` and `@\69mport` as `@import`.
        let css = "a{background:u\\72l(file://attacker.example/share/a.png)}@\\69mport 'x.css';b{c:\\55 RL(\"//evil.example/b.png\")}d{e:url(a b)}f{g:src(\"s.png\")}@namespace svg url(http://www.w3.org/2000/svg);";
        let found = find_raw_urls(css);
        assert!(found.complete);
        let raws: Vec<(&str, bool, bool)> = found
            .refs
            .iter()
            .map(|r| (r.raw.as_str(), r.import, r.bad))
            .collect();
        assert_eq!(
            raws,
            [
                ("file://attacker.example/share/a.png", false, false),
                ("x.css", true, false),
                ("//evil.example/b.png", false, false),
                ("a b", false, true),
                ("s.png", false, false),
            ],
            "@namespace names no file: {:?}",
            found.refs
        );
        // Each range covers the whole token, so a rewrite replaces it whole.
        let first = &found.refs[0];
        assert_eq!(
            &css[first.start..first.end],
            "u\\72l(file://attacker.example/share/a.png)"
        );
    }

    #[test]
    fn the_copy_keeps_only_its_own_files_fragments_and_data() {
        let local = |t: &str| t == "../assets/a.png" || t == "theme.css";
        let css = "a{background:url(../assets/a.png),url(file://attacker.example/s/x.png)}\
                   b{mask:url(#m);background:url(data:image/png;base64,AA)}\
                   @import url(theme.css);@import \"https://evil.example/x.css\";\
                   @import url(data:text/css,b{x:y});c{d:u\\72l(//evil.example/y.png)}\
                   e{f:url(a b)}g{h:image-set(\"https://cdn.example/i.png\" 1x)}i{j:url(\"\")}";
        let out = confine(css, local).unwrap();
        assert!(out.contains("url(../assets/a.png)"), "{out}");
        assert!(
            out.contains("url(#m)") && out.contains("url(data:image/png;base64,AA)"),
            "{out}"
        );
        assert!(out.contains("@import url(theme.css)"), "{out}");
        for gone in [
            "attacker.example",
            "evil.example",
            "cdn.example",
            "data:text/css",
            "a b",
        ] {
            assert!(!out.contains(gone), "{gone} is still in {out}");
        }
        assert_eq!(out.matches(INERT_URL).count(), 7, "{out}");
        // Confining again changes nothing.
        assert_eq!(confine(&out, local).unwrap(), out);
        // Text nested deeper than can be read is not confined at all.
        let deep = format!(
            "{}url(file://x/y){}",
            "(".repeat(MAX_NESTING + 5),
            ")".repeat(MAX_NESTING + 5)
        );
        assert!(confine(&deep, local).is_none());
        assert_eq!(url_text(" \tda\nta:image/png,x "), "data:image/png,x");
    }

    /// The verification of round 1 (V1): a string reaches an image function through a
    /// custom property, out of sight of the text.
    #[test]
    fn image_functions_hold_only_literal_addresses() {
        let local = |t: &str| t == "../assets/a.png";
        for (css, kept) in [
            (":root{--u:\"//attacker.example/share/x.png\"} .a{background-image:image-set(var(--u) 1x)}", false),
            (".h{--hh:\"http://93.184.215.14/x.png\";background-image:-webkit-image-set(var(--hh) 1x)}", false),
            (".l{list-style-image:image-set(var(--u) 1x, \"../assets/a.png\" 2x)}", false),
            (".c{content:image-set(env(safe-area-inset-top) 1x)}", false),
            (".m{mask-image:image-set(attr(data-src) 1x)}", false),
            (".b{border-image-source:image-set(made-up(\"x\") 1x)}", false),
            (".n{background:image-set(image-set(var(--u) 1x) 1x)}", false),
            (".x{background:cross-fade(var(--a), url(../assets/a.png))}", false),
            (".i{background:image(var(--u))}", false),
            (".s{background:src(var(--u))}", false),
            (".k{background:image-set(linear-gradient(red, rgb(0 0 0 / 50%)) 1x, \"../assets/a.png\" 2x)}", true),
            (".t{background:image-set(\"../assets/a.png\" type(\"image/png\") 1x)}", true),
        ] {
            let out = confine(css, local).unwrap();
            assert_eq!(
                out.contains("image-set(") || out.contains("image(") || out.contains("cross-fade(") || out.contains("src("),
                kept,
                "{css}\n=> {out}"
            );
            if !kept {
                assert!(out.contains("url(\"about:invalid\")"), "{out}");
            }
            // What went is the function that made a picture; a string left in a custom
            // property is only text without it.
            for gone in ["var(", "made-up(", "attr(", "env("] {
                assert!(!out.contains(gone), "{gone}: {out}");
            }
            assert_eq!(confine(&out, local).unwrap(), out, "confining again changes nothing");
        }
        // Inside an allowed image function, a string that is not the copy's own file goes too.
        let out = confine(".k{background:image-set(\"//attacker.example/a.png\" 1x, linear-gradient(red, blue) 2x)}", local).unwrap();
        assert!(!out.contains("attacker.example") && out.contains("linear-gradient(red, blue)"), "{out}");
    }

    /// The verification of round 1 (V2): a sheet that says `@charset "ISO-2022-JP"` and hides
    /// an `@import` behind an escape byte, which that encoding reads as nothing.
    #[test]
    fn a_saved_sheet_is_read_as_utf_8_like_it_was_checked() {
        let esc = '\u{1b}';
        let css = format!(
            "\u{feff}@charset \"ISO-2022-JP\";\n/* *{esc}(B/@import url(//attacker.example/share/x.css);/**/\n.m {{ color: red; }}\u{7f}\u{0}"
        );
        let plain = plain_text(&css);
        assert!(!plain.contains(esc) && !plain.contains('\u{feff}') && !plain.contains('\u{0}') && !plain.contains('\u{7f}'), "{plain:?}");
        assert!(!plain.to_ascii_lowercase().contains("@charset"), "{plain:?}");
        assert!(plain.starts_with('\n'), "{plain:?}");
        // Read as UTF-8 (the byte order mark makes the browser do so), the import is still
        // inside the comment, as checked.
        assert!(find_raw_urls(&plain).refs.is_empty(), "{plain:?}");
        assert_eq!(plain_text(&plain), plain);
        assert_eq!(plain_text("a{b:c}\t\u{c}\r\n"), "a{b:c}\t\u{c}\r\n");
        assert_eq!(plain_text("@CHARSET \"x\"; a{}"), " a{}");
        assert!(confine(&css, |_| true).is_some_and(|c| !c.contains(esc) && !c.contains("@charset")));
        assert_eq!(BOM, "\u{feff}".as_bytes());
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
