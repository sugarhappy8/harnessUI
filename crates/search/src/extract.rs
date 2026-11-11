//! A fetched page as plain text.
//!
//! The HTML is parsed into a tree (html5ever, through `dom_query`) and walked once. What
//! comes out is text only: the title, and the main content as paragraphs separated by a
//! blank line, with code blocks kept as they were written. Scripts, styles, navigation,
//! page headers and footers, side notes, forms and hidden elements are left out.
//!
//! The page is untrusted. Nothing of it is ever returned as markup: tags do not survive
//! (text that looks like a tag inside a code block is text), and characters that are
//! invisible or change the reading direction are removed.

/// A page reduced to text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Page {
    /// The page's title, on one line. Empty when it has none.
    pub title: String,
    /// The main content: paragraphs separated by a blank line.
    pub text: String,
}

/// Text kept from one page at most, in characters.
pub const MAX_TEXT_CHARS: usize = 1_000_000;
/// A title kept at most, in characters.
pub const MAX_TITLE_CHARS: usize = 300;
/// How long parsing one page may take. A page that needs longer is read as far as the
/// parser got.
pub const PARSE_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

/// Pieces a page is handed to the parser in, so that the time can be checked in between.
const PARSE_CHUNK_BYTES: usize = 4096;
/// A snippet or title from a search service read at most, in bytes.
const MAX_FRAGMENT_BYTES: usize = 16 * 1024;

/// Characters that are invisible or change how text is laid out (the set the approval
/// cards escape, `policy::sanitize`), and control characters other than line break and
/// tab. Removed from everything this module returns.
fn is_invisible(c: char) -> bool {
    let u = c as u32;
    (c.is_control() && c != '\n' && c != '\t')
        || matches!(u,
            0x00AD            // soft hyphen
            | 0x061C          // Arabic letter mark
            | 0x115F | 0x1160 // Hangul fillers
            | 0x17B4 | 0x17B5
            | 0x180B..=0x180F // Mongolian variation selectors and vowel separator
            | 0x200B..=0x200F // zero-width space and joiners, direction marks
            | 0x202A..=0x202E // direction embeddings and overrides
            | 0x2060..=0x206F // word joiner, invisible operators, direction isolates
            | 0x3164          // Hangul filler
            | 0xFE00..=0xFE0F // variation selectors
            | 0xFEFF          // byte-order mark
            | 0xFFA0
            | 0xFFF9..=0xFFFB // interlinear annotation
            | 0x1D173..=0x1D17A
            | 0xE0000..=0xE007F // tag characters
            | 0xE0100..=0xE01EF)
}

/// Parses `html` into a tree, giving up on the rest of the page when parsing takes longer
/// than [`PARSE_BUDGET`]. (Some nesting makes the parser look at every open element for
/// every new one; a page built for that would otherwise cost minutes.)
pub(crate) fn parse_html(html: &str) -> dom_query::Document {
    use html5ever::tendril::TendrilSink;
    let options = html5ever::ParseOpts {
        tree_builder: html5ever::tree_builder::TreeBuilderOpts {
            scripting_enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut parser = html5ever::parse_document(dom_query::Document::default(), options);
    let started = std::time::Instant::now();
    let mut rest = html;
    while !rest.is_empty() {
        let mut cut = rest.len().min(PARSE_CHUNK_BYTES);
        while !rest.is_char_boundary(cut) {
            cut += 1;
        }
        let (chunk, tail) = rest.split_at(cut);
        parser.process(chunk.into());
        rest = tail;
        if started.elapsed() > PARSE_BUDGET {
            tracing::debug!("a page took too long to parse; the rest of it is left out");
            break;
        }
    }
    parser.finish()
}

/// What stands between two pieces of text, weakest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Gap {
    None,
    Space,
    Cell,
    Line,
    Paragraph,
}

/// How an element takes part in the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Left out with everything inside it.
    Skip,
    /// Its own paragraph.
    Block,
    /// Its own line.
    Line,
    /// A list item: its own line, marked with a dash.
    Item,
    /// A table cell.
    Cell,
    /// A line break.
    Break,
    /// Code kept as written.
    Pre,
    /// Superscript: kept, unless it is a footnote marker.
    Sup,
    Inline,
}

