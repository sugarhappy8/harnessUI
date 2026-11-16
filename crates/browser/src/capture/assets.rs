//! What a capture keeps of the files the page loaded, and within which limits
//! (ARCHITECTURE.md 9.3, SECURITY.md "Website capture").
//!
//! A file is kept only as one of a fixed set of kinds, each with extensions the app
//! chooses (a page cannot make the app write an `.exe`, a `.js` or an `.html` file). The
//! kind comes from the browser's own record of the response and, where that says little,
//! from the file's first bytes. Scripts are kept for reference only, as `.js.txt`, so
//! opening one never runs it (on Windows a `.js` file runs when double-clicked). SVG
//! pictures lose their scripts and event handlers, or are left out when that cannot be
//! done safely.

/// What a saved file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    StyleSheet,
    Font,
    Picture,
    Icon,
    Animation,
    Video,
    Script,
}

impl Kind {
    /// The folder of the capture it goes into.
    pub fn folder(self) -> &'static str {
        match self {
            Kind::StyleSheet => "styles",
            Kind::Script => "scripts",
            _ => "assets",
        }
    }

    /// Plain words for the report.
    pub fn words(self) -> &'static str {
        match self {
            Kind::StyleSheet => "style sheet",
            Kind::Font => "font",
            Kind::Picture => "picture",
            Kind::Icon => "icon",
            Kind::Animation => "animation file",
            Kind::Video => "video",
            Kind::Script => "script",
        }
    }

    /// The order in which files are kept when the limits are reached: what the look needs
    /// most first.
    pub fn priority(self) -> u8 {
        match self {
            Kind::StyleSheet => 0,
            Kind::Font => 1,
            Kind::Picture => 2,
            Kind::Icon => 3,
            Kind::Animation => 4,
            Kind::Video => 5,
            Kind::Script => 6,
        }
    }
}

/// The limits of one capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// Bytes for one file.
    pub per_file: u64,
    /// Bytes for everything.
    pub total: u64,
    /// Files in all.
    pub files: usize,
}

/// 25 MB for one file.
pub const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
/// 300 MB for the whole copy.
pub const MAX_TOTAL_MB: u64 = 300;
/// 2,000 files.
pub const MAX_FILES: usize = 2000;

impl Default for Caps {
    fn default() -> Self {
        Self {
            per_file: MAX_FILE_BYTES,
            total: MAX_TOTAL_MB * 1024 * 1024,
            files: MAX_FILES,
        }
    }
}

impl Caps {
    /// The default caps lowered by what the model asked for (it can never raise them).
    pub fn lowered(total_mb: Option<u64>, files: Option<usize>) -> Self {
        let d = Self::default();
        Self {
            per_file: d.per_file,
            total: total_mb
                .map(|mb| mb.clamp(1, MAX_TOTAL_MB) * 1024 * 1024)
                .unwrap_or(d.total),
            files: files.map(|f| f.clamp(1, MAX_FILES)).unwrap_or(d.files),
        }
    }
}

/// Why a file was not kept, in plain words for the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    TooLarge,
    TotalReached,
    FileCountReached,
    Blocked(String),
    NotKept,
    Partial,
    Gone,
    UnsafePicture,
    Failed(String),
}

impl SkipReason {
    pub fn words(&self, caps: &Caps) -> String {
        match self {
            SkipReason::TooLarge => format!(
                "larger than the {} limit for one file",
                megabytes(caps.per_file)
            ),
            SkipReason::TotalReached => {
                format!(
                    "the {} limit for the whole copy was reached",
                    megabytes(caps.total)
                )
            }
            SkipReason::FileCountReached => {
                format!(
                    "the limit of {} files was reached",
                    thousands(caps.files as u64)
                )
            }
            SkipReason::Blocked(why) => format!("blocked by the browser's network rules ({why})"),
            SkipReason::NotKept => "not a kind of file the copy keeps".into(),
            SkipReason::Partial => {
                "the page loaded it in parts (a stream), so no whole copy exists".into()
            }
            SkipReason::Gone => "the browser no longer held it".into(),
            SkipReason::UnsafePicture => {
                "an SVG picture with scripts in it that could not be removed safely".into()
            }
            SkipReason::Failed(why) => format!("it failed to load ({why})"),
        }
    }
}

