//! The copy's `index.html` (ARCHITECTURE.md 9.3, `browser_capture_site`).
//!
//! The page script (`js/collect.js`) serialises an inert copy of the page with every
//! address the app may replace written as a token `hcap-<nonce>-<kind><n>`. The nonce is
//! random for each capture, so text on the page cannot hold a token. Here each token
//! becomes the local file it points to, or, when that file was not saved, the original
//! address (which the copy's own policy keeps from loading).

use std::collections::HashMap;

/// The policy written into the copy as its first `<meta>`: nothing runs, nothing is
/// fetched from anywhere but the copy's own folder, nothing is sent.
pub const COPY_POLICY: &str = "default-src 'none'; img-src 'self' data: blob: file:; style-src 'self' 'unsafe-inline' file:; font-src 'self' data: file:; media-src 'self' data: blob: file:; script-src 'none'; script-src-attr 'none'; object-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'; manifest-src 'none'";

/// A one-pixel transparent picture: what a canvas gets when no still of it was saved.
pub const BLANK_PICTURE: &str = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

/// Where the tokens of one page point.
#[derive(Debug, Default)]
pub struct Targets {
    /// `u<n>`: the saved file (relative to `index.html`), else `None`.
    pub urls: Vec<Option<String>>,
    /// `u<n>`: the original address, used when the file was not saved.
    pub url_originals: Vec<String>,
    /// `s<n>`: the saved style sheet (relative to `index.html`), or what to write instead.
    pub styles: Vec<String>,
    /// `x<n>`: the srcset value, already made of saved files only (may be empty).
    pub srcsets: Vec<String>,
    /// `c<n>`: the saved still of a canvas, else `None`.
    pub canvases: Vec<Option<String>>,
}

/// The page with every token replaced. A token that names nothing known becomes an
/// address that loads nothing.
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
            (Some('u'), Some(n)) => match t.urls.get(n) {
                Some(Some(local)) => attr_text(local),
                _ => t
                    .url_originals
                    .get(n)
                    .map(|u| attr_text(&safe_url_text(u)))
                    .unwrap_or_else(|| "about:invalid".into()),
            },
            (Some('s'), Some(n)) => t
                .styles
                .get(n)
                .map(|s| attr_text(&safe_url_text(s)))
                .unwrap_or_else(|| "about:invalid".into()),
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

/// Whether the copy still holds a script element or an event handler anywhere outside
/// text (a last check; the page script removed them and the policy blocks them anyway).
pub fn has_active_content(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut i = 0;
    let mut in_tag = false;
    let mut quote: Option<u8> = None;
    while i < b.len() {
        let c = b[i];
        if in_tag {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None => match c {
                    b'"' | b'\'' => quote = Some(c),
                    b'>' => in_tag = false,
                    c if c.is_ascii_whitespace() || c == b'/' => {
                        if lower[i + 1..].starts_with("on") {
                            let name: String = lower[i + 3..]
                                .chars()
                                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                                .collect();
                            let after = lower[i + 3 + name.len()..].trim_start();
                            if !name.is_empty() && after.starts_with('=') {
                                return true;
                            }
                        }
                    }
                    _ => {}
                },
            }
        } else if c == b'<' {
            if lower[i..].starts_with("<!--") {
                i = lower[i..].find("-->").map_or(b.len(), |e| i + e + 3);
                continue;
            }
            let name: String = lower[i + 1..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == ':' || *c == '-')
                .collect();
            let local = name.rsplit(':').next().unwrap_or("");
            if local == "script" {
                return true;
            }
            in_tag = !name.is_empty();
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets() -> Targets {
        Targets {
            urls: vec![Some("assets/hero.png".into()), None, Some("assets/sprite.svg".into())],
            url_originals: vec![
                "https://a.example/hero.png".into(),
                "https://a.example/x.png?a=1&b=\"2\"".into(),
                "https://a.example/sprite.svg".into(),
            ],
            styles: vec!["styles/site.css".into()],
            srcsets: vec!["assets/hero.png 1x, assets/hero-2.png 2x".into()],
            canvases: vec![None],
        }
    }

    #[test]
    fn tokens_become_local_files_or_harmless_originals() {
        let html = r#"<img src="hcap-n1-u0" srcset="hcap-n1-x0"><img src="hcap-n1-u1"><svg><use href="hcap-n1-u2#dot"></use></svg><link rel="stylesheet" href="hcap-n1-s0"><canvas style="background:url(&quot;hcap-n1-c0&quot;) center"></canvas><p>hcap-n1-u99 hcap-other-u0 hcap-n1-q1</p>"#;
        let out = replace_tokens(html, "n1", &targets());
        assert!(out.contains(r#"<img src="assets/hero.png" srcset="assets/hero.png 1x, assets/hero-2.png 2x">"#), "{out}");
        // Not saved: the original address, made safe for the attribute.
        assert!(out.contains(r#"<img src="https://a.example/x.png?a=1&amp;b=%222%22">"#), "{out}");
        assert!(out.contains(r#"href="assets/sprite.svg#dot""#), "{out}");
        assert!(out.contains(r#"href="styles/site.css""#), "{out}");
        assert!(out.contains(BLANK_PICTURE), "{out}");
        assert!(out.contains("about:invalid about:invalid"), "unknown tokens load nothing: {out}");
        assert!(out.contains("hcap-other-u0"), "another nonce is plain text: {out}");
    }

    #[test]
    fn srcsets_keep_only_saved_candidates_and_plain_descriptors() {
        let mut saved = HashMap::new();
        saved.insert("https://a.example/a.png".to_string(), "assets/a.png".to_string());
        saved.insert("https://a.example/c.png".to_string(), "assets/c.png".to_string());
        let c = vec![
            ("https://a.example/a.png".to_string(), "1x".to_string()),
            ("https://a.example/b.png".to_string(), "2x".to_string()),
            ("https://a.example/c.png#f".to_string(), "640w\" onerror=x".to_string()),
        ];
        assert_eq!(srcset(&c, &saved), "assets/a.png 1x, assets/c.png");
        assert_eq!(srcset(&[], &saved), "");
    }

    #[test]
    fn active_content_is_noticed_outside_text_only() {
        assert!(has_active_content("<p><script>x</script></p>"));
        assert!(has_active_content("<svg><svg:script>x</svg:script></svg>"));
        assert!(has_active_content("<img src=x onerror=alert(1)>"));
        assert!(has_active_content("<img src=\"x\"/onerror=\"y\">"));
        assert!(!has_active_content("<p title=\"onclick=x\">a &lt;script&gt; b</p>"));
        assert!(!has_active_content("<!-- <script> --><p>one= two</p>"));
        assert!(!has_active_content("<p class=\"x\" data-online=\"1\">ok</p>"));
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
        assert!(!COPY_POLICY.contains("http"), "nothing is loaded from any web address");
    }
}