fn kind_of(name: &str) -> Kind {
    match name {
        "script" | "style" | "noscript" | "template" | "iframe" | "frame" | "frameset" | "svg"
        | "math" | "canvas" | "nav" | "header" | "footer" | "aside" | "form" | "button"
        | "select" | "datalist" | "option" | "optgroup" | "textarea" | "input" | "dialog"
        | "menu" | "object" | "embed" | "applet" | "video" | "audio" | "picture" | "source"
        | "track" | "map" | "area" | "head" | "link" | "meta" | "base" | "img" | "title" => {
            Kind::Skip
        }
        "p" | "div" | "section" | "article" | "main" | "ul" | "ol" | "dl" | "table"
        | "blockquote" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "figure" | "figcaption"
        | "details" | "summary" | "address" | "hr" | "body" | "html" | "fieldset" | "center" => {
            Kind::Block
        }
        "tr" | "dt" | "dd" | "caption" => Kind::Line,
        "li" => Kind::Item,
        "td" | "th" => Kind::Cell,
        "br" => Kind::Break,
        "pre" | "listing" | "xmp" | "plaintext" => Kind::Pre,
        "sup" => Kind::Sup,
        _ => Kind::Inline,
    }
}

/// Whether an element is hidden from a reader: the `hidden` attribute, `aria-hidden`, an
/// inline style that hides it, or a role that marks page furniture.
fn is_hidden(element: &dom_query::Element) -> bool {
    if element.has_attr("hidden") {
        return true;
    }
    if element
        .attr("aria-hidden")
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
    {
        return true;
    }
    if let Some(role) = element.attr("role")
        && matches!(
            role.trim().to_ascii_lowercase().as_str(),
            "navigation" | "banner" | "contentinfo" | "complementary" | "search" | "dialog"
        )
    {
        return true;
    }
    element.attr("style").is_some_and(|style| {
        let style: String = style
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        style.contains("display:none") || style.contains("visibility:hidden")
    })
}

/// Whether superscript text is a footnote marker: `[1]`, `[note 3]`, `[citation needed]`.
fn is_marker(text: &str) -> bool {
    let text = text.trim();
    text.len() <= 40 && text.starts_with('[') && text.ends_with(']')
}

/// Collects the text of a walk.
struct Writer {
    out: String,
    chars: usize,
    limit: usize,
    gap: Gap,
    /// Written before the next text (the dash of a list item).
    prefix: Option<&'static str>,
    pre_depth: usize,
    pre: String,
    sup_depth: usize,
    sup: String,
    /// Elements found on the way that hold a page's main content (`main`, `article`).
    candidates: Vec<dom_query::NodeId>,
    candidate_depth: usize,
}

impl Writer {
    fn new(limit: usize) -> Self {
        Self {
            out: String::new(),
            chars: 0,
            limit,
            gap: Gap::None,
            prefix: None,
            pre_depth: 0,
            pre: String::new(),
            sup_depth: 0,
            sup: String::new(),
            candidates: Vec::new(),
            candidate_depth: 0,
        }
    }

    fn full(&self) -> bool {
        self.chars >= self.limit
    }

    fn ask(&mut self, gap: Gap) {
        self.gap = self.gap.max(gap);
    }

    fn put(&mut self, c: char) {
        if self.chars < self.limit {
            self.out.push(c);
            self.chars += 1;
        }
    }

    /// Writes what was asked to stand before the next text.
    fn open(&mut self) {
        let gap = std::mem::replace(&mut self.gap, Gap::None);
        if !self.out.is_empty() {
            let between = match gap {
                Gap::None => "",
                Gap::Space => " ",
                Gap::Cell => " | ",
                Gap::Line => "\n",
                Gap::Paragraph => "\n\n",
            };
            between.chars().for_each(|c| self.put(c));
        }
        if let Some(prefix) = self.prefix.take() {
            prefix.chars().for_each(|c| self.put(c));
        }
    }

