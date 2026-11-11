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

/// The text of an HTML page.
pub fn html_to_text(html: &str) -> Page {
    let _ = html;
    unimplemented!()
}

/// The text of a plain-text page: its first line as the title, line ends made uniform,
/// invisible characters removed.
pub fn plain_to_text(text: &str) -> Page {
    let _ = text;
    unimplemented!()
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

/// Text from a search service (a title or a snippet) that may carry markup and character
/// references: its text on one line, without the tags.
pub fn fragment_to_line(html: &str) -> String {
    let _ = html;
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARTICLE: &str = r#"<!DOCTYPE html>
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
</html>"#;

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
        for gone in ["SVG-TEXT", "IFRAME-TEXT", "TEMPLATE-TEXT", "OBJECT-TEXT", "onerror"] {
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
        let plain = plain_to_text("Title line\r\n\r\nBody\u{200B} text\u{202E}.\r\n\r\n\r\n\r\nMore.\u{0007}\n");
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
        let html = format!("<body><p>Start.</p>{}<p>End.</p></body>", "<div>".repeat(400_000));
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
        assert_eq!(html_to_text("<p>Unclosed <b>tags <i>everywhere").text, "Unclosed tags everywhere");
        assert_eq!(html_to_text("just text, no tags").text, "just text, no tags");
        assert_eq!(html_to_text("<<<>>>&&&;;;<p").text, "<<<>>>&&&;;;");
        // Text is cut at the cap, at a character.
        let huge = format!("<p>{}</p>", "é".repeat(MAX_TEXT_CHARS + 500));
        assert_eq!(html_to_text(&huge).text.chars().count(), MAX_TEXT_CHARS);
        assert_eq!(plain_to_text(&"é".repeat(MAX_TEXT_CHARS + 5)).text.chars().count(), MAX_TEXT_CHARS);
    }

    #[test]
    fn the_content_type_decides_how_a_body_is_read() {
        assert_eq!(page_text("text/html", "<p>a <b>b</b></p>").text, "a b");
        assert_eq!(page_text("TEXT/HTML", "<p>a</p>").text, "a");
        assert_eq!(page_text("text/plain", "<p>a <b>b</b></p>").text, "<p>a <b>b</b></p>");
    }

    #[test]
    fn snippets_from_a_search_service_lose_their_markup() {
        assert_eq!(
            fragment_to_line("Runs the closure on a thread where <strong>blocking</strong> is fine &amp; safe&hellip;"),
            "Runs the closure on a thread where blocking is fine & safe…"
        );
        assert_eq!(fragment_to_line("  two\n lines\t here "), "two lines here");
        assert_eq!(fragment_to_line("<script>alert(1)</script>shown"), "shown");
        assert_eq!(fragment_to_line("a\u{202E}b<br>c"), "ab c");
        assert_eq!(fragment_to_line(""), "");
        assert_eq!(fragment_to_line("5 < 6 and 7 > 2"), "5 < 6 and 7 > 2");
    }
}
