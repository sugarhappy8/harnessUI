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

use crate::capture::css;

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

/// The SVG namespace, the only one whose elements a cleaned picture keeps.
const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
/// Elements a cleaned picture keeps (SVG shapes, paint, filters, text, animation). Not
/// kept: `script`, `foreignObject` (which can hold HTML), `metadata` and anything else.
const SVG_ELEMENTS: &[&str] = &[
    "a",
    "animate",
    "animateMotion",
    "animateTransform",
    "circle",
    "clipPath",
    "defs",
    "desc",
    "ellipse",
    "feBlend",
    "feColorMatrix",
    "feComponentTransfer",
    "feComposite",
    "feConvolveMatrix",
    "feDiffuseLighting",
    "feDisplacementMap",
    "feDistantLight",
    "feDropShadow",
    "feFlood",
    "feFuncA",
    "feFuncB",
    "feFuncG",
    "feFuncR",
    "feGaussianBlur",
    "feImage",
    "feMerge",
    "feMergeNode",
    "feMorphology",
    "feOffset",
    "fePointLight",
    "feSpecularLighting",
    "feSpotLight",
    "feTile",
    "feTurbulence",
    "filter",
    "g",
    "image",
    "line",
    "linearGradient",
    "marker",
    "mask",
    "mpath",
    "path",
    "pattern",
    "polygon",
    "polyline",
    "radialGradient",
    "rect",
    "set",
    "stop",
    "style",
    "svg",
    "switch",
    "symbol",
    "text",
    "textPath",
    "title",
    "tspan",
    "use",
    "view",
];
/// Nesting deeper than this is not a picture anyone drew; it is left out.
const MAX_SVG_DEPTH: usize = 256;

/// An SVG picture rebuilt from what an XML parser read of it, keeping only SVG elements
/// from a fixed list (by namespace and name, so a prefix cannot disguise a `script`),
/// without event handlers and with addresses limited to fragments (`#id`) and `data:`
/// pictures (PNG, JPEG, GIF, WebP); CSS in it goes through [`css::confine`]. Comments,
/// processing instructions (`xml-stylesheet`) and the XML declaration are dropped.
/// `None` (the picture is left out) for anything that is not a well-formed SVG document,
/// and for any document type declaration (an entity defined there could hold markup).
///
/// Used through `<img>` or CSS, an SVG never runs scripts; this is for someone opening the
/// saved file by itself, and for the shapes copied into `index.html`.
///
/// [`css::confine`]: crate::capture::css::confine
pub fn neutralise_svg(text: &str) -> Option<String> {
    use quick_xml::events::Event;
    use quick_xml::name::ResolveResult;

    let mut reader = quick_xml::NsReader::from_str(text);
    reader.config_mut().check_end_names = true;
    let mut out = String::with_capacity(text.len());
    // Open elements: whether each was kept.
    let mut open: Vec<bool> = Vec::new();
    // Inside a dropped element: how deep.
    let mut skipping = 0usize;
    let mut roots = 0usize;
    // Text of the `<style>` element being read, written once it ends.
    let mut style: Option<String> = None;
    loop {
        let (ns, event) = match reader.read_resolved_event() {
            Ok((ns, e)) => (
                match ns {
                    ResolveResult::Bound(n) => Some(n.as_ref() == SVG_NS),
                    ResolveResult::Unbound => Some(true),
                    // A prefix nobody declared: not well-formed.
                    ResolveResult::Unknown(_) => None,
                },
                e,
            ),
            Err(_) => return None,
        };
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(event, Event::Empty(_));
                let is_svg = ns?;
                if skipping > 0 {
                    if !empty {
                        skipping += 1;
                    }
                    continue;
                }
                if open.is_empty() {
                    roots += 1;
                    if roots > 1 {
                        return None;
                    }
                }
                let local = e.local_name().into_inner().to_string();
                if !xml_name(e.name().into_inner()) {
                    return None;
                }
                if open.is_empty() && !(is_svg && local == "svg") {
                    return None;
                }
                if open.len() >= MAX_SVG_DEPTH {
                    return None;
                }
                let mut attrs: Vec<(String, String)> = Vec::new();
                let mut target = String::new();
                for a in e.attributes() {
                    let a = a.ok()?;
                    // Not well-formed XML: a name that is not one, or a `<` in a value.
                    if !xml_name(a.key.into_inner()) || a.value.contains('<') {
                        return None;
                    }
                    if a.key.as_namespace_binding().is_some() {
                        continue;
                    }
                    let (ans, alocal) = reader.resolver().resolve_attribute(a.key);
                    let alocal = alocal.into_inner().to_string();
                    let value = a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok()?;
                    let name = match ans {
                        ResolveResult::Unbound => alocal.clone(),
                        ResolveResult::Bound(n) if n.as_ref() == XLINK_NS => format!("xlink:{alocal}"),
                        ResolveResult::Bound(n) if n.as_ref() == XML_NS => format!("xml:{alocal}"),
                        ResolveResult::Bound(_) => continue,
                        ResolveResult::Unknown(_) => return None,
                    };
                    if alocal == "attributeName" && !name.contains(':') {
                        target = value.trim().to_ascii_lowercase();
                    }
                    if let Some(v) = svg_attribute(&name, &value) {
                        attrs.push((name, v));
                    }
                }
                let target = target.rsplit(':').next().unwrap_or("");
                let animation = matches!(local.as_str(), "animate" | "set" | "animateMotion" | "animateTransform");
                let kept = is_svg
                    && SVG_ELEMENTS.contains(&local.as_str())
                    && !(animation && (target == "href" || target.starts_with("on")));
                if !kept {
                    if !empty {
                        skipping = 1;
                    }
                    continue;
                }
                out.push('<');
                out.push_str(&local);
                if open.is_empty() {
                    out.push_str(" xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\"");
                }
                for (name, v) in &attrs {
                    out.push(' ');
                    out.push_str(name);
                    out.push_str("=\"");
                    out.push_str(&xml_escape(v, true));
                    out.push('"');
                }
                if empty {
                    out.push_str("/>");
                } else {
                    out.push('>');
                    open.push(true);
                    if local == "style" {
                        style = Some(String::new());
                    }
                }
            }
            Event::End(ref e) => {
                if skipping > 0 {
                    skipping -= 1;
                    continue;
                }
                if open.pop().is_none() {
                    return None;
                }
                let local = e.local_name().into_inner().to_string();
                if local == "style"
                    && let Some(css_text) = style.take()
                {
                    let clean = css::confine(&css_text, |_| false)?;
                    out.push_str(&xml_escape(&clean, false));
                }
                out.push_str("</");
                out.push_str(&local);
                out.push('>');
            }
            Event::Text(ref t) => {
                let content = t.xml10_content();
                if open.is_empty() {
                    if !content.trim().is_empty() {
                        return None;
                    }
                    continue;
                }
                if skipping == 0 {
                    push_text(&mut out, &mut style, &content);
                }
            }
            Event::CData(ref t) => {
                if open.is_empty() {
                    return None;
                }
                if skipping == 0 {
                    let content = t.xml10_content().to_string();
                    push_text(&mut out, &mut style, &content);
                }
            }
            Event::GeneralRef(ref r) => {
                let ch: String = if r.is_char_ref() {
                    r.resolve_char_ref().ok()??.to_string()
                } else {
                    quick_xml::escape::resolve_predefined_entity(r.as_ref())?.to_string()
                };
                if open.is_empty() {
                    return None;
                }
                if skipping == 0 {
                    push_text(&mut out, &mut style, &ch);
                }
            }
            // An entity defined in a document type could hold markup: not kept at all.
            Event::DocType(_) => return None,
            Event::Comment(_) | Event::PI(_) | Event::Decl(_) => {}
            Event::Eof => break,
        }
    }
    (roots == 1 && open.is_empty() && skipping == 0).then_some(out)
}