    /// Running text: white space collapses to one space.
    fn text(&mut self, text: &str) {
        if self.pre_depth > 0 {
            self.pre.extend(text.chars().filter(|c| !is_invisible(*c)));
            return;
        }
        if self.sup_depth > 0 {
            self.sup.push_str(text);
            return;
        }
        for c in text.chars() {
            if is_invisible(c) {
                continue;
            }
            if c.is_whitespace() {
                self.ask(Gap::Space);
                continue;
            }
            self.open();
            self.put(c);
        }
    }

    /// Code: written as it is, as its own paragraph.
    fn code(&mut self, code: &str) {
        let code = code.trim_matches('\n').trim_end();
        if code.trim().is_empty() {
            return;
        }
        self.ask(Gap::Paragraph);
        self.open();
        code.chars().for_each(|c| self.put(c));
        self.ask(Gap::Paragraph);
    }

    /// Returns whether the walk goes into the element.
    fn enter(&mut self, name: &str, element: &dom_query::Element, id: dom_query::NodeId) -> bool {
        let kind = kind_of(name);
        if kind == Kind::Skip || is_hidden(element) {
            return false;
        }
        let is_candidate = matches!(name, "main" | "article")
            || element
                .attr("role")
                .is_some_and(|r| r.trim().eq_ignore_ascii_case("main"));
        if is_candidate {
            if self.candidate_depth == 0 && self.candidates.len() < 8 {
                self.candidates.push(id);
            }
            self.candidate_depth += 1;
        }
        if self.pre_depth > 0 {
            match kind {
                Kind::Pre => self.pre_depth += 1,
                Kind::Break => self.pre.push('\n'),
                // A highlighter that puts each line of code into its own block.
                Kind::Block | Kind::Line | Kind::Item
                    if !self.pre.is_empty() && !self.pre.ends_with('\n') =>
                {
                    self.pre.push('\n');
                }
                _ => {}
            }
            return true;
        }
        match kind {
            Kind::Block => self.ask(Gap::Paragraph),
            Kind::Line => self.ask(Gap::Line),
            Kind::Item => {
                self.ask(Gap::Line);
                self.prefix = Some("- ");
            }
            Kind::Cell => self.ask(Gap::Cell),
            Kind::Break => self.ask(Gap::Line),
            Kind::Pre => {
                self.pre_depth = 1;
                self.pre.clear();
            }
            Kind::Sup => {
                if self.sup_depth == 0 {
                    self.sup.clear();
                }
                self.sup_depth += 1;
            }
            Kind::Inline | Kind::Skip => {}
        }
        true
    }

    fn exit(&mut self, name: &str, element: &dom_query::Element) {
        let is_candidate = matches!(name, "main" | "article")
            || element
                .attr("role")
                .is_some_and(|r| r.trim().eq_ignore_ascii_case("main"));
        if is_candidate {
            self.candidate_depth = self.candidate_depth.saturating_sub(1);
        }
        let kind = kind_of(name);
        if self.pre_depth > 0 {
            if kind == Kind::Pre {
                self.pre_depth -= 1;
                if self.pre_depth == 0 {
                    let code = std::mem::take(&mut self.pre);
                    self.code(&code);
                }
            }
            return;
        }
        match kind {
            Kind::Block => self.ask(Gap::Paragraph),
            Kind::Line => self.ask(Gap::Line),
            Kind::Item => {
                self.ask(Gap::Line);
                self.prefix = None;
            }
            Kind::Sup => {
                self.sup_depth = self.sup_depth.saturating_sub(1);
                if self.sup_depth == 0 {
                    let sup = std::mem::take(&mut self.sup);
                    if !is_marker(&sup) {
                        self.text(&sup);
                    }
                }
            }
            Kind::Cell | Kind::Break | Kind::Pre | Kind::Inline | Kind::Skip => {}
        }
    }