/// "25 MB", "1.4 MB", "820 KB".
pub fn megabytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if bytes >= 10 * 1024 * 1024 {
        format!("{} MB", mb.round() as u64)
    } else if bytes >= 1024 * 1024 {
        format!("{mb:.1} MB")
    } else if bytes >= 1024 {
        format!("{} KB", (bytes as f64 / 1024.0).round() as u64)
    } else {
        format!("{bytes} bytes")
    }
}

/// "2,000".
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What is left of the limits as files are kept.
#[derive(Debug, Clone)]
pub struct Budget {
    pub caps: Caps,
    pub bytes: u64,
    pub files: usize,
}

impl Budget {
    pub fn new(caps: Caps) -> Self {
        Self {
            caps,
            bytes: 0,
            files: 0,
        }
    }

    /// Takes room for one file of `size` bytes, or says why there is none.
    pub fn take(&mut self, size: u64) -> Result<(), SkipReason> {
        if size > self.caps.per_file {
            return Err(SkipReason::TooLarge);
        }
        if self.files >= self.caps.files {
            return Err(SkipReason::FileCountReached);
        }
        if self.bytes.saturating_add(size) > self.caps.total {
            return Err(SkipReason::TotalReached);
        }
        self.bytes += size;
        self.files += 1;
        Ok(())
    }

    /// Room for a file that must be written whatever happens (the page itself, the report):
    /// counted, never refused.
    pub fn force(&mut self, size: u64) {
        self.bytes = self.bytes.saturating_add(size);
        self.files += 1;
    }

    /// Gives back room taken for a file that was not written after all.
    pub fn give_back(&mut self, size: u64) {
        self.bytes = self.bytes.saturating_sub(size);
        self.files = self.files.saturating_sub(1);
    }
}

