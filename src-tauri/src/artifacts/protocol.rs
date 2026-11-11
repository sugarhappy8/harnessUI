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
//! - **Only the model's page runs the model's scripts.** A picture or a diagram is drawn
//!   by a page of the app's own, served with a policy under which only the app's scripts
//!   run ([`Scripts::AppOnly`]). An HTML page runs its own scripts ([`Scripts::On`]),
//!   unless it was written after content from outside the conversation entered the chat:
//!   then it is shown without them until the user allows them ([`running`]).
//! - **Only into a frame the app just made.** A page is served once, for a ticket the app
//!   issued a moment before it created the frame ([`Tickets`]). An address that was
//!   written down anywhere (in a chat, by a page about itself) opens nothing, so a preview
//!   can never become the page of the whole window, where it would be handed the app's own
//!   script interface.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

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

/// The address of one version's preview, for the frame that holds `ticket`.
pub fn frame_url(origin: &str, id: i64, version: u32, ticket: &str) -> String {
    format!("{origin}/{id}/{version}?t={ticket}")
}

/// The ticket in a query such as `t=0123…`: exactly that form, nothing else.
pub fn ticket_of_query(query: Option<&str>) -> Option<&str> {
    query?.strip_prefix("t=").filter(|t| is_token(t))
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

/// What a webview asks to navigate to, when it is a page of this scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameNavigation {
    /// One version's page, with the ticket the address carries.
    Document {
        id: i64,
        version: u32,
        ticket: String,
        /// The address has a fragment: a move inside a page (a link to `#part`), which
        /// loads nothing when the page is the one already shown.
        within_page: bool,
    },
    /// The page of a sandbox check.
    Probe { nonce: String },
}

/// The page of this scheme a navigation to `url` asks for: a version's page with a
/// ticket, or a check's page. `None` for anything else, also for this scheme's scripts and
/// for a page's address without a ticket.
///
/// The webview asks about navigations of frames on Linux and macOS, and cannot say there
/// whether it is a frame or the window itself that navigates. So the caller
/// (`artifacts::is_frame_navigation`) lets a navigation through only while the ticket is
/// unused or the check is running: an address the app itself made a moment ago for a
/// frame it is creating. On Windows the webview asks about the window only, and nothing
/// of this scheme is allowed there.
pub fn frame_navigation(url: &url::Url) -> Option<FrameNavigation> {
    if url.scheme() != SCHEME
        || url.host_str() != Some(HOST)
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    match route_of_path(url.path())? {
        Route::Document { id, version } => Some(FrameNavigation::Document {
            id,
            version,
            ticket: ticket_of_query(url.query())?.to_owned(),
            within_page: url.fragment().is_some(),
        }),
        Route::Probe { nonce } if url.query().is_none() && url.fragment().is_none() => {
            Some(FrameNavigation::Probe { nonce })
        }
        _ => None,
    }
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

/// Which scripts a page is served with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scripts<'a> {
    /// The page's own scripts run: an HTML artifact (see [`running`] for when).
    On,
    /// Only the app's own scripts run: the ones this file writes into the page, which
    /// carry `nonce`, and the diagram library. A script or a handler in what the model
    /// wrote does not run under this policy. For pictures and diagrams.
    AppOnly { nonce: &'a str },
    /// Nothing runs.
    Off,
}

/// The policy every response of this scheme carries.
pub fn content_security_policy(scripts: Scripts<'_>, origin: &str) -> String {
    const REST: &str = "style-src 'unsafe-inline'; img-src data: blob:; font-src data:; \
                        media-src data: blob:; connect-src 'none'; form-action 'none'; \
                        frame-src 'none'; base-uri 'none'";
    match scripts {
        Scripts::On => format!(
            "sandbox allow-scripts; default-src 'none'; \
             script-src 'unsafe-inline' {origin}/__lib/ {origin}/__s/; {REST}"
        ),
        Scripts::AppOnly { nonce } => format!(
            "sandbox allow-scripts; default-src 'none'; \
             script-src 'nonce-{nonce}' {origin}/__lib/; {REST}"
        ),
        Scripts::Off => format!("sandbox; default-src 'none'; {REST}"),
    }
}

/// What runs in the preview of one version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Running {
    /// The page's own scripts: an HTML page.
    Page,
    /// Only the app's scripts, which draw a picture or a diagram.
    AppOnly,
    /// Nothing: the sandbox has not been checked on this system, or the check failed.
    NotProven,
    /// Nothing: the page was written after content from outside the conversation entered
    /// the chat, and the user has not allowed its scripts.
    Held,
}