    fn finish(mut self) -> String {
        // Text left in an element the page never closed.
        if self.pre_depth > 0 {
            self.pre_depth = 0;
            let code = std::mem::take(&mut self.pre);
            self.code(&code);
        }
        if self.sup_depth > 0 {
            self.sup_depth = 0;
            let sup = std::mem::take(&mut self.sup);
            self.text(&sup);
        }
        self.out
    }
}

/// Walks the tree below `root` in document order without using the call stack: a page
/// may nest its elements as deep as it likes.
fn walk(root: dom_query::NodeRef<'_>, writer: &mut Writer) {
    use dom_query::NodeData;
    let mut current = Some(root);
    let mut entering = true;
    while let Some(node) = current {
        if writer.full() {
            break;
        }
        let mut descend = false;
        if entering {
            node.query(|inner| match &inner.data {
                NodeData::Text { contents } => writer.text(contents),
                NodeData::Element(element) => {
                    descend = writer.enter(element.name.local.as_ref(), element, node.id);
                }
                NodeData::Document | NodeData::Fragment => descend = true,
                _ => {}
            });
        }
        if descend && let Some(child) = node.first_child() {
            current = Some(child);
            continue;
        }
        // Leaving: an element that was gone into (with or without children), or one the
        // walk came back up to.
        if descend || !entering {
            node.query(|inner| {
                if let NodeData::Element(element) = &inner.data {
                    writer.exit(element.name.local.as_ref(), element);
                }
            });
        }
        if node.id == root.id {
            break;
        }
        match node.next_sibling() {
            Some(next) => {
                current = Some(next);
                entering = true;
            }
            None => {
                current = node.parent();
                entering = false;
            }
        }
    }
}

/// The text below one node.
fn text_below(node: dom_query::NodeRef<'_>, limit: usize) -> Writer {
    let mut writer = Writer::new(limit);
    walk(node, &mut writer);
    writer
}

/// One line: white space of any kind becomes a single space.
fn one_line(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out.chars().take(max_chars).collect()
}

/// The first element called `name` below `root`, in document order.
fn first_named<'a>(
    root: dom_query::NodeRef<'a>,
    name: &str,
    skip: &[&str],
) -> Option<dom_query::NodeRef<'a>> {
    let mut current = Some(root);
    let mut entering = true;
    while let Some(node) = current {
        let mut descend = false;
        if entering {
            if node.has_name(name) {
                return Some(node);
            }
            descend = !skip.iter().any(|s| node.has_name(s));
        }
        if descend && let Some(child) = node.first_child() {
            current = Some(child);
            continue;
        }
        if node.id == root.id {
            break;
        }
        match node.next_sibling() {
            Some(next) => {
                current = Some(next);
                entering = true;
            }
            None => {
                current = node.parent();
                entering = false;
            }
        }
    }
    None
}

/// The text of an HTML page.
pub fn html_to_text(html: &str) -> Page {
    if html.trim().is_empty() {
        return Page::default();
    }
    let document = parse_html(html);
    let root = document.root();
    // Everything readable on the page, and on the way the elements that hold main content.
    let whole = text_below(root, MAX_TEXT_CHARS);
    let candidates = whole.candidates.clone();
    let mut text = whole.finish();
    let whole_chars = text.chars().count();
    let mut best: Option<String> = None;
    for id in candidates {
        let candidate =
            text_below(dom_query::NodeRef::new(id, &document.tree), MAX_TEXT_CHARS).finish();
        if best.as_ref().is_none_or(|b| candidate.len() > b.len()) {
            best = Some(candidate);
        }
    }
    // A `main` or `article` that holds a good part of the page is the page; a teaser in an
    // `article` next to the real text is not.
    if let Some(best) = best {
        let chars = best.chars().count();
        if chars >= 200 && chars * 100 >= whole_chars * 40 {
            text = best;
        }
    }
    let title_node = first_named(root, "title", &["body", "svg"])
        .or_else(|| first_named(root, "h1", &["head", "svg", "script", "style", "template"]));
    let title = title_node
        .map(|node| {
            // The title element holds text only; a heading may hold markup.
            let mut writer = Writer::new(MAX_TITLE_CHARS * 4);
            for child in node.children_it(false) {
                walk(child, &mut writer);
            }
            one_line(&writer.finish(), MAX_TITLE_CHARS)
        })
        .unwrap_or_default();
    Page {
        title,
        text: text.trim().to_string(),
    }
}

