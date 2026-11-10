//! Making approval-card text safe to read (ARCHITECTURE.md 13.3): every string on a
//! card was chosen by a model or a server, so control, bidirectional-override and
//! zero-width characters are shown as visible escapes, and hosts are shown in
//! punycode with the registrable part emphasised by the UI.

/// Characters that change how text is displayed without being visible.
fn is_invisible_or_control(c: char) -> bool {
    let u = c as u32;
    (c.is_control() && c != '\n' && c != '\t')
        || matches!(u,
            0x061C            // Arabic letter mark
            | 0x115F | 0x1160 // Hangul fillers
            | 0x17B4 | 0x17B5
            | 0x180B..=0x180F // Mongolian variation selectors and vowel separator
            | 0x200B..=0x200F // zero-width space/joiners, LRM, RLM
            | 0x202A..=0x202E // bidi embeddings and overrides
            | 0x2060..=0x206F // word joiner, invisible operators, bidi isolates
            | 0x3164          // Hangul filler
            | 0xFE00..=0xFE0F // variation selectors
            | 0xFEFF          // BOM / zero-width no-break space
            | 0xFFA0
            | 0xFFF9..=0xFFFB // interlinear annotation
            | 0x1D173..=0x1D17A
            | 0xE0000..=0xE007F // tag characters
            | 0xE0100..=0xE01EF)
}

/// Replaces control and invisible characters with `\u{…}` escapes. Newlines and tabs
/// are kept; the UI shows text in a monospace block where needed.
pub fn visible(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if is_invisible_or_control(c) {
            out.push_str(&format!("\\u{{{:04X}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// Single-line version for summaries: newlines and tabs are escaped too, and the
/// result is cut to `max` characters with an ellipsis.
pub fn one_line(text: &str, max: usize) -> String {
    let mut s = String::new();
    for c in visible(text).chars() {
        match c {
            '\n' => s.push_str("\\n"),
            '\t' => s.push_str("\\t"),
            c => s.push(c),
        }
    }
    if s.chars().count() > max {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        s
    }
}

/// The host of a URL as the card shows it: lowercase ASCII (punycode for
/// internationalised names), with the port when it is not the default.
pub fn display_host(url: &url::Url) -> Option<String> {
    let host = url.host_str()?.to_string();
    Some(match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    })
}

/// scheme://host[:port] — the "site" a browser grant covers.
pub fn origin(url: &url::Url) -> Option<String> {
    let host = display_host(url)?;
    Some(format!("{}://{}", url.scheme(), host))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_bidi_zero_width_and_controls() {
        // "evil\u{202E}txt.exe" displays as "evilexe.txt" without escaping.
        assert_eq!(visible("evil\u{202E}txt.exe"), "evil\\u{202E}txt.exe");
        assert_eq!(visible("a\u{200B}b"), "a\\u{200B}b");
        assert_eq!(visible("x\u{FEFF}"), "x\\u{FEFF}");
        assert_eq!(visible("bell\u{7}"), "bell\\u{0007}");
        assert_eq!(visible("esc\u{1b}[31m"), "esc\\u{001B}[31m");
        assert_eq!(visible("tag\u{E0041}"), "tag\\u{E0041}");
        assert_eq!(visible("iso\u{2066}x\u{2069}"), "iso\\u{2066}x\\u{2069}");
        assert_eq!(visible("line1\nline2\tx"), "line1\nline2\tx");
        assert_eq!(visible("日本語 ok"), "日本語 ok");
    }

    #[test]
    fn one_line_escapes_newlines_and_truncates() {
        assert_eq!(one_line("a\nb", 10), "a\\nb");
        assert_eq!(one_line("abcdefghij", 5), "abcd…");
        assert_eq!(one_line("abc", 5), "abc");
    }

    #[test]
    fn hosts_are_punycode() {
        let u = url::Url::parse("https://bücher.example/x").unwrap();
        assert_eq!(display_host(&u).unwrap(), "xn--bcher-kva.example");
        let u = url::Url::parse("http://Example.COM:8080/").unwrap();
        assert_eq!(origin(&u).unwrap(), "http://example.com:8080");
        let u = url::Url::parse("https://example.com:443/").unwrap();
        assert_eq!(origin(&u).unwrap(), "https://example.com");
        // Cyrillic "а" in "аpple" is not "apple".
        let u = url::Url::parse("https://\u{430}pple.com/").unwrap();
        assert_ne!(display_host(&u).unwrap(), "apple.com");
        assert!(display_host(&u).unwrap().starts_with("xn--"));
    }
}