/// An XML name (the ASCII part of the rule; other letters are allowed as they are).
fn xml_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_alphabetic() || c == '_' || c == ':')
        && chars.all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '\u{B7}'))
}

/// Text of a kept element: escaped, or kept for the `<style>` being read.
fn push_text(out: &mut String, style: &mut Option<String>, text: &str) {
    match style {
        Some(css_text) => css_text.push_str(text),
        None => out.push_str(&xml_escape(text, false)),
    }
}

/// The value an attribute of a cleaned SVG keeps, or `None` when it goes.
fn svg_attribute(name: &str, value: &str) -> Option<String> {
    let lname = name.to_ascii_lowercase();
    if lname.starts_with("on") || lname == "xml:base" {
        return None;
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')) {
        return None;
    }
    if name == "href" || name == "xlink:href" {
        let t = css::url_text(value);
        let lower = t.to_ascii_lowercase();
        let picture = ["data:image/png", "data:image/jpeg", "data:image/gif", "data:image/webp"]
            .iter()
            .any(|p| lower.starts_with(p) && lower[p.len()..].starts_with([';', ',']));
        return (t.starts_with('#') || picture).then_some(t);
    }
    let value = if name == "style" || value.contains('(') {
        css::confine(value, |_| false)?
    } else {
        value.to_string()
    };
    (!crate::capture::inert::leaves_folder(&value)).then_some(value)
}

