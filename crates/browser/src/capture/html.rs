//! The copy's `index.html` (ARCHITECTURE.md 9.3, `browser_capture_site`).
//!
//! The page script (`js/collect.js`) serialises an inert copy of the page with every
//! address the app may replace written as a token `hcap-<nonce>-<kind><n>`. The nonce is
//! random for each capture, so text on the page cannot hold a token. Here each token
//! becomes the local file it points to, or, when that file was not saved, `about:invalid`:
//! the copy never names a resource elsewhere. `capture::inert` then parses the result
//! again, cleans it as a tree and checks it.

use std::collections::HashMap;

/// The policy written into the copy as its first `<meta>`: nothing runs, nothing is
/// fetched from anywhere but the copy's own folder, nothing is sent.
pub const COPY_POLICY: &str = "default-src 'none'; img-src 'self' data: blob: file:; style-src 'self' 'unsafe-inline' file:; font-src 'self' data: file:; media-src 'self' data: blob: file:; script-src 'none'; script-src-attr 'none'; object-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'; manifest-src 'none'";

/// A one-pixel transparent picture: what a canvas gets when no still of it was saved.
pub const BLANK_PICTURE: &str =
    "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

/// Where the tokens of one page point.
#[derive(Debug, Default)]
pub struct Targets {
    /// `u<n>`: the saved file (relative to `index.html`), else `None`.
    pub urls: Vec<Option<String>>,
    /// `s<n>`: the saved style sheet (relative to `index.html`), else `None`.
    pub styles: Vec<Option<String>>,
    /// `x<n>`: the srcset value, already made of saved files only (may be empty).
    pub srcsets: Vec<String>,
    /// `c<n>`: the saved still of a canvas, else `None`.
    pub canvases: Vec<Option<String>>,
    /// `v<n>` (a `<use>` of a shape in another SVG file, indexed like `u<n>`): `true` when
    /// that file is copied into the page (`capture::inert::make_inert`), so the reference
    /// becomes the fragment alone; otherwise it is treated like `u<n>`.
    pub inlined: Vec<bool>,
}

/// The page with every token replaced. A token whose file was not saved, or that names
/// nothing known, becomes an address that loads nothing.
pub fn replace_tokens(html: &str, nonce: &str, t: &Targets) -> String {
    let prefix = format!("hcap-{nonce}-");
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(pos) = rest.find(&prefix) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + prefix.len()..];
        let kind = after.chars().next();
        let digits: String = after
            .chars()
            .skip(1)
            .take_while(char::is_ascii_digit)
            .take(9)
            .collect();
        let consumed = prefix.len() + kind.map_or(0, char::len_utf8) + digits.len();
        let index: Option<usize> = digits.parse().ok();
        let replacement = match (kind, index) {
            (Some('v'), Some(n)) if t.inlined.get(n).copied().unwrap_or(false) => String::new(),
            (Some('u' | 'v'), Some(n)) => match t.urls.get(n) {
                Some(Some(local)) => attr_text(local),
                _ => "about:invalid".into(),
            },
            (Some('s'), Some(n)) => match t.styles.get(n) {
                Some(Some(local)) => attr_text(&safe_url_text(local)),
                _ => "about:invalid".into(),
            },
            (Some('x'), Some(n)) => t.srcsets.get(n).map(|s| attr_text(s)).unwrap_or_default(),
            (Some('c'), Some(n)) => match t.canvases.get(n) {
                Some(Some(local)) => attr_text(local),
                _ => BLANK_PICTURE.to_string(),
            },
            _ => "about:invalid".into(),
        };
        out.push_str(&replacement);
        rest = &rest[pos + consumed.min(rest.len() - pos)..];
    }
    out.push_str(rest);
    out
}

/// The largest SVG file copied into the page for its shapes.
pub const MAX_SPRITE_BYTES: usize = 256 * 1024;

/// The indexes of the `v<n>` tokens in the page (shapes of other SVG files).
pub fn sprite_indexes(html: &str, nonce: &str) -> Vec<usize> {
    let prefix = format!("hcap-{nonce}-v");
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find(&prefix) {
        let after = &rest[pos + prefix.len()..];
        let digits: String = after
            .chars()
            .take_while(char::is_ascii_digit)
            .take(9)
            .collect();
        if let Ok(n) = digits.parse::<usize>()
            && !out.contains(&n)
        {
            out.push(n);
        }
        rest = &rest[pos + prefix.len()..];
    }
    out
}