/// The kind a response becomes in the copy, from the browser's resource type and the
/// response's media type; `None` for what the copy does not keep (documents, data the page
/// asked for, beacons).
pub fn kind_of(resource_type: &str, mime: &str, url: &str) -> Option<Kind> {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let ext = crate::capture::names::extension_of_url(url).unwrap_or_default();
    match resource_type {
        "Stylesheet" => Some(Kind::StyleSheet),
        "Script" => Some(Kind::Script),
        "Font" => Some(Kind::Font),
        "Image" => Some(if is_icon(&mime, &ext) {
            Kind::Icon
        } else {
            Kind::Picture
        }),
        "Media" => mime
            .starts_with("video/")
            .then_some(Kind::Video)
            .or_else(|| mime.starts_with("audio/").then_some(Kind::Video)),
        "Fetch" | "XHR" | "Other" => {
            if mime.starts_with("image/") {
                Some(if is_icon(&mime, &ext) {
                    Kind::Icon
                } else {
                    Kind::Picture
                })
            } else if mime.starts_with("font/") || mime.contains("font-") || is_font_ext(&ext) {
                Some(Kind::Font)
            } else if mime == "application/json" || mime.ends_with("+json") || ext == "json" {
                // Only Lottie animations are kept; other data the page fetched is not part
                // of its look and may hold someone's information.
                Some(Kind::Animation)
            } else if ext == "lottie" {
                Some(Kind::Animation)
            } else if mime == "text/css" {
                Some(Kind::StyleSheet)
            } else if mime.starts_with("video/") {
                Some(Kind::Video)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_icon(mime: &str, ext: &str) -> bool {
    mime == "image/x-icon"
        || mime == "image/vnd.microsoft.icon"
        || ext == "ico"
        || mime == "image/svg+xml"
        || ext == "svg"
}

fn is_font_ext(ext: &str) -> bool {
    matches!(ext, "woff" | "woff2" | "ttf" | "otf" | "eot")
}

/// The extension a kept file gets: from its first bytes when they say what it is, else
/// from its media type, else from the address when that is one the kind allows, else a
/// plain fallback. Always one of the app's own extensions.
pub fn extension_for(kind: Kind, mime: &str, url: &str, head: &[u8]) -> &'static str {
    if kind == Kind::StyleSheet {
        return "css";
    }
    if kind == Kind::Script {
        return "js.txt";
    }
    if let Some(e) = sniff(head) {
        // A picture's bytes decide; a font that sniffs as a picture is still a picture.
        return e;
    }
    let mime = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let by_mime = match mime.as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" | "image/pjpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/avif" => Some("avif"),
        "image/svg+xml" => Some("svg"),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some("ico"),
        "image/bmp" => Some("bmp"),
        "font/woff2" | "application/font-woff2" => Some("woff2"),
        "font/woff" | "application/font-woff" | "application/x-font-woff" => Some("woff"),
        "font/ttf" | "application/x-font-ttf" | "font/sfnt" | "application/font-sfnt" => {
            Some("ttf")
        }
        "font/otf" | "application/x-font-otf" | "font/opentype" => Some("otf"),
        "application/vnd.ms-fontobject" => Some("eot"),
        "application/json" => Some("json"),
        "video/mp4" => Some("mp4"),
        "video/webm" => Some("webm"),
        "video/ogg" => Some("ogv"),
        "audio/mpeg" => Some("mp3"),
        "audio/ogg" => Some("ogg"),
        "audio/wav" | "audio/x-wav" => Some("wav"),
        _ => None,
    };
    if let Some(e) = by_mime {
        return e;
    }
    let allowed: &[&'static str] = match kind {
        Kind::Font => &["woff2", "woff", "ttf", "otf", "eot"],
        Kind::Picture | Kind::Icon => &[
            "png", "jpg", "jpeg", "gif", "webp", "avif", "svg", "ico", "bmp",
        ],
        Kind::Animation => &["json", "lottie"],
        Kind::Video => &["mp4", "webm", "ogv", "mp3", "ogg", "wav"],
        Kind::StyleSheet | Kind::Script => &[],
    };
    let from_url = crate::capture::names::extension_of_url(url).unwrap_or_default();
    if let Some(e) = allowed.iter().find(|e| **e == from_url) {
        return if *e == "jpeg" { "jpg" } else { e };
    }
    match kind {
        Kind::Font => "font",
        Kind::Animation => "json",
        Kind::Video => "media",
        _ => "img",
    }
}

/// What a file's first bytes say it is.
pub fn sniff(head: &[u8]) -> Option<&'static str> {
    let starts = |p: &[u8]| head.starts_with(p);
    if starts(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if starts(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        Some("gif")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("webp")
    } else if head.len() >= 12
        && &head[4..8] == b"ftyp"
        && (&head[8..12] == b"avif" || &head[8..12] == b"avis")
    {
        Some("avif")
    } else if starts(b"wOF2") {
        Some("woff2")
    } else if starts(b"wOFF") {
        Some("woff")
    } else if starts(b"OTTO") {
        Some("otf")
    } else if starts(&[0x00, 0x01, 0x00, 0x00]) || starts(b"true") {
        Some("ttf")
    } else if starts(&[0x00, 0x00, 0x01, 0x00]) {
        Some("ico")
    } else if starts(&[0x1A, 0x45, 0xDF, 0xA3]) {
        Some("webm")
    } else if head.len() >= 8 && &head[4..8] == b"ftyp" {
        Some("mp4")
    } else if looks_like_svg(head) {
        Some("svg")
    } else {
        None
    }
}

fn looks_like_svg(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(&head[..head.len().min(512)]).to_ascii_lowercase();
    let t = text.trim_start_matches('\u{feff}').trim_start();
    (t.starts_with("<svg") || (t.starts_with("<?xml") && t.contains("<svg")))
        || (t.starts_with("<!--") && t.contains("<svg"))
}

/// Whether a JSON file is a Lottie animation (`v`, `fr`, `ip`, `op` and `layers`).
pub fn is_lottie(bytes: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return false;
    };
    let Some(o) = v.as_object() else {
        return false;
    };
    ["v", "fr", "ip", "op"].iter().all(|k| o.contains_key(*k))
        && o.get("layers").is_some_and(serde_json::Value::is_array)
}

/// An SVG picture without what can run in it: `<script>` elements, `on…` attributes,
/// `javascript:` addresses, `<foreignObject>` (which can hold HTML) and frames. Returns
/// `None` when something that may still run is left, so the picture is not kept.
///
/// Used through `<img>` or CSS, an SVG never runs scripts; this is for someone opening the
/// saved file by itself in a browser.
pub fn neutralise_svg(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            // Copy one whole character.
            let ch_len = utf8_len(bytes[i]);
            out.push_str(&text[i..(i + ch_len).min(text.len())]);
            i += ch_len;
            continue;
        }
        // Comments, CDATA and declarations are copied as they are (they cannot run).
        if lower[i..].starts_with("<!--") {
            let end = lower[i..]
                .find("-->")
                .map(|e| i + e + 3)
                .unwrap_or(bytes.len());
            out.push_str(&text[i..end]);
            i = end;
            continue;
        }
        let rest = &lower[i..];
        let name: String = rest[1..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == ':' || *c == '-' || *c == '/')
            .collect();
        let bare = name.trim_start_matches('/');
        let bare = bare.rsplit(':').next().unwrap_or(bare);
        if matches!(
            bare,
            "script" | "foreignobject" | "iframe" | "embed" | "object" | "handler" | "listener"
        ) {
            if name.starts_with('/') {
                // A stray closing tag: drop it.
                let end = lower[i..]
                    .find('>')
                    .map(|e| i + e + 1)
                    .unwrap_or(bytes.len());
                i = end;
                continue;
            }
            // Drop the element and everything up to its end tag.
            let tag_end = lower[i..]
                .find('>')
                .map(|e| i + e + 1)
                .unwrap_or(bytes.len());
            if text[i..tag_end].trim_end_matches('>').ends_with('/') {
                i = tag_end;
                continue;
            }
            let close = format!("</{name}");
            let alt = format!("</svg:{bare}");
            let end = lower[tag_end..]
                .find(&close)
                .or_else(|| lower[tag_end..].find(&alt))
                .map(|e| {
                    let at = tag_end + e;
                    lower[at..]
                        .find('>')
                        .map(|g| at + g + 1)
                        .unwrap_or(bytes.len())
                })
                .unwrap_or(bytes.len());
            i = end;
            continue;
        }
        // An ordinary tag: copy it without event handlers and script addresses.
        let tag_end = find_tag_end(text, i).unwrap_or(bytes.len());
        out.push_str(&clean_tag(&text[i..tag_end]));
        i = tag_end;
    }
    let check = out.to_ascii_lowercase();
    let squeezed: String = check.chars().filter(|c| !c.is_whitespace()).collect();
    let risky = squeezed.contains("<script")
        || squeezed.contains("javascript:")
        || squeezed.contains("vbscript:")
        || squeezed.contains("<foreignobject")
        || has_event_attribute(&check)
        || check.contains("&#") && (squeezed.contains("href=") || squeezed.contains("values="));
    (!risky).then_some(out)
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// The end of the tag starting at `start` (just past its `>`), skipping quoted values.
fn find_tag_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = start + 1;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b'>' => return Some(i + 1),
            None => {}
        }
        i += 1;
    }
    None
}

