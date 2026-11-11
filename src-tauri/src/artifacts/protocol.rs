//! The `artifact` scheme: what a preview frame loads, and the headers it comes with.
//!
//! A preview is a page the model wrote. It is shown in a frame of the main window, and
//! everything that keeps it apart from the app is decided here and in the frame element:
//!
//! - **Its own address.** Frames load `artifact://sandbox.localhost/<id>/<version>`
//!   (`http://artifact.sandbox.localhost/...` on Windows). The host is deliberately not the
//!   plain `localhost` Tauri gives a custom scheme: on Windows the app's script interface
//!   is put into every frame, and Tauri then decides by the frame's address whether its
//!   calls are the app's own. `http://artifact.localhost` counts as the app's own;
//!   `http://artifact.sandbox.localhost` does not, and no capability grants anything to
//!   an address that is not the app's own. Requests for any other host are refused.
//! - **A sandbox.** Every response carries `Content-Security-Policy: sandbox ...`, and the
//!   frame element has the same sandbox. Without `allow-same-origin` the page has an
//!   origin of its own that nothing else shares: no cookies, no storage, no reading of
//!   the app's page. Without the other keywords it cannot move the window, open another,
//!   submit a form or show a dialog.
//! - **No way out.** The same policy allows no request of any kind: `default-src 'none'`,
//!   with only inline styles and scripts, `data:` and `blob:` pictures, sounds and fonts,
//!   and two folders of this scheme for scripts (the diagram library and the page's own
//!   scripts, see [`slice`]).
//! - **Scripts only when proven.** Until the check in `isolation.rs` has passed on this
//!   system, pages are served with scripts switched off altogether.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::Mutex;

use store::ArtifactKind;
use tauri::http::header::{self, HeaderName, HeaderValue};
use tauri::http::{HeaderMap, Response, StatusCode, Uri};

/// The scheme previews load from.
pub const SCHEME: &str = "artifact";
/// The only host of that scheme that is served.
pub const HOST: &str = "sandbox.localhost";

/// The diagram library, bundled with the app: nothing is loaded from the network.
static MERMAID: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../node_modules/mermaid/dist/mermaid.min.js"
));

const BOOTSTRAP_JS: &str = include_str!("bootstrap.js");
const SVG_PAGE_JS: &str = include_str!("svg_page.js");
const MERMAID_PAGE_JS: &str = include_str!("mermaid_page.js");
const PROBE_JS: &str = include_str!("probe.js");

/// What the frame's first line starts with, before the bootstrap script. The bootstrap
/// counts on its length to report positions on the first line.
pub const DOCUMENT_START: &str = "<!doctype html><script>";
const SCRIPT_END: &str = "</script>";

/// The origin frames load from, as this platform's webview spells it. WebView2 has no
/// custom schemes: there a scheme is a host under `.localhost`.
pub fn frame_origin(windows_form: bool, https: bool) -> String {
    if windows_form {
        let scheme = if https { "https" } else { "http" };
        format!("{scheme}://{SCHEME}.{HOST}")
    } else {
        format!("{SCHEME}://{HOST}")
    }
}

/// The address of one version's preview.
pub fn frame_url(origin: &str, id: i64, version: u32) -> String {
    format!("{origin}/{id}/{version}")
}

// ---- Routes ------------------------------------------------------------------------

/// What a request asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// One version of an artifact, as a page.
    Document { id: i64, version: u32 },
    /// The diagram library.
    Library,
    /// A piece of a page's own text, as a script (see [`slice`]).
    Slice {
        token: String,
        from: usize,
        to: usize,
    },
    /// The page that checks the sandbox on this system.
    Probe { nonce: String },
    /// A request the check's page should not have been able to make.
    ProbeHit { nonce: String, what: String },
}

