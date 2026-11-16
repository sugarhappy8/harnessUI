//! Safe names for what a capture saves (ARCHITECTURE.md 9.3, `browser_capture_site`).
//!
//! Every file and folder name comes from the app, never as written by a page: a URL only
//! lends a short stem, cut down to lower-case ASCII letters, digits, `-`, `_` and `.`, so a
//! name can never hold a path separator, `..`, a drive letter, a Windows device name (`con`,
//! `nul`, `com1`…) or an alternate data stream, never ends in a dot or a space, and is at
//! most [`MAX_NAME`] bytes. The extension is chosen by the app from what the file is. Names
//! are compared without case, because the folder may be on a volume that ignores it.

use std::collections::HashSet;

use url::Url;

/// The longest file name written, extension included.
pub const MAX_NAME: usize = 80;
/// The longest stem taken from an address.
const MAX_STEM: usize = 56;

/// Names Windows keeps for devices, whatever the extension (`con.png` opens the console).
const WINDOWS_DEVICES: &[&str] = &[
    "con", "prn", "aux", "nul", "conin$", "conout$", "clock$", "com0", "com1", "com2", "com3",
    "com4", "com5", "com6", "com7", "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5",
    "lpt6", "lpt7", "lpt8", "lpt9",
];

/// A stem made safe: lower-case ASCII letters, digits, `-` and `_`; runs of anything else
/// become one `-`; no dots (the extension is added by the app); never empty, never a
/// Windows device name, at most [`MAX_STEM`] bytes.
pub fn safe_stem(raw: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in raw.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c);
        } else {
            dash = true;
        }
        if out.len() >= MAX_STEM {
            break;
        }
    }
    let out = out.trim_matches(|c| c == '-' || c == '_').to_string();
    let out = if out.is_empty() {
        fallback.to_string()
    } else {
        out
    };
    if WINDOWS_DEVICES.contains(&out.as_str()) {
        format!("{out}-file")
    } else {
        out
    }
}

/// The stem an address suggests: its last path segment without its extension, decoded,
/// or the host for an address without one.
pub fn stem_of_url(url: &str, fallback: &str) -> String {
    let Ok(parsed) = Url::parse(url) else {
        return safe_stem("", fallback);
    };
    let last = parsed
        .path_segments()
        .and_then(|mut s| s.rfind(|seg| !seg.is_empty()))
        .map(percent_decode)
        .unwrap_or_default();
    let base = match last.rfind('.') {
        Some(i) if i > 0 => last[..i].to_string(),
        _ => last,
    };
    if base.trim().is_empty() {
        safe_stem(parsed.host_str().unwrap_or(""), fallback)
    } else {
        safe_stem(&base, fallback)
    }
}