/// The text of a plain-text page: its first line as the title, line ends made uniform,
/// invisible characters removed.
pub fn plain_to_text(text: &str) -> Page {
    let mut out = String::new();
    let mut chars = 0;
    let mut line_breaks = 0;
    for c in text.chars() {
        if chars >= MAX_TEXT_CHARS {
            break;
        }
        if c == '\n' {
            line_breaks += 1;
            continue;
        }
        if is_invisible(c) {
            continue;
        }
        if c.is_whitespace() && c != '\t' && c != ' ' {
            // Other kinds of space read as a space.
            out.push(' ');
            chars += 1;
            continue;
        }
        if line_breaks > 0 && !out.is_empty() {
            // More than one empty line says no more than one.
            let breaks = line_breaks.min(2);
            (0..breaks).for_each(|_| out.push('\n'));
            chars += breaks;
        }
        line_breaks = 0;
        out.push(c);
        chars += 1;
    }
    let out: String = out.trim().chars().take(MAX_TEXT_CHARS).collect();
    let title = out
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| one_line(line, MAX_TITLE_CHARS))
        .unwrap_or_default();
    Page { title, text: out }
}

/// The text of a fetched body, by its content type (`text/html` or anything else as plain
/// text).
pub fn page_text(content_type: &str, body: &str) -> Page {
    if content_type.eq_ignore_ascii_case("text/html") {
        html_to_text(body)
    } else {
        plain_to_text(body)
    }
}

/// What pages say that stand in for the real page until a browser has passed a check.
const CHALLENGE_PHRASES: &[&str] = &[
    "just a moment",
    "checking your browser",
    "attention required",
    "client challenge",
    "verify you are human",
    "verifying you are human",
    "are you a robot",
    "are you human",
    "captcha",
    "enable javascript and cookies",
    "access denied",
    "unusual traffic",
    "request blocked",
    "a required part of this site couldn",
];

/// Whether a page is a check for browsers instead of the page that was asked for: a short
/// page that says one of the things such pages say. A long page that mentions a captcha
/// is an article about captchas.
pub fn looks_like_challenge(page: &Page) -> bool {
    if page.text.chars().count() > 1200 {
        return false;
    }
    let said = format!("{}\n{}", page.title, page.text).to_lowercase();
    CHALLENGE_PHRASES.iter().any(|phrase| said.contains(phrase))
}

/// Plain text on one line: invisible characters removed, white space of any kind as a
/// single space.
pub fn plain_to_line(text: &str) -> String {
    let visible: String = text.chars().filter(|c| !is_invisible(*c)).collect();
    one_line(&visible, usize::MAX)
}