fn is_token(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

fn digits<T: std::str::FromStr>(text: &str, max_len: usize) -> Option<T> {
    if text.is_empty() || text.len() > max_len || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// The route of a path such as `/12/3`. Anything that is not exactly one of the known
/// shapes is `None`.
pub fn route_of_path(path: &str) -> Option<Route> {
    let parts: Vec<&str> = path.strip_prefix('/')?.split('/').collect();
    match parts.as_slice() {
        ["__lib", "mermaid.min.js"] => Some(Route::Library),
        ["__s", token, range] if is_token(token) => {
            let (from, to) = range.strip_suffix(".js")?.split_once('-')?;
            let (from, to): (usize, usize) = (digits(from, 9)?, digits(to, 9)?);
            (from < to).then(|| Route::Slice {
                token: (*token).to_owned(),
                from,
                to,
            })
        }
        ["__probe", nonce] if is_token(nonce) => Some(Route::Probe {
            nonce: (*nonce).to_owned(),
        }),
        ["__probe", nonce, "hit", what]
            if is_token(nonce)
                && (1..=12).contains(&what.len())
                && what.bytes().all(|b| b.is_ascii_lowercase()) =>
        {
            Some(Route::ProbeHit {
                nonce: (*nonce).to_owned(),
                what: (*what).to_owned(),
            })
        }
        [id, version] => {
            let id: i64 = digits(id, 18)?;
            let version: u32 = digits(version, 9)?;
            (id >= 1 && version >= 1).then_some(Route::Document { id, version })
        }
        _ => None,
    }
}

/// The route of a request, or `None` when it is not for this scheme and host.
pub fn route(uri: &Uri) -> Option<Route> {
    if uri.scheme_str() != Some(SCHEME) || uri.host() != Some(HOST) || uri.port().is_some() {
        return None;
    }
    route_of_path(uri.path())
}

/// Whether a webview may be allowed to navigate a frame to `url`: only to a page of this
/// scheme, never to one of its scripts.
///
/// The webview asks about frames on Linux and macOS. On Windows it asks about the main
/// frame only, and the main frame must never show a preview: there a preview would be
/// the page of the window, with the window's rights. So nothing is allowed on Windows.
pub fn is_frame_navigation(url: &url::Url) -> bool {
    frame_navigation_allowed(url, cfg!(windows))
}

fn frame_navigation_allowed(url: &url::Url, asks_main_frame_only: bool) -> bool {
    if asks_main_frame_only {
        return false;
    }
    url.scheme() == SCHEME
        && url.host_str() == Some(HOST)
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(
            route_of_path(url.path()),
            Some(Route::Document { .. } | Route::Probe { .. })
        )
}

/// True when the webview says the request is for the page of a whole window rather than
/// a frame. WebView2 says so (`Sec-Fetch-Dest`); WebKit sends no such header, and there
/// the bootstrap script stops a preview that finds itself outside a frame.
pub fn is_for_a_whole_window(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-dest")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("document"))
}

// ---- Headers -----------------------------------------------------------------------

/// Whether pages are served with their scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scripts {
    /// The sandbox has been checked on this system.
    On,
    /// It has not, or the check failed: nothing in a preview runs.
    Off,
}

/// The policy every response of this scheme carries.
pub fn content_security_policy(scripts: Scripts, origin: &str) -> String {
    const REST: &str = "style-src 'unsafe-inline'; img-src data: blob:; font-src data:; \
                        media-src data: blob:; connect-src 'none'; form-action 'none'; \
                        frame-src 'none'; base-uri 'none'";
    match scripts {
        Scripts::On => format!(
            "sandbox allow-scripts; default-src 'none'; \
             script-src 'unsafe-inline' {origin}/__lib/ {origin}/__s/; {REST}"
        ),
        Scripts::Off => format!("sandbox; default-src 'none'; {REST}"),
    }
}

/// Who may load a response as part of their own page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sharing {
    /// A page: nobody may load it as a picture or a script.
    Page,
    /// A script for a preview. A sandboxed page shares its origin with nobody, not even
    /// with the address it came from, so its own scripts must be marked as loadable from
    /// any origin, and readable (`Access-Control-Allow-Origin`) for errors in them to be
    /// reported in full.
    Script,
}

/// The headers of a response.
pub fn response_headers(
    content_type: &'static str,
    sharing: Sharing,
    scripts: Scripts,
    origin: &str,
) -> Vec<(HeaderName, String)> {
    let mut headers = vec![
        (header::CONTENT_TYPE, content_type.to_owned()),
        (
            header::CONTENT_SECURITY_POLICY,
            content_security_policy(scripts, origin),
        ),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
        (header::CACHE_CONTROL, "no-store".to_owned()),
        (header::REFERRER_POLICY, "no-referrer".to_owned()),
        (header::X_DNS_PREFETCH_CONTROL, "off".to_owned()),
        (
            HeaderName::from_static("cross-origin-resource-policy"),
            match sharing {
                Sharing::Page => "same-origin",
                Sharing::Script => "cross-origin",
            }
            .to_owned(),
        ),
    ];
    if sharing == Sharing::Script {
        headers.push((header::ACCESS_CONTROL_ALLOW_ORIGIN, "*".to_owned()));
    }
    headers
}

const HTML: &str = "text/html; charset=utf-8";
const JAVASCRIPT: &str = "text/javascript; charset=utf-8";
const PLAIN: &str = "text/plain; charset=utf-8";

/// A complete response.
pub fn response(
    status: StatusCode,
    content_type: &'static str,
    sharing: Sharing,
    scripts: Scripts,
    origin: &str,
    body: Vec<u8>,
) -> Response<Vec<u8>> {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    for (name, value) in response_headers(content_type, sharing, scripts, origin) {
        if let Ok(value) = HeaderValue::from_str(&value) {
            response.headers_mut().insert(name, value);
        }
    }
    response
}

/// A refusal: a line of plain text, with scripts off whatever the state.
pub fn refusal(status: StatusCode, origin: &str, text: &str) -> Response<Vec<u8>> {
    response(
        status,
        PLAIN,
        Sharing::Page,
        Scripts::Off,
        origin,
        text.as_bytes().to_vec(),
    )
}

/// The diagram library.
pub fn library(scripts: Scripts, origin: &str) -> Response<Vec<u8>> {
    response(
        StatusCode::OK,
        JAVASCRIPT,
        Sharing::Script,
        scripts,
        origin,
        MERMAID.to_vec(),
    )
}

/// A page.
pub fn page(scripts: Scripts, origin: &str, html: String) -> Response<Vec<u8>> {
    response(
        StatusCode::OK,
        HTML,
        Sharing::Page,
        scripts,
        origin,
        html.into_bytes(),
    )
}

/// A script.
pub fn script(scripts: Scripts, origin: &str, text: String) -> Response<Vec<u8>> {
    response(
        StatusCode::OK,
        JAVASCRIPT,
        Sharing::Script,
        scripts,
        origin,
        text.into_bytes(),
    )
}

// ---- Pages -------------------------------------------------------------------------

/// A page's text with every line break as `\n`, which is how a browser reads it anyway.
/// The page, the copy of it the bootstrap searches and the pieces served as scripts are
/// all cut from this one text, so their positions agree.
pub fn normalise_newlines(content: &str) -> Cow<'_, str> {
    if content.contains('\r') {
        Cow::Owned(content.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(content)
    }
}