/// A tag without `on…` attributes and without attributes whose value is a script address.
fn clean_tag(tag: &str) -> String {
    // Split into the name and the attribute run.
    let inner = tag.trim_start_matches('<');
    let closing = inner.starts_with('/') || inner.starts_with('!') || inner.starts_with('?');
    if closing {
        return tag.to_string();
    }
    let name_end = inner
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(inner.len());
    let name = &inner[..name_end];
    let mut rest = &inner[name_end..];
    let mut out = format!("<{name}");
    loop {
        let trimmed = rest.trim_start();
        let ws = &rest[..rest.len() - trimmed.len()];
        rest = trimmed;
        if rest.is_empty() || rest.starts_with('>') || rest.starts_with("/>") {
            out.push_str(rest);
            break;
        }
        let an_end = rest
            .find(|c: char| c.is_whitespace() || c == '=' || c == '>' || c == '/')
            .unwrap_or(rest.len())
            .max(1);
        let attr_name = &rest[..an_end];
        let mut after = &rest[an_end..];
        let mut value = "";
        let mut raw_value = "";
        let t = after.trim_start();
        if let Some(v) = t.strip_prefix('=') {
            let v = v.trim_start();
            let (val, consumed) = match v.as_bytes().first() {
                Some(&q) if q == b'"' || q == b'\'' => match v[1..].find(q as char) {
                    Some(e) => (&v[1..1 + e], 2 + e),
                    None => (&v[1..], v.len()),
                },
                _ => {
                    let e = v
                        .find(|c: char| c.is_whitespace() || c == '>')
                        .unwrap_or(v.len());
                    (&v[..e], e)
                }
            };
            value = val;
            let start_offset = after.len() - v.len();
            raw_value = &after[..start_offset + consumed];
            after = &after[start_offset + consumed..];
        }
        let lname = attr_name.to_ascii_lowercase();
        let squeezed: String = value
            .chars()
            .filter(|c| !c.is_whitespace() && !c.is_control())
            .collect::<String>()
            .to_ascii_lowercase();
        let drop = lname.starts_with("on")
            || squeezed.starts_with("javascript:")
            || squeezed.starts_with("vbscript:")
            || squeezed.starts_with("data:text/html");
        if !drop {
            out.push_str(ws);
            out.push_str(attr_name);
            out.push_str(raw_value);
        }
        rest = after;
    }
    out
}