/// Text from a search service (a title or a snippet) that may carry markup and character
/// references: its text on one line, without the tags.
pub fn fragment_to_line(html: &str) -> String {
    let mut cut = html.len().min(MAX_FRAGMENT_BYTES);
    while !html.is_char_boundary(cut) {
        cut -= 1;
    }
    let html = &html[..cut];
    if !html.contains(['<', '&']) {
        // Nothing to parse.
        return plain_to_line(html);
    }
    let fragment = dom_query::Document::fragment(html);
    let text = text_below(fragment.root(), MAX_FRAGMENT_BYTES).finish();
    one_line(&text, usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARTICLE: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>  spawn_blocking in tokio::task
     - Rust </title>
  <style>body { color: red } .x::after { content: "STYLE-TEXT" }</style>
  <script>window.dataLayer = ["SCRIPT-TEXT"];</script>
</head>
<body>
  <header><a href="/">Docs.rs</a> <span>HEADER-TEXT</span></header>
  <nav><ul><li><a href="/a">NAV-ONE</a><li><a href="/b">NAV-TWO</a></ul></nav>
  <main>
    <h1>Function <a href="#">spawn_blocking</a></h1>
    <p>Runs the provided closure on a thread where
       blocking is <em>acceptable</em>.</p>
    <p>In general, issuing a blocking call or performing a lot of compute in a
       future without yielding is problematic &amp; should be avoided.</p>
    <h2>Examples</h2>
    <pre><code class="language-rust">use tokio::task;

let res = task::spawn_blocking(move || {
    // do some compute-heavy work
    "done &lt;computing&gt;"
}).await?;
</code></pre>
    <ul>
      <li>First point</li>
      <li>Second <code>point</code></li>
    </ul>
    <form action="/search"><input name="q" value="FORM-TEXT"><button>FORM-BUTTON</button></form>
    <aside>ASIDE-TEXT</aside>
    <p hidden>HIDDEN-TEXT</p>
    <div aria-hidden="true">ARIA-HIDDEN-TEXT</div>
    <p style="color: red; display : none">STYLE-HIDDEN-TEXT</p>
    <span style="VISIBILITY:hidden">INVISIBLE-TEXT</span>
    <noscript>NOSCRIPT-TEXT</noscript>
    <p>Line one<br>line two</p>
  </main>
  <footer>FOOTER-TEXT &copy; 2026</footer>
  <script type="application/ld+json">{"name": "JSON-LD-TEXT"}</script>
</body>
</html>"##;

    #[test]
    fn the_title_and_the_main_text_come_out() {
        let page = html_to_text(ARTICLE);
        assert_eq!(page.title, "spawn_blocking in tokio::task - Rust");
        assert_eq!(
            page.text,
            "Function spawn_blocking\n\n\
             Runs the provided closure on a thread where blocking is acceptable.\n\n\
             In general, issuing a blocking call or performing a lot of compute in a future \
             without yielding is problematic & should be avoided.\n\n\
             Examples\n\n\
             use tokio::task;\n\nlet res = task::spawn_blocking(move || {\n    // do some compute-heavy work\n    \"done <computing>\"\n}).await?;\n\n\
             - First point\n- Second point\n\n\
             Line one\nline two"
        );
    }

    #[test]
    fn scripts_styles_navigation_forms_and_hidden_parts_are_dropped() {
        let page = html_to_text(ARTICLE);
        for gone in [
            "STYLE-TEXT",
            "SCRIPT-TEXT",
            "HEADER-TEXT",
            "NAV-ONE",
            "NAV-TWO",
            "FORM-TEXT",
            "FORM-BUTTON",
            "ASIDE-TEXT",
            "HIDDEN-TEXT",
            "ARIA-HIDDEN-TEXT",
            "STYLE-HIDDEN-TEXT",
            "INVISIBLE-TEXT",
            "NOSCRIPT-TEXT",
            "FOOTER-TEXT",
            "JSON-LD-TEXT",
            "Docs.rs",
        ] {
            assert!(!page.text.contains(gone), "{gone} was kept");
            assert!(!page.title.contains(gone), "{gone} is in the title");
        }
    }

    /// The output is text. Markup never survives, whatever the page does.
    #[test]
    fn nothing_comes_out_as_markup() {
        let page = html_to_text(
            "<body><p>Before <b>bold</b> <img src=x onerror=alert(1)> after</p>\
             <p>&lt;script&gt;alert(2)&lt;/script&gt;</p>\
             <svg><text>SVG-TEXT</text></svg><iframe src='https://evil.example'>IFRAME-TEXT</iframe>\
             <template><p>TEMPLATE-TEXT</p></template><object>OBJECT-TEXT</object></body>",
        );
        assert_eq!(page.text, "Before bold after\n\n<script>alert(2)</script>");
        // The second paragraph is text that a page wrote with character references: it is
        // data, shown as it reads. No element of the page is in the output.
        for gone in [
            "SVG-TEXT",
            "IFRAME-TEXT",
            "TEMPLATE-TEXT",
            "OBJECT-TEXT",
            "onerror",
        ] {
            assert!(!page.text.contains(gone), "{gone}");
        }
    }

    #[test]
    fn a_page_without_main_uses_the_body_and_a_small_article_does_not_win() {
        let page = html_to_text(
            "<html><head><title>T</title></head><body><div><p>Only paragraph.</p></div></body></html>",
        );
        assert_eq!(page.text, "Only paragraph.");
        // A teaser in an <article> next to the real text: the body is used.
        let long = "Real content sentence. ".repeat(40);
        let page = html_to_text(&format!(
            "<body><article><p>Teaser.</p></article><div><p>{long}</p></div></body>"
        ));
        assert!(page.text.contains("Real content sentence."));
        assert!(page.text.contains("Teaser."));
        // A real article next to a sidebar of links: the article is used.
        let page = html_to_text(&format!(
            "<body><div class=side><p>SIDEBAR-LINKS</p></div><article><p>{long}</p></article></body>"
        ));
        assert!(page.text.contains("Real content sentence."));
        assert!(!page.text.contains("SIDEBAR-LINKS"));
    }

    #[test]
    fn a_missing_title_falls_back_to_the_first_heading() {
        let page = html_to_text("<body><h1>The  heading</h1><p>Text.</p></body>");
        assert_eq!(page.title, "The heading");
        assert_eq!(html_to_text("<p>Text only.</p>").title, "");
        let long = format!("<title>{}</title>", "t".repeat(1000));
        assert_eq!(html_to_text(&long).title.chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn tables_and_definition_lists_keep_their_rows() {
        let page = html_to_text(
            "<body><table><tr><th>Name</th><th>Default</th></tr>\
             <tr><td>timeout</td><td>10 s</td></tr></table>\
             <dl><dt>Term</dt><dd>Its meaning.</dd></dl></body>",
        );
        assert_eq!(
            page.text,
            "Name | Default\ntimeout | 10 s\n\nTerm\nIts meaning."
        );
    }

    #[test]
    fn footnote_markers_are_left_out() {
        let page = html_to_text(
            "<body><p>Rust is fast.<sup class=\"reference\"><a href=\"#cite-1\">[1]</a></sup> \
             It has x<sup>2</sup> and E = mc<sup>2</sup>.<sup>[note 3]</sup></p></body>",
        );
        assert_eq!(page.text, "Rust is fast. It has x2 and E = mc2.");
    }

    #[test]
    fn invisible_and_direction_changing_characters_are_removed() {
        let page = html_to_text(
            "<body><p>a\u{200B}b\u{202E}c\u{FEFF}d\u{00AD}e\u{2066}f\u{0007}g\u{00A0}h</p>\
             <pre>x\u{202E}y\r\n\tz\u{0000}</pre></body>",
        );
        assert_eq!(page.text, "abcdefg h\n\nxy\n\tz");
        let plain = plain_to_text(
            "Title line\r\n\r\nBody\u{200B} text\u{202E}.\r\n\r\n\r\n\r\nMore.\u{0007}\n",
        );
        assert_eq!(plain.title, "Title line");
        assert_eq!(plain.text, "Title line\n\nBody text.\n\nMore.");
    }

    /// A page can nest elements as deep as it likes; the walk must not use the call stack.
    #[test]
    fn deep_nesting_does_not_overflow_the_stack() {
        let depth = 100_000;
        let spans = format!(
            "<p>{}Deep text.{}</p>",
            "<span>".repeat(depth),
            "</span>".repeat(depth)
        );
        assert_eq!(html_to_text(&spans).text, "Deep text.");
        let divs = format!(
            "<body>{}<p>Deep text.</p>{}</body>",
            "<div>".repeat(2000),
            "</div>".repeat(2000)
        );
        assert_eq!(html_to_text(&divs).text, "Deep text.");
    }

    /// Some nesting makes the HTML parser slow (each new block looks at every open one).
    /// A page built for that must cost a bounded time, not minutes.
    #[test]
    fn a_page_built_to_be_slow_is_cut_off() {
        let started = std::time::Instant::now();
        let html = format!(
            "<body><p>Start.</p>{}<p>End.</p></body>",
            "<div>".repeat(400_000)
        );
        let page = html_to_text(&html);
        let took = started.elapsed();
        assert!(
            took < PARSE_BUDGET + std::time::Duration::from_secs(6),
            "took {took:?}"
        );
        assert!(page.text.starts_with("Start."), "{:?}", page.text);
    }

    #[test]
    fn broken_and_empty_pages_give_what_they_have() {
        assert_eq!(html_to_text(""), Page::default());
        assert_eq!(html_to_text("   \n "), Page::default());
        assert_eq!(
            html_to_text("<p>Unclosed <b>tags <i>everywhere").text,
            "Unclosed tags everywhere"
        );
        assert_eq!(
            html_to_text("just text, no tags").text,
            "just text, no tags"
        );
        assert_eq!(html_to_text("<<<>>>&&&;;;<p").text, "<<<>>>&&&;;;");
        // Text is cut at the cap, at a character.
        let huge = format!("<p>{}</p>", "é".repeat(MAX_TEXT_CHARS + 500));
        assert_eq!(html_to_text(&huge).text.chars().count(), MAX_TEXT_CHARS);
        assert_eq!(
            plain_to_text(&"é".repeat(MAX_TEXT_CHARS + 5))
                .text
                .chars()
                .count(),
            MAX_TEXT_CHARS
        );
    }

    #[test]
    fn the_content_type_decides_how_a_body_is_read() {
        assert_eq!(page_text("text/html", "<p>a <b>b</b></p>").text, "a b");
        assert_eq!(page_text("TEXT/HTML", "<p>a</p>").text, "a");
        assert_eq!(
            page_text("text/plain", "<p>a <b>b</b></p>").text,
            "<p>a <b>b</b></p>"
        );
    }

    #[test]
    fn a_browser_check_is_not_the_page() {
        let page = |title: &str, text: &str| Page {
            title: title.into(),
            text: text.into(),
        };
        for (title, text) in [
            (
                "Just a moment...",
                "Enable JavaScript and cookies to continue",
            ),
            (
                "Client Challenge",
                "A required part of this site couldn\u{2019}t load. This may be due to a browser extension, network issues, or browser settings.",
            ),
            (
                "Attention Required! | Cloudflare",
                "Sorry, you have been blocked",
            ),
            ("", "Please complete the CAPTCHA to continue."),
            (
                "Access Denied",
                "You don't have permission to access this page.",
            ),
        ] {
            assert!(looks_like_challenge(&page(title, text)), "{title}: {text}");
        }
        assert!(!looks_like_challenge(&page(
            "Spawning | Tokio",
            "A Tokio task is an asynchronous green thread."
        )));
        assert!(!looks_like_challenge(&page("", "")));
        // A real article about such checks is long.
        let article = format!("How a captcha works. {}", "It shows a puzzle. ".repeat(100));
        assert!(!looks_like_challenge(&page("Captcha explained", &article)));
    }

    #[test]
    fn snippets_from_a_search_service_lose_their_markup() {
        assert_eq!(
            fragment_to_line(
                "Runs the closure on a thread where <strong>blocking</strong> is fine &amp; safe&hellip;"
            ),
            "Runs the closure on a thread where blocking is fine & safe…"
        );
        assert_eq!(fragment_to_line("  two\n lines\t here "), "two lines here");
        assert_eq!(fragment_to_line("<script>alert(1)</script>shown"), "shown");
        assert_eq!(fragment_to_line("a\u{202E}b<br>c"), "ab c");
        assert_eq!(fragment_to_line(""), "");
        assert_eq!(fragment_to_line("5 < 6 and 7 > 2"), "5 < 6 and 7 > 2");
    }
}