/// Link types that make a page connect to another server without a request the page's
/// rules could stop: a browser opens the connection for the page to use later, and no
/// rule covers a connection that carries no request yet.
const CONNECTION_HINTS: [&str; 2] = ["preconnect", "dns-prefetch"];

/// The text with the `rel` of every `<link rel="preconnect">` and `<link
/// rel="dns-prefetch">` renamed to `data-rel`, so that showing the page does not open a
/// connection to the servers named there (pages a model writes very often name a font
/// service this way).
///
/// This reads tags as they are usually written. It is a courtesy to ordinary pages, not
/// a barrier: a page that wants such a connection can still make one from a script (see
/// the notes on what a preview can do in ARCHITECTURE.md).
pub fn without_connection_hints(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    // Same length, same offsets: only ASCII letters change.
    let lower = text.to_ascii_lowercase();
    let space = |i: usize| bytes[i].is_ascii_whitespace();
    let mut renames: Vec<usize> = Vec::new();
    let mut from = 0;
    while let Some(found) = lower[from..].find("<link") {
        let mut i = from + found + "<link".len();
        from = i;
        if i >= len || !(space(i) || bytes[i] == b'/') {
            continue;
        }
        loop {
            while i < len && (space(i) || bytes[i] == b'/') {
                i += 1;
            }
            if i >= len || bytes[i] == b'>' {
                break;
            }
            let name = i;
            while i < len && !space(i) && !matches!(bytes[i], b'=' | b'>' | b'/') {
                i += 1;
            }
            let name_end = i;
            if name_end == name {
                // A stray "=".
                i += 1;
                continue;
            }
            while i < len && space(i) {
                i += 1;
            }
            let mut value = (i, i);
            if i < len && bytes[i] == b'=' {
                i += 1;
                while i < len && space(i) {
                    i += 1;
                }
                if i < len && matches!(bytes[i], b'"' | b'\'') {
                    let quote = bytes[i];
                    i += 1;
                    let start = i;
                    while i < len && bytes[i] != quote {
                        i += 1;
                    }
                    value = (start, i);
                    i = (i + 1).min(len);
                } else {
                    let start = i;
                    while i < len && !space(i) && bytes[i] != b'>' {
                        i += 1;
                    }
                    value = (start, i);
                }
            }
            if &lower[name..name_end] == "rel"
                && lower[value.0..value.1]
                    .split_ascii_whitespace()
                    .any(|token| CONNECTION_HINTS.contains(&token))
            {
                renames.push(name);
            }
        }
        from = from.max(i);
    }
    if renames.is_empty() {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(len + renames.len() * 5);
    let mut copied = 0;
    for at in renames {
        out.push_str(&text[copied..at]);
        out.push_str("data-");
        copied = at;
    }
    out.push_str(&text[copied..]);
    Cow::Owned(out)
}

/// An artifact's text as it is served: line breaks as `\n`, and for pages and pictures
/// without connection hints. The page, the copy of it the bootstrap searches and the
/// pieces served as scripts are all cut from this one text.
pub fn prepared(kind: ArtifactKind, content: &str) -> String {
    let content = normalise_newlines(content);
    match kind {
        ArtifactKind::Html | ArtifactKind::Svg => without_connection_hints(&content).into_owned(),
        ArtifactKind::Markdown | ArtifactKind::Mermaid => content.into_owned(),
    }
}

/// JSON as it can stand inside a `<script>` element: no `<` (so no `</script>` and no
/// `<!--` can end or bend the element) and no raw line separators.
pub fn script_json(value: &serde_json::Value) -> String {
    let mut out = String::new();
    for c in value.to_string().chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

/// A script file as one line without comments, so it adds no line to the page it is put
/// into. The scripts in this folder are written for it: every statement ends with `;`,
/// comments are block comments, and no string or pattern contains `/*`.
pub fn one_line(script: &str) -> String {
    let mut code = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("/*") {
        code.push_str(&rest[..start]);
        rest = rest[start..]
            .find("*/")
            .map_or("", |end| &rest[start + end + 2..]);
    }
    code.push_str(rest);
    code.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn scripted(script: &str, config: &serde_json::Value) -> String {
    one_line(script).replace("__CONFIG__", &script_json(config))
}

/// What a page is told about itself.
#[derive(Debug, Clone, Copy)]
pub struct About<'a> {
    pub id: i64,
    pub version: u32,
    pub origin: &'a str,
    /// For the page's own scripts (see [`slice`]).
    pub token: &'a str,
}

fn bootstrap(about: &About<'_>, kind: ArtifactKind, source: &str) -> String {
    scripted(
        BOOTSTRAP_JS,
        &serde_json::json!({
            "id": about.id,
            "version": about.version,
            "kind": kind.as_str(),
            "origin": about.origin,
            "token": about.token,
            "source": source,
        }),
    )
}

const PAGE_STYLE: &str = "<style>html,body{margin:0;background:#fff;color:#17181c;font:14px/1.5 system-ui,sans-serif}\
body{padding:12px}svg{max-width:100%;height:auto}\
#notice{white-space:pre-wrap;font:12px/1.5 ui-monospace,Menlo,Consolas,monospace;color:#c0262d}</style>";

/// An HTML artifact with its scripts. The bootstrap is the very first thing in the page,
/// on the page's first line, so the lines of the model's text keep their numbers.
pub fn html_page(about: &About<'_>, content: &str) -> String {
    let content = prepared(ArtifactKind::Html, content);
    format!(
        "{DOCUMENT_START}{}{SCRIPT_END}{content}",
        bootstrap(about, ArtifactKind::Html, &content)
    )
}

/// An SVG artifact: a page of the app's own that reads the picture as XML, reports what
/// does not parse and shows the rest.
pub fn svg_page(about: &About<'_>, content: &str) -> String {
    let content = prepared(ArtifactKind::Svg, content);
    format!(
        "{DOCUMENT_START}{}{SCRIPT_END}<meta charset=\"utf-8\">{PAGE_STYLE}<body><script>{}{SCRIPT_END}",
        bootstrap(about, ArtifactKind::Svg, &content),
        one_line(SVG_PAGE_JS)
    )
}

/// A Mermaid artifact: a page of the app's own that loads the bundled library and draws
/// the diagram with the library's strict setting.
pub fn mermaid_page(about: &About<'_>, content: &str) -> String {
    let content = prepared(ArtifactKind::Mermaid, content);
    format!(
        "{DOCUMENT_START}{}{SCRIPT_END}<meta charset=\"utf-8\">{PAGE_STYLE}<body><div id=\"diagram\"></div>\
         <script src=\"{}/__lib/mermaid.min.js\">{SCRIPT_END}<script>{}{SCRIPT_END}",
        bootstrap(about, ArtifactKind::Mermaid, &content),
        about.origin,
        one_line(MERMAID_PAGE_JS)
    )
}

fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A page for when scripts are off: the model's HTML as it is (nothing in it runs), the
/// SVG as markup, and for a diagram its source, since drawing it needs a script.
pub fn static_page(kind: ArtifactKind, content: &str) -> String {
    let content = prepared(kind, content);
    match kind {
        ArtifactKind::Html => format!("<!doctype html>{content}"),
        ArtifactKind::Svg => {
            format!("<!doctype html><meta charset=\"utf-8\">{PAGE_STYLE}<body>{content}")
        }
        ArtifactKind::Mermaid | ArtifactKind::Markdown => format!(
            "<!doctype html><meta charset=\"utf-8\">{PAGE_STYLE}<body><p>This diagram is drawn by a script, \
             and scripts are switched off in previews on this system. Its source:</p><pre>{}</pre>",
            escape_text(&content)
        ),
    }
}

/// The page that checks the sandbox.
pub fn probe_page(nonce: &str, origin: &str) -> String {
    format!(
        "{DOCUMENT_START}{}{SCRIPT_END}",
        scripted(
            PROBE_JS,
            &serde_json::json!({ "nonce": nonce, "origin": origin })
        )
    )
}

// ---- A page's own scripts, as files --------------------------------------------------
//
// WebKit reports an error in a script written into a sandboxed page as "Script error."
// and nothing else: to it the script belongs to the page's address, which the sandboxed
// page is not allowed to read. A script loaded as a file with permission to read it is
// reported in full. So on WebKit the bootstrap turns each script of the page into such a
// file before it runs: the same text, asked for by where it is in the page.

/// The piece of `content` between two positions (counted as JavaScript counts a string:
/// UTF-16 units), with as many empty lines and spaces in front of it as the page has
/// before it, so an error in it is reported at its line and column in the page.
pub fn slice(content: &str, from: usize, to: usize) -> Option<String> {
    if from >= to {
        return None;
    }
    let (mut start, mut end) = (None, None);
    let mut units = 0;
    for (at, c) in content.char_indices() {
        if units == from {
            start = Some(at);
        }
        if units == to {
            end = Some(at);
            break;
        }
        units += c.len_utf16();
    }
    if units == to && end.is_none() {
        end = Some(content.len());
    }
    let (start, end) = (start?, end?);
    let before = &content[..start];
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let lines = before.bytes().filter(|&b| b == b'\n').count();
    let column: usize = before[line_start..].chars().map(char::len_utf16).sum();
    let mut out = String::with_capacity(lines + column + (end - start));
    out.extend(std::iter::repeat_n('\n', lines));
    out.extend(std::iter::repeat_n(' ', column));
    out.push_str(&content[start..end]);
    Some(out)
}

/// Which page a slice may be cut from. A page is given a random token when it is served,
/// and only that token opens its text: a preview cannot ask for the text of another
/// artifact, which may belong to another chat.
#[derive(Default)]
pub struct Tokens(Mutex<VecDeque<(String, i64, u32)>>);

/// Pages whose tokens are remembered. A page asks for its scripts while it loads.
const TOKENS_KEPT: usize = 64;

impl Tokens {
    /// A new token for one version's page.
    pub fn issue(&self, id: i64, version: u32) -> String {
        let token = uuid::Uuid::new_v4().simple().to_string();
        if let Ok(mut kept) = self.0.lock() {
            if kept.len() >= TOKENS_KEPT {
                kept.pop_front();
            }
            kept.push_back((token.clone(), id, version));
        }
        token
    }

    /// The page a token was issued for.
    pub fn page_of(&self, token: &str) -> Option<(i64, u32)> {
        let kept = self.0.lock().ok()?;
        kept.iter()
            .find(|(t, _, _)| t == token)
            .map(|(_, id, version)| (*id, *version))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINUX: &str = "artifact://sandbox.localhost";
    const WINDOWS: &str = "http://artifact.sandbox.localhost";

    #[test]
    fn the_origin_is_spelled_for_the_platform() {
        assert_eq!(frame_origin(false, false), LINUX);
        assert_eq!(frame_origin(false, true), LINUX);
        assert_eq!(frame_origin(true, false), WINDOWS);
        assert_eq!(
            frame_origin(true, true),
            "https://artifact.sandbox.localhost"
        );
        assert_eq!(frame_url(LINUX, 12, 3), "artifact://sandbox.localhost/12/3");
        assert_eq!(
            frame_url(WINDOWS, 12, 3),
            "http://artifact.sandbox.localhost/12/3"
        );
    }

    /// The Windows address must not be the one Tauri takes for the app's own
    /// (`http://<scheme>.localhost`): what keeps a preview from calling the app there is
    /// that its address is somebody else's.
    #[test]
    fn the_windows_address_is_not_the_one_tauri_treats_as_the_apps_own() {
        let url = url::Url::parse(&frame_url(WINDOWS, 1, 1)).unwrap();
        let domain = url.domain().unwrap();
        // Tauri 2.12 (`Webview::is_local_url`): local when the domain minus ".localhost"
        // is the name of a registered scheme.
        assert_ne!(domain.strip_suffix(".localhost"), Some(SCHEME));
        assert_eq!(domain, "artifact.sandbox.localhost");
        // wry routes a request to the scheme's handler by this prefix.
        assert!(url.as_str().starts_with("http://artifact."));
    }

    #[test]
    fn only_the_known_paths_are_routes() {
        let token = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            route_of_path("/12/3"),
            Some(Route::Document { id: 12, version: 3 })
        );
        assert_eq!(route_of_path("/__lib/mermaid.min.js"), Some(Route::Library));
        assert_eq!(
            route_of_path(&format!("/__s/{token}/10-250.js")),
            Some(Route::Slice {
                token: token.into(),
                from: 10,
                to: 250
            })
        );
        assert_eq!(
            route_of_path(&format!("/__probe/{token}")),
            Some(Route::Probe {
                nonce: token.into()
            })
        );
        assert_eq!(
            route_of_path(&format!("/__probe/{token}/hit/fetch")),
            Some(Route::ProbeHit {
                nonce: token.into(),
                what: "fetch".into()
            })
        );
        for bad in [
            "",
            "/",
            "12/3",
            "/12",
            "/12/",
            "/12/3/",
            "/12/3/4",
            "/0/1",
            "/1/0",
            "/-1/1",
            "/1/+1",
            "/1/1e3",
            "/ 1/1",
            "/1/99999999999",
            "/9999999999999999999/1",
            "/__lib/",
            "/__lib/other.js",
            "/__lib/../1/1",
            "/__s/short/1-2.js",
            &format!("/__s/{token}/2-2.js"),
            &format!("/__s/{token}/5-2.js"),
            &format!("/__s/{token}/1-2"),
            &format!("/__s/{token}/a-b.js"),
            &format!("/__probe/{token}/hit/"),
            &format!("/__probe/{token}/hit/Fetch"),
            &format!("/__probe/{token}/other/x"),
            "/__probe/nonce",
            "/index.html",
            "/%31/1",
        ] {
            assert_eq!(route_of_path(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_request_for_another_host_or_scheme_has_no_route() {
        let ok: Uri = "artifact://sandbox.localhost/1/2".parse().unwrap();
        assert_eq!(route(&ok), Some(Route::Document { id: 1, version: 2 }));
        let with_query: Uri = "artifact://sandbox.localhost/1/2?x=1".parse().unwrap();
        assert_eq!(
            route(&with_query),
            Some(Route::Document { id: 1, version: 2 })
        );
        for other in [
            // On Windows this is `http://artifact.localhost`, which Tauri takes for the
            // app's own address: nothing is served there.
            "artifact://localhost/1/2",
            "artifact://sandbox.localhost:8080/1/2",
            "artifact://evil.sandbox.localhost/1/2",
            "artifact://sandbox.localhost.evil.example/1/2",
            "http://sandbox.localhost/1/2",
            "tauri://sandbox.localhost/1/2",
        ] {
            let uri: Uri = other.parse().unwrap();
            assert_eq!(route(&uri), None, "{other}");
        }
    }

    #[test]
    fn a_frame_may_only_be_sent_to_a_page_of_this_scheme() {
        let allowed = |s: &str, windows: bool| {
            frame_navigation_allowed(&url::Url::parse(s).unwrap(), windows)
        };
        let token = "0123456789abcdef0123456789abcdef";
        assert!(allowed("artifact://sandbox.localhost/1/2", false));
        assert!(allowed(
            &format!("artifact://sandbox.localhost/__probe/{token}"),
            false
        ));
        for url in [
            "artifact://localhost/1/2",
            "artifact://sandbox.localhost/__lib/mermaid.min.js",
            &format!("artifact://sandbox.localhost/__s/{token}/1-2.js"),
            &format!("artifact://sandbox.localhost/__probe/{token}/hit/nav"),
            "artifact://user@sandbox.localhost/1/2",
            "artifact://sandbox.localhost:1/1/2",
            "http://artifact.sandbox.localhost/1/2",
            "http://artifact.localhost/1/2",
            "https://example.com/1/2",
            "http://127.0.0.1:9000/leak",
            "data:text/html,hi",
            "javascript:alert(1)",
            "file:///etc/passwd",
        ] {
            assert!(!allowed(url, false), "{url} must be refused");
        }
        // Windows asks about the main frame only, which never shows a preview.
        assert!(!allowed("artifact://sandbox.localhost/1/2", true));
        assert!(!allowed("http://artifact.sandbox.localhost/1/2", true));
    }

    #[test]
    fn a_request_for_a_whole_window_is_recognised_where_the_webview_says_so() {
        let mut headers = HeaderMap::new();
        assert!(!is_for_a_whole_window(&headers), "WebKit sends no header");
        headers.insert("sec-fetch-dest", HeaderValue::from_static("iframe"));
        assert!(!is_for_a_whole_window(&headers));
        headers.insert("sec-fetch-dest", HeaderValue::from_static("script"));
        assert!(!is_for_a_whole_window(&headers));
        headers.insert("sec-fetch-dest", HeaderValue::from_static("document"));
        assert!(is_for_a_whole_window(&headers));
        headers.insert("sec-fetch-dest", HeaderValue::from_static(" Document "));
        assert!(is_for_a_whole_window(&headers));
    }

    /// The policy, word for word. Loosening it must be a deliberate change here.
    #[test]
    fn the_policy_is_pinned() {
        assert_eq!(
            content_security_policy(Scripts::On, LINUX),
            "sandbox allow-scripts; default-src 'none'; \
             script-src 'unsafe-inline' artifact://sandbox.localhost/__lib/ artifact://sandbox.localhost/__s/; \
             style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; \
             connect-src 'none'; form-action 'none'; frame-src 'none'; base-uri 'none'"
        );
        assert_eq!(
            content_security_policy(Scripts::On, WINDOWS),
            "sandbox allow-scripts; default-src 'none'; \
             script-src 'unsafe-inline' http://artifact.sandbox.localhost/__lib/ http://artifact.sandbox.localhost/__s/; \
             style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; \
             connect-src 'none'; form-action 'none'; frame-src 'none'; base-uri 'none'"
        );
        assert_eq!(
            content_security_policy(Scripts::Off, LINUX),
            "sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data: blob:; \
             font-src data:; media-src data: blob:; connect-src 'none'; form-action 'none'; \
             frame-src 'none'; base-uri 'none'"
        );
    }

    #[test]
    fn the_sandbox_never_grants_more_than_scripts() {
        for scripts in [Scripts::On, Scripts::Off] {
            for origin in [LINUX, WINDOWS] {
                let policy = content_security_policy(scripts, origin);
                let sandbox = policy.split(';').next().unwrap().trim();
                assert!(
                    sandbox == "sandbox" || sandbox == "sandbox allow-scripts",
                    "{sandbox}"
                );
                for word in [
                    "allow-same-origin",
                    "allow-top-navigation",
                    "allow-popups",
                    "allow-forms",
                    "allow-modals",
                    "allow-downloads",
                    "unsafe-eval",
                    "http:",
                    "https:",
                    "ws:",
                    "*",
                ] {
                    let rest = policy.replace(origin, "");
                    assert!(!rest.contains(word), "{word} in {policy}");
                }
            }
        }
    }

    #[test]
    fn every_response_carries_the_policy_and_the_fixed_headers() {
        let token = "0".repeat(32);
        let about = About {
            id: 1,
            version: 1,
            origin: LINUX,
            token: &token,
        };
        let responses = [
            (
                "page",
                page(Scripts::On, LINUX, html_page(&about, "<p>x</p>")),
            ),
            ("static page", page(Scripts::Off, LINUX, "x".into())),
            ("library", library(Scripts::On, LINUX)),
            ("slice", script(Scripts::On, LINUX, "x".into())),
            (
                "refusal",
                refusal(StatusCode::NOT_FOUND, LINUX, "Nothing here."),
            ),
        ];
        for (what, response) in &responses {
            let get = |name: &str| {
                response
                    .headers()
                    .get(name)
                    .map(|v| v.to_str().unwrap().to_owned())
            };
            let policy = get("content-security-policy").unwrap_or_default();
            assert!(policy.starts_with("sandbox"), "{what}: {policy}");
            assert!(policy.contains("default-src 'none'"), "{what}");
            assert_eq!(
                get("x-content-type-options").as_deref(),
                Some("nosniff"),
                "{what}"
            );
            assert_eq!(get("cache-control").as_deref(), Some("no-store"), "{what}");
            assert_eq!(
                get("referrer-policy").as_deref(),
                Some("no-referrer"),
                "{what}"
            );
            assert_eq!(
                get("x-dns-prefetch-control").as_deref(),
                Some("off"),
                "{what}"
            );
            assert!(get("content-type").is_some(), "{what}");
            assert!(get("cross-origin-resource-policy").is_some(), "{what}");
        }
        let corp = |i: usize| {
            responses[i]
                .1
                .headers()
                .get("cross-origin-resource-policy")
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned()
        };
        let cors = |i: usize| {
            responses[i]
                .1
                .headers()
                .get("access-control-allow-origin")
                .is_some()
        };
        // Pages: nobody may load them as a part of another page, and nobody may read them.
        assert_eq!((corp(0).as_str(), cors(0)), ("same-origin", false));
        assert_eq!((corp(1).as_str(), cors(1)), ("same-origin", false));
        assert_eq!((corp(4).as_str(), cors(4)), ("same-origin", false));
        // Scripts: a sandboxed page has an origin of its own, so they must be shareable.
        assert_eq!((corp(2).as_str(), cors(2)), ("cross-origin", true));
        assert_eq!((corp(3).as_str(), cors(3)), ("cross-origin", true));
        // A refusal never runs scripts, whatever the state.
        let policy = responses[4]
            .1
            .headers()
            .get("content-security-policy")
            .unwrap();
        assert!(!policy.to_str().unwrap().contains("allow-scripts"));
        assert_eq!(responses[4].1.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn connection_hints_are_taken_out_of_link_tags_and_nothing_else_changes() {
        let changed = [
            (
                "<link rel=\"preconnect\" href=\"https://fonts.example.com\">",
                "<link data-rel=\"preconnect\" href=\"https://fonts.example.com\">",
            ),
            (
                "<LINK REL=Preconnect HREF=x>",
                "<LINK data-REL=Preconnect HREF=x>",
            ),
            (
                "<link href=x rel='dns-prefetch preconnect' crossorigin>",
                "<link href=x data-rel='dns-prefetch preconnect' crossorigin>",
            ),
            (
                "<link\n  rel = \"preconnect\"\n  href=x\n/>",
                "<link\n  data-rel = \"preconnect\"\n  href=x\n/>",
            ),
            ("<link/rel=preconnect>", "<link/data-rel=preconnect>"),
            (
                "é<link crossorigin rel=dns-prefetch>ü",
                "é<link crossorigin data-rel=dns-prefetch>ü",
            ),
            (
                "<link rel=preconnect href=a><p>x</p><link rel=\"stylesheet preconnect\">",
                "<link data-rel=preconnect href=a><p>x</p><link data-rel=\"stylesheet preconnect\">",
            ),
        ];
        for (before, after) in changed {
            assert_eq!(without_connection_hints(before), after, "{before}");
            // The lines stay where they are.
            assert_eq!(before.lines().count(), after.lines().count());
        }
        for same in [
            "<link rel=\"stylesheet\" href=\"x.css\">",
            "<link rel=icon href=x>",
            "<link rel=\"preload\" as=font href=x>",
            "<linker rel=\"preconnect\">",
            "<a rel=\"preconnect\" href=x>y</a>",
            "rel=preconnect, in a sentence",
            "<link title=\"rel=preconnect\" rel=icon>",
            "<link data-rel=preconnect>",
            "<link relx=preconnect>",
            "<link rel=preconnected>",
            "<p>plain</p>",
            "",
        ] {
            assert!(
                matches!(without_connection_hints(same), Cow::Borrowed(_)),
                "{same}"
            );
        }
        // Cut-off and odd tags do not trip it.
        for odd in [
            "<link",
            "<link ",
            "<link rel",
            "<link rel=",
            "<link rel=\"preconnect",
            "<link =x rel=preconnect",
            "<link rel=preconnect",
        ] {
            let _ = without_connection_hints(odd);
        }
        assert_eq!(
            without_connection_hints("<link =x rel=preconnect>"),
            "<link =x data-rel=preconnect>"
        );
        // A page is served without them; a diagram's text is left alone.
        assert_eq!(
            prepared(ArtifactKind::Html, "<link rel=preconnect href=x>\r\n<p>"),
            "<link data-rel=preconnect href=x>\n<p>"
        );
        assert_eq!(
            prepared(
                ArtifactKind::Mermaid,
                "graph TD\r\n A[<link rel=preconnect>]"
            ),
            "graph TD\n A[<link rel=preconnect>]"
        );
    }

    #[test]
    fn json_inside_a_script_cannot_end_the_script() {
        let value =
            serde_json::json!({ "s": "</script><!-- <script> & \u{2028}\u{2029} \"q\" \\ \n" });
        let text = script_json(&value);
        assert!(!text.contains('<') && !text.contains('>') && !text.contains('&'));
        assert!(!text.contains('\u{2028}') && !text.contains('\u{2029}'));
        assert!(!text.contains('\n'));
        // Still the same value.
        let back: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back, value);
    }

    #[test]
    fn a_script_file_becomes_one_line_without_comments() {
        let script = "/* a comment\n   over two lines */\n(function () {\n  var a = 1; /* inline */\n  if (a) {\n    go(a);\n  }\n})();\n";
        assert_eq!(
            one_line(script),
            "(function () { var a = 1; if (a) { go(a); } })();"
        );
    }

    /// The scripts put into pages: one line each, no comment left, nothing that could end
    /// the element they are in, and their brackets in balance (a cheap check that the
    /// comment stripper did not eat code).
    #[test]
    fn the_embedded_scripts_survive_being_put_on_one_line() {
        for (name, source) in [
            ("bootstrap.js", BOOTSTRAP_JS),
            ("svg_page.js", SVG_PAGE_JS),
            ("mermaid_page.js", MERMAID_PAGE_JS),
            ("probe.js", PROBE_JS),
        ] {
            assert!(
                !source.lines().any(|l| l.trim_start().starts_with("//")),
                "{name}: line comments would swallow the rest of the one line"
            );
            let line = one_line(source);
            assert!(!line.contains('\n'), "{name}");
            assert!(!line.contains("/*") && !line.contains("*/"), "{name}");
            assert!(!line.to_lowercase().contains("</script"), "{name}");
            assert!(!line.contains("<!--"), "{name}");
            // Brackets written with a backslash (in patterns) are not brackets.
            let mut plain = String::new();
            let mut chars = line.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    chars.next();
                } else {
                    plain.push(c);
                }
            }
            for (open, close) in [('(', ')'), ('{', '}'), ('[', ']')] {
                assert_eq!(
                    plain.matches(open).count(),
                    plain.matches(close).count(),
                    "{name}: {open}{close}"
                );
            }
            assert!(line.len() > 200, "{name} is nearly empty");
        }
        assert_eq!(BOOTSTRAP_JS.matches("__CONFIG__").count(), 1);
        assert_eq!(PROBE_JS.matches("__CONFIG__").count(), 1);
    }

    #[test]
    fn an_html_page_keeps_the_models_lines_and_starts_with_the_bootstrap() {
        let token = "ab".repeat(16);
        let about = About {
            id: 7,
            version: 2,
            origin: LINUX,
            token: &token,
        };
        let content = "<!doctype html>\r\n<html>\r\n<script>boom()</script>\r</html>";
        let page = html_page(&about, content);
        assert!(page.starts_with(DOCUMENT_START));
        // One line of the app's own, then the model's text with its own line breaks.
        let first_line = page.lines().next().unwrap();
        assert!(
            first_line.ends_with("</script><!doctype html>"),
            "{}",
            &first_line[first_line.len() - 60..]
        );
        assert_eq!(page.lines().count(), 4);
        assert_eq!(page.lines().nth(2), Some("<script>boom()</script>"));
        assert!(!page.contains('\r'));
        // The page is told who it is, and is given its own text to find its scripts in.
        assert!(page.contains("\"id\":7") && page.contains("\"version\":2"));
        assert!(page.contains(&format!("\"token\":\"{token}\"")));
        assert!(page.contains("boom()\\u003c/script\\u003e"));
        // The model's text cannot close the bootstrap's script element early.
        let boot_end = page.find(SCRIPT_END).unwrap();
        assert_eq!(
            &page[boot_end + SCRIPT_END.len()..],
            normalise_newlines(content)
        );
    }

    #[test]
    fn the_apps_own_pages_hold_the_content_as_data_only() {
        let token = "ab".repeat(16);
        let about = About {
            id: 1,
            version: 1,
            origin: WINDOWS,
            token: &token,
        };
        let hostile = "</script><script>window.pwned = 1</script><img src=x onerror=1>";
        for page in [svg_page(&about, hostile), mermaid_page(&about, hostile)] {
            assert!(!page.contains("window.pwned = 1</script>"));
            assert!(!page.contains("<img src=x"));
            assert!(page.contains("\\u003c/script\\u003e\\u003cscript\\u003ewindow.pwned"));
        }
        let mermaid = mermaid_page(&about, "graph TD");
        assert!(mermaid.contains(
            "<script src=\"http://artifact.sandbox.localhost/__lib/mermaid.min.js\"></script>"
        ));
        assert!(mermaid.contains("securityLevel"));
    }

    #[test]
    fn with_scripts_off_pages_carry_no_script_of_the_apps() {
        let html = static_page(ArtifactKind::Html, "<p>hi</p>\r\n<script>x()</script>");
        assert_eq!(html, "<!doctype html><p>hi</p>\n<script>x()</script>");
        let svg = static_page(ArtifactKind::Svg, "<svg></svg>");
        assert!(svg.ends_with("<body><svg></svg>") && !svg.contains("<script"));
        let diagram = static_page(ArtifactKind::Mermaid, "graph TD\n  A --> B<script>");
        assert!(diagram.contains("A --&gt; B&lt;script&gt;"));
        assert!(!diagram.contains("<script"));
    }

    #[test]
    fn the_check_page_is_told_its_nonce_and_origin() {
        let nonce = "cd".repeat(16);
        let page = probe_page(&nonce, LINUX);
        assert!(page.starts_with(DOCUMENT_START) && page.ends_with(SCRIPT_END));
        assert!(page.contains(&format!("\"nonce\":\"{nonce}\"")));
        assert!(page.contains("artifact_canary"));
        assert_eq!(page.lines().count(), 1);
    }

    #[test]
    fn a_slice_sits_at_its_place_in_the_page() {
        let content = "<p>é</p>\n<script>\n  a();\n</script>\n<b>😀</b><script>b()</script>";
        let units = |needle: &str| {
            let at = content.find(needle).unwrap();
            content[..at].encode_utf16().count()
        };
        // The first script: from the end of its start tag to the start of its end tag.
        let from = units("\n  a();");
        let to = units("</script>\n<b>");
        let piece = slice(content, from, to).unwrap();
        assert_eq!(piece, "\n        \n  a();\n");
        // Its code is on the page's line 3, where an error will be reported.
        assert_eq!(piece.lines().nth(2), Some("  a();"));
        assert_eq!(content.lines().nth(2), Some("  a();"));
        // The second one starts mid-line, after a character that takes two units.
        let from = units("b()");
        let piece = slice(content, from, from + 3).unwrap();
        assert_eq!(piece.lines().count(), 5);
        let last = piece.lines().last().unwrap();
        assert_eq!(last.trim_start(), "b()");
        assert_eq!(last.len() - 3, "<b>😀</b><script>".encode_utf16().count());
        // The whole text, and ranges that are not there.
        let all = content.encode_utf16().count();
        assert_eq!(slice(content, 0, all).as_deref(), Some(content));
        assert_eq!(slice(content, 0, all + 1), None);
        assert_eq!(slice(content, 5, 5), None);
        assert_eq!(slice(content, 9, 3), None);
        // A position inside a two-unit character is not a position.
        let inside = units("😀") + 1;
        assert_eq!(slice(content, inside, inside + 2), None);
    }

    #[test]
    fn a_token_opens_only_the_page_it_was_issued_for() {
        let tokens = Tokens::default();
        let a = tokens.issue(1, 1);
        let b = tokens.issue(2, 5);
        assert_ne!(a, b);
        assert!(is_token(&a) && is_token(&b));
        assert_eq!(tokens.page_of(&a), Some((1, 1)));
        assert_eq!(tokens.page_of(&b), Some((2, 5)));
        assert_eq!(tokens.page_of(&"0".repeat(32)), None);
        // Old tokens are forgotten.
        for _ in 0..TOKENS_KEPT {
            tokens.issue(9, 9);
        }
        assert_eq!(tokens.page_of(&a), None);
    }

    #[test]
    fn the_library_is_bundled_and_served_as_a_script() {
        assert!(MERMAID.len() > 1_000_000, "the diagram library is missing");
        let response = library(Scripts::On, LINUX);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(response.body().len(), MERMAID.len());
        // It can be put after other scripts without ending anything.
        assert!(!String::from_utf8_lossy(MERMAID).contains("</script"));
    }
}