fn has_event_attribute(lower: &str) -> bool {
    // ` on…=` inside a tag, after any whitespace or quote.
    let bytes = lower.as_bytes();
    let mut i = 0;
    let mut in_tag = false;
    while i + 3 < bytes.len() {
        match bytes[i] {
            b'<' => in_tag = true,
            b'>' => in_tag = false,
            c if in_tag
                && (c.is_ascii_whitespace() || c == b'"' || c == b'\'' || c == b'/')
                && bytes[i + 1] == b'o'
                && bytes[i + 2] == b'n' =>
            {
                let mut j = i + 3;
                while j < bytes.len() && bytes[j].is_ascii_alphanumeric() {
                    j += 1;
                }
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j > i + 3 && j < bytes.len() && bytes[j] == b'=' {
                    return true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_can_only_be_lowered() {
        let d = Caps::default();
        assert_eq!(d.per_file, 25 * 1024 * 1024);
        assert_eq!(d.total, 300 * 1024 * 1024);
        assert_eq!(d.files, 2000);
        let low = Caps::lowered(Some(10), Some(50));
        assert_eq!(low.total, 10 * 1024 * 1024);
        assert_eq!(low.files, 50);
        let high = Caps::lowered(Some(100_000), Some(1_000_000));
        assert_eq!(high, d, "asking for more than the limits gives the limits");
        assert_eq!(Caps::lowered(Some(0), Some(0)).files, 1);
    }

    #[test]
    fn the_budget_refuses_in_order_and_says_why() {
        let mut b = Budget::new(Caps {
            per_file: 100,
            total: 250,
            files: 3,
        });
        assert_eq!(b.take(101), Err(SkipReason::TooLarge));
        assert_eq!(b.take(100), Ok(()));
        assert_eq!(b.take(100), Ok(()));
        assert_eq!(b.take(60), Err(SkipReason::TotalReached));
        assert_eq!(b.take(50), Ok(()));
        assert_eq!(b.take(0), Err(SkipReason::FileCountReached));
        b.force(1000);
        assert_eq!(b.files, 4);
        let caps = Caps::default();
        assert_eq!(
            SkipReason::TooLarge.words(&caps),
            "larger than the 25 MB limit for one file"
        );
        assert_eq!(
            SkipReason::FileCountReached.words(&caps),
            "the limit of 2,000 files was reached"
        );
        assert_eq!(megabytes(1536), "2 KB");
        assert_eq!(megabytes(5 * 1024 * 1024 / 2), "2.5 MB");
        assert_eq!(thousands(1234567), "1,234,567");
    }

    #[test]
    fn kinds_and_extensions_come_from_the_app() {
        assert_eq!(
            kind_of("Image", "image/png", "https://a/x.png"),
            Some(Kind::Picture)
        );
        assert_eq!(
            kind_of("Image", "image/svg+xml", "https://a/i.svg"),
            Some(Kind::Icon)
        );
        assert_eq!(
            kind_of("Font", "font/woff2", "https://a/f"),
            Some(Kind::Font)
        );
        assert_eq!(kind_of("Document", "text/html", "https://a/"), None);
        assert_eq!(kind_of("Fetch", "text/html", "https://a/x"), None);
        assert_eq!(
            kind_of("XHR", "application/json", "https://a/anim.json"),
            Some(Kind::Animation)
        );
        assert_eq!(kind_of("Ping", "", "https://a/x"), None);
        assert_eq!(
            kind_of("Media", "video/mp4", "https://a/v.mp4"),
            Some(Kind::Video)
        );
        // The bytes decide over the media type, and a hostile name never gives the extension.
        let png = b"\x89PNG\r\n\x1a\n0000";
        assert_eq!(
            extension_for(Kind::Picture, "text/html", "https://a/x.exe", png),
            "png"
        );
        assert_eq!(
            extension_for(
                Kind::Picture,
                "application/octet-stream",
                "https://a/x.exe",
                b"??"
            ),
            "img"
        );
        assert_eq!(
            extension_for(Kind::Picture, "", "https://a/x.JPEG", b"??"),
            "jpg"
        );
        assert_eq!(
            extension_for(Kind::Font, "", "https://a/f.woff2?v=1", b"wOF2..."),
            "woff2"
        );
        assert_eq!(
            extension_for(Kind::Script, "text/javascript", "https://a/app.js", b""),
            "js.txt"
        );
        assert_eq!(
            extension_for(Kind::StyleSheet, "text/css", "https://a/x.css", b""),
            "css"
        );
        assert_eq!(
            extension_for(Kind::Video, "", "https://a/x.html", b"??"),
            "media"
        );
        assert_eq!(sniff(b"<?xml version=\"1.0\"?><svg"), Some("svg"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
    }

    #[test]
    fn lottie_files_are_told_apart_from_other_data() {
        assert!(is_lottie(
            br#"{"v":"5.7.4","fr":30,"ip":0,"op":60,"w":100,"h":100,"layers":[]}"#
        ));
        assert!(!is_lottie(br#"{"user":"ada","email":"ada@example.com"}"#));
        assert!(!is_lottie(b"not json"));
    }

    #[test]
    fn svg_pictures_lose_what_could_run() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"><script>alert(2)</script><a href="javascript:alert(3)"><circle r="4" fill="#33f" onclick="x()"/></a><foreignObject><iframe src="https://evil"></iframe></foreignObject><text>ok</text></svg>"##;
        let clean = neutralise_svg(svg).expect("kept");
        let lower = clean.to_ascii_lowercase();
        assert!(!lower.contains("script"), "{clean}");
        assert!(
            !lower.contains("onload") && !lower.contains("onclick"),
            "{clean}"
        );
        assert!(!lower.contains("javascript"), "{clean}");
        assert!(
            !lower.contains("iframe") && !lower.contains("foreignobject"),
            "{clean}"
        );
        assert!(clean.contains("<circle r=\"4\" fill=\"#33f\"/>"), "{clean}");
        assert!(clean.contains("<text>ok</text>"), "{clean}");
        // A plain icon is unchanged.
        let icon = r#"<svg viewBox="0 0 10 10"><path d="M0 0h10v10z"/></svg>"#;
        assert_eq!(neutralise_svg(icon).as_deref(), Some(icon));
        // Encoded tricks are not guessed at: the picture is left out.
        assert!(neutralise_svg(r#"<svg><a href="&#106;avascript:alert(1)">x</a></svg>"#).is_none());
        assert!(
            neutralise_svg(r#"<svg><SCRIPT>alert(1)</SCRIPT></svg>"#)
                .is_some_and(|s| !s.to_ascii_lowercase().contains("script"))
        );
        assert!(
            neutralise_svg(r#"<svg><svg:script>alert(1)</svg:script></svg>"#)
                .is_some_and(|s| !s.contains("alert"))
        );
    }
}