/// What runs in the preview of a version of this kind.
///
/// A page the model writes is what the model was told to write, and after it has read a
/// file, a web page or a tool result, that may be what a stranger told it. Such a page can
/// make no request, but a script in it can still open a connection whose host name
/// carries data (see [`without_connection_hints`]). So a page written after outside
/// content (`after_outside`) is shown without its scripts until the user says otherwise
/// (`allowed`). Pictures and diagrams are drawn by the app's own scripts and are shown
/// either way.
pub fn running(kind: ArtifactKind, after_outside: bool, allowed: bool, proven: bool) -> Running {
    if !proven {
        return Running::NotProven;
    }
    match kind {
        ArtifactKind::Svg | ArtifactKind::Mermaid => Running::AppOnly,
        ArtifactKind::Html | ArtifactKind::Markdown => {
            if after_outside && !allowed {
                Running::Held
            } else {
                Running::Page
            }
        }
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
    scripts: Scripts<'_>,
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
    scripts: Scripts<'_>,
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
pub fn library(scripts: Scripts<'_>, origin: &str) -> Response<Vec<u8>> {
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
pub fn page(scripts: Scripts<'_>, origin: &str, html: String) -> Response<Vec<u8>> {
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
pub fn script(scripts: Scripts<'_>, origin: &str, text: String) -> Response<Vec<u8>> {
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
///
/// TODO(0.0.5): close this for scripts too. No rule a page is served with covers a
/// connection hint (measured on WebKitGTK: one TCP connection per hint, no request), so
/// it needs the webview itself to have no route to the network, where windows are set
/// up (`lib.rs`).
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

/// The text with every `<link>` tag renamed to `<link-off>`, for pages that are shown
/// without scripts.
///
/// Inside a preview a `<link>` can do nothing useful: its rules allow no style sheet, no
/// icon and no preload from anywhere. The one thing it can still do is hint a connection
/// (see [`without_connection_hints`]), and that hint can be spelled in more ways than a
/// reader of the text can list (`rel="pre&#99;onnect"`). A page shown without scripts has
/// no other way to reach a server, so with the tag itself gone it has none.
///
/// In HTML a tag name is the letters after `<` up to white space, `/` or `>`, written as
/// they are: no escapes. So the tag is found by its letters.
pub fn without_links(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let lower = text.to_ascii_lowercase();
    let mut out: Option<String> = None;
    let mut copied = 0;
    let mut from = 0;
    while let Some(found) = lower[from..].find("<link") {
        let end = from + found + "<link".len();
        from = end;
        let ends_the_name = match bytes.get(end) {
            None => true,
            Some(b) => b.is_ascii_whitespace() || matches!(b, b'/' | b'>'),
        };
        if ends_the_name {
            let out = out.get_or_insert_with(|| String::with_capacity(text.len() + 16));
            out.push_str(&text[copied..end]);
            out.push_str("-off");
            copied = end;
        }
    }
    match out {
        Some(mut out) => {
            out.push_str(&text[copied..]);
            Cow::Owned(out)
        }
        None => Cow::Borrowed(text),
    }
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

/// How a page of the app's own starts: like [`DOCUMENT_START`], with the mark that lets
/// the script run under [`Scripts::AppOnly`].
fn own_script(nonce: &str) -> String {
    format!("<script nonce=\"{nonce}\">")
}

/// An SVG artifact: a page of the app's own that reads the picture as XML, reports what
/// does not parse and shows the rest. Served with [`Scripts::AppOnly`] for `nonce`: a
/// script or a handler inside the picture does not run.
pub fn svg_page(about: &About<'_>, content: &str, nonce: &str) -> String {
    let content = prepared(ArtifactKind::Svg, content);
    let script = own_script(nonce);
    format!(
        "<!doctype html>{script}{}{SCRIPT_END}<meta charset=\"utf-8\">{PAGE_STYLE}<body>{script}{}{SCRIPT_END}",
        bootstrap(about, ArtifactKind::Svg, &content),
        one_line(SVG_PAGE_JS)
    )
}

/// A Mermaid artifact: a page of the app's own that loads the bundled library and draws
/// the diagram with the library's strict setting.
///
/// The library does not load where `Object.prototype` is frozen. Tauri's `freezePrototype`
/// would freeze it in every frame on WebView2, so that setting is off and the app's own
/// pages freeze it themselves (`src/lib/freeze.ts`). If the library cannot start all the
/// same, the page says the diagram could not be drawn and the tool result says it was not
/// checked.
///
/// Served with [`Scripts::AppOnly`] for `nonce`: only the app's scripts and the library
/// run, whatever the diagram's text makes the library write into the page.
pub fn mermaid_page(about: &About<'_>, content: &str, nonce: &str) -> String {
    let content = prepared(ArtifactKind::Mermaid, content);
    let script = own_script(nonce);
    format!(
        "<!doctype html>{script}{}{SCRIPT_END}<meta charset=\"utf-8\">{PAGE_STYLE}<body><div id=\"diagram\"></div>\
         <script nonce=\"{nonce}\" src=\"{}/__lib/mermaid.min.js\">{SCRIPT_END}{script}{}{SCRIPT_END}",
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
/// SVG as markup, and for a diagram its source, since drawing it needs a script. No
/// `<link>` is left in it ([`without_links`]), so it has no way at all to reach a server.
pub fn static_page(kind: ArtifactKind, content: &str) -> String {
    let content = prepared(kind, content);
    let content = without_links(&content);
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

/// Which frames may load a page.
///
/// A version's page is served for a ticket, once. The app issues a ticket when it is about
/// to create a frame for that version (`artifact_frame`), and the frame's address carries
/// it. Everything that could ask for a page without the app having decided to show it has
/// no unused ticket: an address the model wrote into a chat, an address a page read from
/// its own location, a second load of the same address. That matters most on Linux and
/// macOS, where the webview cannot tell the app whether a request comes from a frame or
/// from the window itself, and a preview loaded as the window's own page would be given
/// the app's script interface.
#[derive(Default)]
pub struct Tickets(Mutex<Kept>);

#[derive(Default)]
struct Kept {
    /// Issued and not used yet.
    unused: VecDeque<Ticket>,
    /// Used: the pages that were loaded, newest last. A link inside such a page to a part
    /// of itself is a navigation to the page's own address, which the webview asks about.
    used: VecDeque<(String, i64, u32)>,
}

struct Ticket {
    token: String,
    id: i64,
    version: u32,
    issued: Instant,
}

/// A frame loads its page right after it is created.
const TICKET_LIFETIME: Duration = Duration::from_secs(30);
/// Unused tickets remembered. The panel shows one preview and renders a few out of sight.
const TICKETS_KEPT: usize = 64;

impl Tickets {
    /// A ticket for one load of one version's page.
    pub fn issue(&self, id: i64, version: u32) -> String {
        self.issue_at(id, version, Instant::now())
    }

    /// Whether the ticket could still be used for this page. Uses nothing up.
    pub fn is_live(&self, token: &str, id: i64, version: u32) -> bool {
        self.find(token, id, version, Instant::now(), false)
    }

    /// Uses the ticket up: `true` once, for the page it was issued for, while it is fresh.
    pub fn take(&self, token: &str, id: i64, version: u32) -> bool {
        self.find(token, id, version, Instant::now(), true)
    }

    /// Whether this page was loaded with this ticket (and is among the pages remembered).
    /// Says nothing about loading it again: a used ticket loads nothing.
    pub fn was_used(&self, token: &str, id: i64, version: u32) -> bool {
        self.0.lock().is_ok_and(|kept| {
            kept.used
                .iter()
                .any(|(t, i, v)| t == token && *i == id && *v == version)
        })
    }

    fn issue_at(&self, id: i64, version: u32, now: Instant) -> String {
        let token = uuid::Uuid::new_v4().simple().to_string();
        if let Ok(mut kept) = self.0.lock() {
            kept.unused
                .retain(|t| now.saturating_duration_since(t.issued) < TICKET_LIFETIME);
            if kept.unused.len() >= TICKETS_KEPT {
                kept.unused.pop_front();
            }
            kept.unused.push_back(Ticket {
                token: token.clone(),
                id,
                version,
                issued: now,
            });
        }
        token
    }

    fn find(&self, token: &str, id: i64, version: u32, now: Instant, take: bool) -> bool {
        let Ok(mut kept) = self.0.lock() else {
            return false;
        };
        let Some(at) = kept.unused.iter().position(|t| t.token == token) else {
            return false;
        };
        let ticket = &kept.unused[at];
        let good = ticket.id == id
            && ticket.version == version
            && now.saturating_duration_since(ticket.issued) < TICKET_LIFETIME;
        if take || !good {
            // Used, or presented for another page or too late: gone either way.
            kept.unused.remove(at);
        }
        if take && good {
            if kept.used.len() >= TICKETS_KEPT {
                kept.used.pop_front();
            }
            kept.used.push_back((token.to_owned(), id, version));
        }
        good
    }
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
        let ticket = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            frame_url(LINUX, 12, 3, ticket),
            format!("artifact://sandbox.localhost/12/3?t={ticket}")
        );
        assert_eq!(
            frame_url(WINDOWS, 12, 3, ticket),
            format!("http://artifact.sandbox.localhost/12/3?t={ticket}")
        );
        // What the app writes is what it reads back.
        let url = url::Url::parse(&frame_url(LINUX, 12, 3, ticket)).unwrap();
        assert_eq!(ticket_of_query(url.query()), Some(ticket));
        for query in [
            None,
            Some(""),
            Some("t="),
            Some("t=short"),
            Some("x=0123456789abcdef0123456789abcdef"),
            Some("t=0123456789abcdef0123456789abcdef&u=1"),
            Some("t=0123456789abcdef0123456789abcdeg"),
        ] {
            assert_eq!(ticket_of_query(query), None, "{query:?}");
        }
    }

    /// The Windows address must not be the one Tauri takes for the app's own
    /// (`http://<scheme>.localhost`): what keeps a preview from calling the app there is
    /// that its address is somebody else's.
    #[test]
    fn the_windows_address_is_not_the_one_tauri_treats_as_the_apps_own() {
        let url = url::Url::parse(&frame_url(WINDOWS, 1, 1, &"ab".repeat(16))).unwrap();
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
        let asked = |s: &str| frame_navigation(&url::Url::parse(s).unwrap());
        let token = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            asked(&format!("artifact://sandbox.localhost/1/2?t={token}")),
            Some(FrameNavigation::Document {
                id: 1,
                version: 2,
                ticket: token.into(),
                within_page: false
            })
        );
        // A link to a part of the page itself.
        assert_eq!(
            asked(&format!("artifact://sandbox.localhost/1/2?t={token}#part")),
            Some(FrameNavigation::Document {
                id: 1,
                version: 2,
                ticket: token.into(),
                within_page: true
            })
        );
        assert_eq!(
            asked(&format!("artifact://sandbox.localhost/__probe/{token}")),
            Some(FrameNavigation::Probe {
                nonce: token.into()
            })
        );
        for url in [
            // A page's address without a ticket is nothing: not from a chat, not from a
            // page that read its own address after its ticket was used.
            "artifact://sandbox.localhost/1/2",
            "artifact://sandbox.localhost/1/2?t=",
            "artifact://sandbox.localhost/1/2?t=short",
            "artifact://sandbox.localhost/1/2#part",
            &format!("artifact://sandbox.localhost/__probe/{token}#part"),
            &format!("artifact://sandbox.localhost/1/2?t={token}&again=1"),
            &format!("artifact://sandbox.localhost/__probe/{token}?t={token}"),
            &format!("artifact://localhost/1/2?t={token}"),
            "artifact://sandbox.localhost/__lib/mermaid.min.js",
            &format!("artifact://sandbox.localhost/__s/{token}/1-2.js"),
            &format!("artifact://sandbox.localhost/__probe/{token}/hit/nav"),
            &format!("artifact://user@sandbox.localhost/1/2?t={token}"),
            &format!("artifact://sandbox.localhost:1/1/2?t={token}"),
            &format!("http://artifact.sandbox.localhost/1/2?t={token}"),
            &format!("http://artifact.localhost/1/2?t={token}"),
            "https://example.com/1/2",
            "http://127.0.0.1:9000/leak",
            "data:text/html,hi",
            "javascript:alert(1)",
            "file:///etc/passwd",
        ] {
            assert_eq!(asked(url), None, "{url} must be refused");
        }
    }

    #[test]
    fn a_ticket_opens_one_page_once_and_not_for_long() {
        let tickets = Tickets::default();
        let start = Instant::now();
        let ticket = tickets.issue_at(7, 2, start);
        assert!(is_token(&ticket));
        // Looking uses nothing up; the frame's navigation is asked about before its page
        // is requested.
        assert!(tickets.find(&ticket, 7, 2, start, false));
        assert!(tickets.find(&ticket, 7, 2, start, false));
        assert!(!tickets.was_used(&ticket, 7, 2), "not loaded yet");
        assert!(tickets.find(&ticket, 7, 2, start, true), "the frame's load");
        assert!(
            !tickets.find(&ticket, 7, 2, start, true),
            "the same address a second time"
        );
        assert!(!tickets.find(&ticket, 7, 2, start, false));
        // The loaded page is remembered for links to its own parts, and only it.
        assert!(tickets.was_used(&ticket, 7, 2));
        assert!(!tickets.was_used(&ticket, 7, 3) && !tickets.was_used(&"0".repeat(32), 7, 2));

        // Not for another page, and a ticket shown for the wrong page is spent.
        let other = tickets.issue_at(7, 2, start);
        assert!(!tickets.find(&other, 7, 3, start, true));
        assert!(!tickets.find(&other, 8, 2, start, true));
        assert!(!tickets.find(&other, 7, 2, start, true));

        // Not after the moment a frame needs to load.
        let late = tickets.issue_at(7, 2, start);
        let later = start + TICKET_LIFETIME;
        assert!(!tickets.find(&late, 7, 2, later, false));
        let fresh = tickets.issue_at(7, 2, start);
        assert!(tickets.find(&fresh, 7, 2, later - Duration::from_millis(1), true));

        // A ticket nobody issued, and a limited number of unused ones.
        assert!(!tickets.take(&"0".repeat(32), 7, 2));
        let first = tickets.issue_at(1, 1, start);
        for _ in 0..TICKETS_KEPT {
            tickets.issue_at(1, 1, start);
        }
        assert!(!tickets.find(&first, 1, 1, start, true));
        assert_eq!(tickets.0.lock().unwrap().unused.len(), TICKETS_KEPT);
        // Old ones are dropped when a new one is issued.
        tickets.issue_at(1, 1, later);
        assert_eq!(tickets.0.lock().unwrap().unused.len(), 1);
        // The pages remembered as loaded are limited too.
        for _ in 0..TICKETS_KEPT + 3 {
            let t = tickets.issue_at(2, 1, later);
            assert!(tickets.find(&t, 2, 1, later, true));
        }
        assert_eq!(tickets.0.lock().unwrap().used.len(), TICKETS_KEPT);
        assert!(!tickets.was_used(&ticket, 7, 2), "the oldest was forgotten");
    }

    #[test]
    fn what_runs_in_a_preview() {
        use ArtifactKind::{Html, Mermaid, Svg};
        // Nothing until the sandbox is proven, whatever else holds.
        for kind in [Html, Svg, Mermaid] {
            for (after_outside, allowed) in [(false, false), (true, false), (true, true)] {
                assert_eq!(
                    running(kind, after_outside, allowed, false),
                    Running::NotProven
                );
            }
        }
        // A page the model wrote before anything from outside entered the chat runs.
        assert_eq!(running(Html, false, false, true), Running::Page);
        // After outside content it is held until the user allows it.
        assert_eq!(running(Html, true, false, true), Running::Held);
        assert_eq!(running(Html, true, true, true), Running::Page);
        // Pictures and diagrams are drawn by the app's scripts, never by the model's.
        for kind in [Svg, Mermaid] {
            for after_outside in [false, true] {
                assert_eq!(running(kind, after_outside, false, true), Running::AppOnly);
            }
        }
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
        // A picture or a diagram: the app's scripts by their nonce, the library by its
        // folder, and no inline script or handler of anyone else's.
        assert_eq!(
            content_security_policy(Scripts::AppOnly { nonce: "N0NCE" }, WINDOWS),
            "sandbox allow-scripts; default-src 'none'; \
             script-src 'nonce-N0NCE' http://artifact.sandbox.localhost/__lib/; \
             style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; \
             connect-src 'none'; form-action 'none'; frame-src 'none'; base-uri 'none'"
        );
    }

    #[test]
    fn under_the_app_only_policy_nothing_inline_runs_without_the_nonce() {
        for origin in [LINUX, WINDOWS] {
            let policy = content_security_policy(Scripts::AppOnly { nonce: "abc" }, origin);
            let script_src = policy
                .split(';')
                .map(str::trim)
                .find(|d| d.starts_with("script-src "))
                .expect("a script-src directive");
            assert_eq!(script_src, format!("script-src 'nonce-abc' {origin}/__lib/"));
            for word in ["unsafe-inline", "unsafe-hashes", "unsafe-eval", "strict-dynamic", "/__s/"] {
                assert!(!script_src.contains(word), "{word} in {script_src}");
            }
        }
    }

    #[test]
    fn the_sandbox_never_grants_more_than_scripts() {
        for scripts in [Scripts::On, Scripts::AppOnly { nonce: "abc" }, Scripts::Off] {
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
    fn every_link_tag_is_renamed_and_nothing_else() {
        for (text, expected) in [
            ("<link rel=stylesheet href=a.css>", "<link-off rel=stylesheet href=a.css>"),
            ("<link>", "<link-off>"),
            ("<link/>", "<link-off/>"),
            ("<LiNk\nrel=x>", "<LiNk-off\nrel=x>"),
            ("a<link", "a<link-off"),
            ("<link rel=a><link rel=b>", "<link-off rel=a><link-off rel=b>"),
        ] {
            assert_eq!(without_links(text), expected, "{text}");
        }
        // Other tags and plain text stay as they are, borrowed.
        for text in [
            "<links>",
            "<linkx rel=preconnect>",
            "<link-item>",
            "a < link",
            "</link>",
            "<a href=\"#link\">link</a>",
            "<p>ünïcödé <b>ok</b></p>",
            "",
        ] {
            assert!(matches!(without_links(text), Cow::Borrowed(t) if t == text), "{text}");
        }
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
        let nonce = "cd".repeat(16);
        for page in [
            svg_page(&about, hostile, &nonce),
            mermaid_page(&about, hostile, &nonce),
        ] {
            assert!(!page.contains("window.pwned = 1</script>"));
            assert!(!page.contains("<img src=x"));
            assert!(page.contains("\\u003c/script\\u003e\\u003cscript\\u003ewindow.pwned"));
            // Every script element in the page is the app's own and carries the nonce,
            // which is what lets it run under the app-only policy.
            let scripts = page.matches("<script").count();
            let marked = page.matches(&format!("<script nonce=\"{nonce}\"")).count();
            assert!(scripts >= 2 && scripts == marked, "{scripts} scripts, {marked} marked");
        }
        let mermaid = mermaid_page(&about, "graph TD", &nonce);
        assert!(mermaid.contains(&format!(
            "<script nonce=\"{nonce}\" src=\"http://artifact.sandbox.localhost/__lib/mermaid.min.js\"></script>"
        )));
        assert!(mermaid.contains("securityLevel"));
    }

    #[test]
    fn with_scripts_off_pages_carry_no_script_of_the_apps() {
        let html = static_page(ArtifactKind::Html, "<p>hi</p>\r\n<script>x()</script>");
        assert_eq!(html, "<!doctype html><p>hi</p>\n<script>x()</script>");
        // No link element is left, however its hint is spelled.
        let hinted = static_page(
            ArtifactKind::Html,
            "<LINK rel=\"pre&#99;onnect\" href=\"https://a.example\"><p>x</p><link\trel=dns-prefetch href=//b.example>",
        );
        assert_eq!(
            hinted,
            "<!doctype html><LINK-off rel=\"pre&#99;onnect\" href=\"https://a.example\"><p>x</p><link-off\tdata-rel=dns-prefetch href=//b.example>"
        );
        let picture = static_page(
            ArtifactKind::Svg,
            "<svg><foreignObject><link rel=preconnect href=//c.example /></foreignObject></svg>",
        );
        assert!(picture.contains("<link-off data-rel=preconnect") && !picture.contains("<link "));
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