/// A srcset value made of the candidates whose files were saved, in their order.
pub fn srcset(candidates: &[(String, String)], saved: &HashMap<String, String>) -> String {
    candidates
        .iter()
        .filter_map(|(url, descriptor)| {
            let local = saved.get(url.split('#').next().unwrap_or(url))?;
            let d = clean_descriptor(descriptor);
            Some(if d.is_empty() {
                local.clone()
            } else {
                format!("{local} {d}")
            })
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A srcset descriptor (`2x`, `640w`) or nothing.
fn clean_descriptor(d: &str) -> String {
    let d = d.trim();
    let ok = !d.is_empty()
        && d.len() <= 12
        && d.ends_with(['x', 'w'])
        && d[..d.len() - 1]
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.');
    if ok { d.to_string() } else { String::new() }
}

/// An address with the characters that could end a quoted attribute or CSS string, or
/// start markup, percent-encoded.
pub fn safe_url_text(u: &str) -> String {
    let mut out = String::with_capacity(u.len());
    for c in u.chars() {
        match c {
            '"' | '\'' | '<' | '>' | '\\' | '(' | ')' | '`' | ' ' => {
                out.push_str(&format!("%{:02X}", c as u32));
            }
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Text safe inside a double-quoted HTML attribute.
fn attr_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets() -> Targets {
        Targets {
            urls: vec![
                Some("assets/hero.png".into()),
                None,
                Some("assets/sprite.svg".into()),
            ],
            styles: vec![Some("styles/site.css".into()), None],
            srcsets: vec!["assets/hero.png 1x, assets/hero-2.png 2x".into()],
            canvases: vec![None],
            inlined: vec![],
        }
    }

    #[test]
    fn tokens_become_local_files_or_load_nothing() {
        let html = r#"<img src="hcap-n1-u0" srcset="hcap-n1-x0"><img src="hcap-n1-u1"><svg><use href="hcap-n1-u2#dot"></use></svg><link rel="stylesheet" href="hcap-n1-s0"><link rel="stylesheet" href="hcap-n1-s1"><canvas style="background:url(&quot;hcap-n1-c0&quot;) center"></canvas><p>hcap-n1-u99 hcap-other-u0 hcap-n1-q1</p>"#;
        let out = replace_tokens(html, "n1", &targets());
        assert!(
            out.contains(
                r#"<img src="assets/hero.png" srcset="assets/hero.png 1x, assets/hero-2.png 2x">"#
            ),
            "{out}"
        );
        // Not saved: nothing to load, never the address elsewhere.
        assert!(out.contains(r#"<img src="about:invalid">"#), "{out}");
        assert!(
            out.contains(r#"<link rel="stylesheet" href="about:invalid">"#),
            "{out}"
        );
        assert!(!out.contains("a.example"), "{out}");
        assert!(out.contains(r#"href="assets/sprite.svg#dot""#), "{out}");
        assert!(out.contains(r#"href="styles/site.css""#), "{out}");
        assert!(out.contains(BLANK_PICTURE), "{out}");
        // Unknown tokens load nothing; another nonce is plain text.
        assert!(
            out.contains("<p>about:invalid hcap-other-u0 about:invalid</p>"),
            "{out}"
        );
    }

    #[test]
    fn shapes_of_other_svg_files_are_copied_into_the_page() {
        let mut t = targets();
        t.inlined = vec![false, false, true];
        let html = r##"<html><head></head><body class="x"><svg><use href="hcap-n1-v2#dot"></use><use href="hcap-n1-v0#a"></use></svg></body></html>"##;
        assert_eq!(sprite_indexes(html, "n1"), [2, 0]);
        let out = replace_tokens(html, "n1", &t);
        assert!(out.contains(r##"<use href="#dot">"##), "{out}");
        assert!(
            out.contains(r##"<use href="assets/hero.png#a">"##),
            "not copied in: the saved file: {out}"
        );
    }

    #[test]
    fn srcsets_keep_only_saved_candidates_and_plain_descriptors() {
        let mut saved = HashMap::new();
        saved.insert(
            "https://a.example/a.png".to_string(),
            "assets/a.png".to_string(),
        );
        saved.insert(
            "https://a.example/c.png".to_string(),
            "assets/c.png".to_string(),
        );
        let c = vec![
            ("https://a.example/a.png".to_string(), "1x".to_string()),
            ("https://a.example/b.png".to_string(), "2x".to_string()),
            (
                "https://a.example/c.png#f".to_string(),
                "640w\" onerror=x".to_string(),
            ),
        ];
        assert_eq!(srcset(&c, &saved), "assets/a.png 1x, assets/c.png");
        assert_eq!(srcset(&[], &saved), "");
    }

    #[test]
    fn the_policy_blocks_scripts_and_requests() {
        for part in [
            "script-src 'none'",
            "connect-src 'none'",
            "form-action 'none'",
            "frame-src 'none'",
            "object-src 'none'",
            "base-uri 'none'",
            "default-src 'none'",
        ] {
            assert!(COPY_POLICY.contains(part), "{part}");
        }
        assert!(
            !COPY_POLICY.contains("http"),
            "nothing is loaded from any web address"
        );
    }
}