/// Text safe in XML (and in HTML, where the shapes are copied in): `&`, `<`, `>` and, in
/// attributes, `"` written as references.
fn xml_escape(s: &str, attribute: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            c if c.is_control() && !matches!(c, '\n' | '\t' | '\r') => {}
            c => out.push(c),
        }
    }
    out
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
        let svg = r##"<?xml version="1.0"?><?xml-stylesheet href="https://evil.example/x.css"?><!-- c --><svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" onload="alert(1)"><script>alert(2)</script><a href="javascript:alert(3)"><circle r="4" fill="#33f" onclick="x()"/></a><foreignObject><iframe src="https://evil"></iframe></foreignObject><text>ok &amp; &lt;b&gt;</text><use xlink:href="#dot"/><image href="data:image/png;base64,AAAA"/><image href="https://evil.example/i.png"/><rect style="fill:url(#g);background:url(//evil.example/b.png)" filter="url(file://evil.example/f.svg#f)"/><style><![CDATA[ .a > .b { fill: url(https://evil.example/p.svg#p) } ]]></style><animate attributeName="href" values="javascript:alert(4)"/><set attributeName="xlink:href" to="javascript:alert(5)"/><animate attributeName="opacity" values="0;1"/></svg>"##;
        let clean = neutralise_svg(svg).expect("kept");
        let lower = clean.to_ascii_lowercase();
        for gone in ["script", "onload", "onclick", "javascript", "iframe", "foreignobject", "evil", "<!--", "<?"] {
            assert!(!lower.contains(gone), "{gone}: {clean}");
        }
        assert!(clean.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\">"), "{clean}");
        assert!(clean.contains("<a><circle r=\"4\" fill=\"#33f\"/></a>"), "{clean}");
        assert!(clean.contains("<text>ok &amp; &lt;b&gt;</text>"), "{clean}");
        assert!(clean.contains("<use xlink:href=\"#dot\"/>"), "{clean}");
        assert!(clean.contains("<image href=\"data:image/png;base64,AAAA\"/><image/>"), "{clean}");
        assert!(clean.contains("<style> .a &gt; .b { fill: url(\"about:invalid\") } </style>"), "{clean}");
        assert!(clean.contains("<animate attributeName=\"opacity\" values=\"0;1\"/></svg>"), "{clean}");
        // Cleaning the clean picture changes nothing.
        assert_eq!(neutralise_svg(&clean).as_deref(), Some(clean.as_str()));
        // A plain icon keeps its shapes; one without a namespace gets the SVG one.
        let icon = r#"<svg viewBox="0 0 10 10"><path d="M0 0h10v10z"/></svg>"#;
        assert_eq!(
            neutralise_svg(icon).as_deref(),
            Some("<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" viewBox=\"0 0 10 10\"><path d=\"M0 0h10v10z\"/></svg>")
        );
        // An encoded address is read as the browser reads it, and goes.
        let enc = neutralise_svg(r#"<svg xmlns="http://www.w3.org/2000/svg"><a href="&#106;avascript:alert(1)">x</a></svg>"#).unwrap();
        assert!(!enc.contains("href"), "{enc}");
        assert!(neutralise_svg(r#"<svg><SCRIPT>alert(1)</SCRIPT></svg>"#).is_some_and(|s| !s.contains("alert")));
        // A prefix nobody declared, broken XML and not-an-SVG are left out.
        for bad in [
            r#"<svg><svg:script>alert(1)</svg:script></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><g></svg>"#,
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><script>alert(1)</script></html>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"/><svg xmlns="http://www.w3.org/2000/svg"/>"#,
            r#"text<svg xmlns="http://www.w3.org/2000/svg"/>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg">&nbsp;</svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><1 a="x"/></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><g a="<img src=x onerror=y>"/></svg>"#,
        ] {
            assert!(neutralise_svg(bad).is_none(), "{bad}");
        }
    }

    /// The pictures of the independent review (runs/C): each ran script when opened.
    #[test]
    fn disguised_scripts_in_svg_pictures_are_gone() {
        let pub_ = "http://93.184.215.14:8080";
        let c1 = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\"><rect width=\"20\" height=\"20\" fill=\"green\"/><_:script xmlns:_=\"http://www.w3.org/2000/svg\">document.documentElement.setAttribute(\"data-pwned\",\"C1\");fetch(\"{pub_}/BEACON-C1\")</_:script></svg>");
        let c2 = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\"><rect width=\"20\" height=\"20\" fill=\"blue\"/><h.x:script xmlns:h.x=\"http://www.w3.org/1999/xhtml\">document.documentElement.setAttribute(\"data-pwned\",\"C2\");fetch(\"{pub_}/BEACON-C2\")</h.x:script></svg>");
        let c3 = format!("<?xml version=\"1.0\"?><!DOCTYPE svg [<!ENTITY x \"&#60;script&#62;document.documentElement.setAttribute(&#34;data-pwned&#34;,&#34;C3&#34;);fetch(&#34;{pub_}/BEACON-C3&#34;)&#60;/script&#62;\">]><svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\"><rect width=\"20\" height=\"20\" fill=\"orange\"/>&x;</svg>");
        let c4 = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\" x=\">\" onload=\"document.documentElement.setAttribute('data-pwned','C4')\"><rect width=\"20\" height=\"20\"/></svg>".to_string();
        for (name, svg) in [("C1", &c1), ("C2", &c2), ("C4", &c4)] {
            let clean = neutralise_svg(svg).unwrap_or_else(|| panic!("{name} keeps its picture"));
            for gone in ["script", "pwned", "BEACON", "onload"] {
                assert!(!clean.contains(gone), "{name}: {gone} in {clean}");
            }
            assert!(clean.contains("<rect width=\"20\" height=\"20\""), "{name}: {clean}");
        }
        assert!(neutralise_svg(&c3).is_none(), "a document type is never kept");
    }
}