/// The extension an address shows, lower case, when it is a short plain one.
pub fn extension_of_url(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let last = parsed.path_segments()?.rfind(|s| !s.is_empty())?;
    let last = percent_decode(last);
    let dot = last.rfind('.')?;
    let ext = last[dot + 1..].to_ascii_lowercase();
    (!ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .then_some(ext)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// The names already used in one folder of the capture, compared without case. Hands out
/// a free name for a stem and an extension, adding `-2`, `-3`… on a collision.
#[derive(Debug, Default)]
pub struct NameBook {
    used: HashSet<String>,
}

impl NameBook {
    /// A book with some names taken already.
    pub fn with_taken(names: &[&str]) -> Self {
        let mut book = Self::default();
        for n in names {
            book.used.insert(n.to_ascii_lowercase());
        }
        book
    }

    /// A free name `stem.ext` (or `stem-N.ext`). `ext` must come from the app's own list.
    pub fn take(&mut self, stem: &str, ext: &str) -> String {
        let ext = safe_extension(ext);
        let stem = safe_stem(stem, "file");
        // Room for "-NNNNN." and the extension.
        let room = MAX_NAME.saturating_sub(ext.len() + 8);
        let stem: String = stem.chars().take(room.max(1)).collect();
        let stem = stem.trim_end_matches(['-', '_']).to_string();
        let stem = if stem.is_empty() {
            "file".to_string()
        } else {
            stem
        };
        let mut n = 1usize;
        loop {
            let name = if n == 1 {
                format!("{stem}.{ext}")
            } else {
                format!("{stem}-{n}.{ext}")
            };
            if self.used.insert(name.to_ascii_lowercase()) {
                return name;
            }
            n += 1;
        }
    }
}

/// An extension made safe: lower-case ASCII letters and digits, with dots between parts
/// (`js.txt`), at most 12 bytes; `bin` when nothing is left.
pub fn safe_extension(ext: &str) -> String {
    let parts: Vec<String> = ext
        .split('.')
        .map(|p| {
            p.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_lowercase())
                .collect::<String>()
        })
        .filter(|p| !p.is_empty())
        .collect();
    let joined = parts.join(".");
    if joined.is_empty() || joined.len() > 12 {
        "bin".into()
    } else {
        joined
    }
}

/// The name of the folder a capture goes into: the site's host (punycode, port after a
/// dash) and the local time, `example.com-2026-11-16-1530`. Without a usable host, `site`.
pub fn capture_folder_name(url: &Url, now: chrono::DateTime<chrono::Local>) -> String {
    let host = url.host_str().unwrap_or("");
    let mut host = host
        .trim_matches(|c| c == '[' || c == ']')
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    host = host.trim_matches(|c| c == '.' || c == '-').to_string();
    while host.contains("..") {
        host = host.replace("..", ".");
    }
    if host.is_empty() {
        host = "site".into();
    }
    if let Some(port) = url.port() {
        host.push_str(&format!("-{port}"));
    }
    let host: String = host.chars().take(60).collect();
    let host = host.trim_end_matches(['.', '-']).to_string();
    let first = host.split('.').next().unwrap_or("");
    let host = if WINDOWS_DEVICES.contains(&first) {
        format!("site-{host}")
    } else {
        host
    };
    format!("{host}-{}", now.format("%Y-%m-%d-%H%M%S"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_safe(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= MAX_NAME
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.".contains(c))
            && !name.starts_with('.')
            && !name.ends_with('.')
            && !name.contains("..")
            && !WINDOWS_DEVICES.contains(&name.split('.').next().unwrap_or(""))
    }

    #[test]
    fn hostile_addresses_give_plain_names() {
        let mut book = NameBook::default();
        for url in [
            "https://a.example/../../etc/passwd",
            "https://a.example/%2e%2e%2f%2e%2e%2fwindows%2fsystem32%2fcmd.exe",
            "https://a.example/..%5c..%5cboot.ini",
            "https://a.example/con.png",
            "https://a.example/NUL",
            "https://a.example/LPT1.jpg",
            "https://a.example/com9.svg",
            "https://a.example/file.txt::$DATA",
            "https://a.example/C:%5Cwindows%5Cwin.ini",
            "https://a.example/%00nul%00.png",
            "https://a.example/name%20with%20spaces%20and%20dots....png",
            "https://a.example/.htaccess",
            "https://a.example/\u{202e}gnp.exe",
            "https://a.example/%E2%80%AEgnp.exe",
            "https://a.example/日本語の画像.png",
            "https://a.example/",
            "https://a.example/a/b/c/",
            "https://a.example/?q=1",
            &format!("https://a.example/{}.png", "x".repeat(400)),
        ] {
            let stem = stem_of_url(url, "picture");
            let name = book.take(&stem, "png");
            assert!(is_safe(&name), "{url} gave {name:?}");
            assert!(!name.contains('/') && !name.contains('\\') && !name.contains(':'));
        }
    }

    #[test]
    fn device_names_and_traversal_are_refused_in_stems() {
        assert_eq!(safe_stem("CON", "f"), "con-file");
        assert_eq!(safe_stem("lpt1", "f"), "lpt1-file");
        assert_eq!(safe_stem("../..", "f"), "f");
        assert_eq!(safe_stem("..\\..\\x", "f"), "x");
        assert_eq!(safe_stem("", "picture"), "picture");
        assert_eq!(safe_stem("Hero Image (Final)", "f"), "hero-image-final");
        assert!(safe_stem(&"a".repeat(500), "f").len() <= MAX_STEM);
    }

    #[test]
    fn collisions_get_numbers_and_ignore_case() {
        let mut book = NameBook::with_taken(&["index.html"]);
        assert_eq!(book.take("logo", "png"), "logo.png");
        assert_eq!(book.take("LOGO", "png"), "logo-2.png");
        assert_eq!(book.take("logo", "PNG"), "logo-3.png");
        assert_eq!(book.take("logo", "svg"), "logo.svg");
        assert_eq!(book.take("index", "html"), "index-2.html");
        let long = "z".repeat(200);
        let a = book.take(&long, "woff2");
        let b = book.take(&long, "woff2");
        assert_ne!(a, b);
        assert!(a.len() <= MAX_NAME && b.len() <= MAX_NAME, "{a} {b}");
    }

    #[test]
    fn extensions_come_from_the_app_and_stay_plain() {
        assert_eq!(safe_extension("js.txt"), "js.txt");
        assert_eq!(safe_extension("../exe"), "exe");
        assert_eq!(safe_extension(""), "bin");
        assert_eq!(safe_extension("png%00.exe"), "png00.exe");
        assert_eq!(
            extension_of_url("https://a.example/x/Logo.SVG?v=2"),
            Some("svg".into())
        );
        assert_eq!(extension_of_url("https://a.example/x/"), None);
        assert_eq!(
            extension_of_url("https://a.example/x.toolongextension"),
            None
        );
        assert_eq!(
            stem_of_url("https://a.example/img/Hero%20Shot.webp", "p"),
            "hero-shot"
        );
        assert_eq!(
            stem_of_url("https://cdn.example.com/", "p"),
            "cdn-example-com"
        );
    }

    #[test]
    fn folder_names_are_the_host_and_the_time() {
        use chrono::TimeZone;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 11, 16, 15, 30, 12)
            .single()
            .unwrap();
        let name = |u: &str| capture_folder_name(&Url::parse(u).unwrap(), now);
        assert_eq!(
            name("https://www.example.com/a/b"),
            "www.example.com-2026-11-16-153012"
        );
        assert_eq!(
            name("http://127.0.0.1:8080/"),
            "127.0.0.1-8080-2026-11-16-153012"
        );
        assert_eq!(name("https://[::1]:9/"), "1-9-2026-11-16-153012");
        assert_eq!(
            name("https://xn--bcher-kva.example/"),
            "xn--bcher-kva.example-2026-11-16-153012"
        );
        assert_eq!(
            name("https://con.example/"),
            "site-con.example-2026-11-16-153012"
        );
    }
}
