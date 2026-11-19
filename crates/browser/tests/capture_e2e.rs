//! End-to-end test of `browser_capture_site` (0.0.7, P7) against a real Chromium in
//! headless mode and a local test site: a style sheet with `@keyframes`, a hover
//! transition, a web font, pictures (one loaded lazily, one only named for hover), a script
//! that starts a Web Animations API animation, and pictures on a private and a loopback
//! address the capture must not get.
//!
//! The copy's `index.html` is then opened from disk, offline, in a separate headless run of
//! the same browser: its first screen is compared with the original's at 1440 px, and its
//! DOM after loading shows that no script ran.
//!
//! The independent reviews' hostile pages are captured too, and each copy is opened with
//! every request recorded (by a proxy, and over the browser's debugging channel for `file:`
//! addresses): disguised markup and SVG, `file:` addresses, WebSockets, addresses passed
//! through `var()` into `image-set()`, style sheets declaring ISO-2022-JP, files that are
//! not pictures asked for as pictures, a redirect without `Location`, and workers (shared,
//! service, dedicated, and a service worker that ran before) asking a "device" on loopback
//! for its data.
//!
//! Like `browser_e2e.rs`, it needs a browser (`HARNESS_BROWSER_PATH`, an installed
//! Chrome/Edge/Chromium, or Playwright's Chromium under `/opt/pw-browsers`); without one it
//! prints a note and passes. On Windows and macOS it runs only with `HARNESS_BROWSER_E2E=1`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use browser::{BrowserConfig, BrowserServer, ConnectionMode, DenyAll};
use mcp_host::{BuiltinServer, CallContext, ChatContext, Content, Described};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

// ----- a tiny HTTP server that serves bytes ---------------------------------------------

#[derive(Clone)]
struct Page {
    /// 200, or 302 (a redirect without a `Location`, which the browser shows as a page).
    status: u16,
    mime: &'static str,
    body: Vec<u8>,
}

struct Site {
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
}

impl Site {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
    fn requests(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

async fn serve(pages: HashMap<String, Page>) -> Site {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let log2 = log.clone();
    let pages = Arc::new(pages);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let pages = pages.clone();
            let log = log2.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let Ok(n) = sock.read(&mut chunk).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let path = head
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                log.lock().unwrap().push(path.clone());
                let key = path.split('?').next().unwrap_or("/").to_string();
                let (status, mime, body) = match pages.get(&key) {
                    Some(p) if p.status == 302 => ("302 Found", p.mime, p.body.clone()),
                    Some(p) => ("200 OK", p.mime, p.body.clone()),
                    None => ("404 Not Found", "text/plain", b"not found".to_vec()),
                };
                // Any page may read every answer (as a device on the network may allow), so
                // whatever a request that got through brought back could reach the copy.
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&body).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    Site { port, log }
}

fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// A real web font when the frontend's packages are installed (KaTeX's, MIT), else bytes
/// that only look like one (the capture saves them the same way; the page then falls back).
fn font() -> Vec<u8> {
    let katex = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../node_modules/katex/dist/fonts/KaTeX_Main-Regular.woff2");
    std::fs::read(katex).unwrap_or_else(|_| b"wOF2\0\x01\0\0not a real font".to_vec())
}

fn site_pages(secret_port: u16) -> HashMap<String, Page> {
    let html = format!(
        r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Studio Example</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="refresh" content="300;url=javascript:alert(1)">
<link rel="preconnect" href="https://fonts.example">
<link rel="stylesheet" href="/css/site.css">
<style>.inline-box {{ width: 120px; height: 40px; background: url("/img/dots.png") repeat; }}</style>
<script src="/js/app.js" defer></script>
</head><body>
<header class="site-header"><nav><a href="/">Home</a> <a href="/about">About</a> <a href="javascript:alert(1)" onclick="steal()">Bad</a></nav></header>
<main>
<section class="hero"><h1 class="fade-in">Make something people love</h1><p>Hero text here.</p>
<button class="cta" onclick="alert(2)">Get started</button>
<img src="/img/hero.png" srcset="/img/hero.png 1x, /img/hero2x.png 2x" alt="hero" width="200" height="100"></section>
<section class="features"><h2>Features</h2><div class="inline-box"></div><div class="spinner"></div><div id="mover" class="mover"></div>
<svg width="40" height="40"><use href="/img/sprite.svg#dot"></use></svg></section>
<div class="spacer"></div>
<section class="gallery"><h2>Gallery</h2><img loading="lazy" src="/img/lazy.png" alt="lazy" width="200" height="100"></section>
</main>
<footer><p>Footer text</p></footer>
<img src="http://127.0.0.1:{secret_port}/secret.png" alt="loopback" width="10" height="10">
<img src="http://10.255.255.1/private.png" alt="private" width="10" height="10">
<noscript><p class="noscript">Please turn on JavaScript</p></noscript>
<script>window.inlineRan = true;</script>
</body></html>"##
    );
    let css = r#"@font-face { font-family: "Studio Sans"; src: url("../fonts/studio.woff2") format("woff2"); font-weight: 400; font-display: block; }
:root { --brand: #3355ff; --radius: 12px; }
body { margin: 0; font-family: "Studio Sans", sans-serif; color: #1a1a2e; background: #fafafa; }
.hero { padding: 64px 32px; background: #ffffff url('../img/bg.png') no-repeat right top; }
.fade-in { animation: fade-up 0.4s ease-out both; }
@keyframes fade-up { from { opacity: 0; transform: translateY(24px); } to { opacity: 1; transform: none; } }
.spinner { width: 24px; height: 24px; border: 3px solid var(--brand); animation: spin 1s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
.cta { background: var(--brand); color: #fff; border-radius: var(--radius); padding: 12px 24px; border: 0; transition: background-color 0.2s ease, transform 0.2s ease; }
.cta:hover { background-color: #2244dd; transform: translateY(-2px); background-image: url(/img/hover.png); }
.spacer { height: 2400px; background: linear-gradient(#ffffff, #eeeeff); }
@media (max-width: 600px) { .hero { padding: 32px 16px; } }
.mover { width: 40px; height: 40px; background: #ff5533; }
"#;
    let js = r#"document.body.dataset.runs = String(Number(document.body.dataset.runs || 0) + 1);
document.getElementById('mover').animate([{transform: 'translateX(0px)'}, {transform: 'translateX(100px)'}], {duration: 2000, iterations: Infinity, direction: 'alternate', easing: 'ease-in-out', id: 'slide'});
var s = document.createElement('style'); document.head.appendChild(s); s.sheet.insertRule('.css-in-js { color: rgb(1, 2, 3); }');
"#;
    let sprite = r##"<svg xmlns="http://www.w3.org/2000/svg"><symbol id="dot" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4" fill="#33f"/></symbol><script>alert(3)</script></svg>"##;
    let mut pages = HashMap::new();
    let mut add = |path: &str, mime: &'static str, body: Vec<u8>| {
        pages.insert(
            path.to_string(),
            Page {
                status: 200,
                mime,
                body,
            },
        );
    };
    add("/", "text/html; charset=utf-8", html.into_bytes());
    add("/css/site.css", "text/css", css.as_bytes().to_vec());
    add(
        "/js/app.js",
        "application/javascript",
        js.as_bytes().to_vec(),
    );
    add("/fonts/studio.woff2", "font/woff2", font());
    add("/img/hero.png", "image/png", png(200, 100, [40, 120, 200]));
    add(
        "/img/hero2x.png",
        "image/png",
        png(400, 200, [40, 120, 200]),
    );
    add("/img/lazy.png", "image/png", png(200, 100, [200, 80, 40]));
    add("/img/dots.png", "image/png", png(8, 8, [10, 200, 10]));
    add("/img/bg.png", "image/png", png(64, 64, [230, 230, 250]));
    add("/img/hover.png", "image/png", png(4, 4, [1, 1, 1]));
    add(
        "/img/sprite.svg",
        "image/svg+xml",
        sprite.as_bytes().to_vec(),
    );
    pages
}

// ----- harness -------------------------------------------------------------------------

fn have_browser() -> bool {
    if !cfg!(target_os = "linux") && std::env::var_os("HARNESS_BROWSER_E2E").is_none() {
        eprintln!(
            "skipping capture end-to-end test: set HARNESS_BROWSER_E2E=1 to run it against this computer's browser"
        );
        return false;
    }
    match browser::find_browser(None, None) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("skipping capture end-to-end test: {e}");
            false
        }
    }
}

fn server(ports: &[u16], profile: &Path) -> BrowserServer {
    server_with(ports, profile, false)
}

/// `no_policy`: the capture adds no WebSocket-blocking policy (test hook), so the line
/// behind it (stop, save nothing) is what is tested.
fn server_with(ports: &[u16], profile: &Path, no_policy: bool) -> BrowserServer {
    let mut config = BrowserConfig::new(profile.join("profile"));
    config.test_no_capture_policy = no_policy;
    config.headless = true;
    config.no_sandbox = true;
    config.connection = ConnectionMode::Pipe;
    config.navigation_timeout = Duration::from_secs(20);
    config.approval_timeout = Duration::from_secs(20);
    config.test_allowed_loopback_ports = ports.to_vec();
    BrowserServer::new(config, Arc::new(DenyAll))
}

fn chat(approved: &Path) -> ChatContext {
    ChatContext {
        chat_id: 11,
        approved_folders: vec![approved.display().to_string()],
        result_cap_tokens: 6000,
        ..Default::default()
    }
}

fn args(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

async fn describe(server: &BrowserServer, a: Value, chat: &ChatContext) -> policy::Action {
    match server
        .describe("browser_capture_site", &args(a), chat)
        .await
    {
        Described::Action(action) => action,
        Described::Invalid(why) => panic!("not describable: {why}"),
    }
}

fn png_size(path: &Path) -> (u32, u32) {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(
        &bytes[..8],
        b"\x89PNG\r\n\x1a\n",
        "{} is a PNG",
        path.display()
    );
    (
        u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
        u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
    )
}

/// Runs the browser by itself on a local file, offline, and returns what it printed.
async fn run_browser_on_file(extra: &[String], file: &Path, work: &Path) -> String {
    // Offline: every web request goes to a closed port.
    run_browser_through(extra, file, work, "127.0.0.1:9").await
}

/// Like [`run_browser_on_file`], with every request (loopback ones too) sent to `proxy`.
async fn run_browser_through(extra: &[String], file: &Path, work: &Path, proxy: &str) -> String {
    let exe = browser::find_browser(None, None).unwrap().path;
    // A profile folder of its own makes this browser build wait for ever with --dump-dom,
    // so the run uses the temporary profile headless mode makes by itself.
    let mut list: Vec<String> = vec![
        "--headless".into(),
        "--no-sandbox".into(),
        "--disable-gpu".into(),
        "--hide-scrollbars".into(),
        "--force-device-scale-factor=1".into(),
        "--no-first-run".into(),
        "--disable-background-networking".into(),
        format!("--proxy-server={proxy}"),
        "--proxy-bypass-list=<-loopback>".into(),
    ];
    list.extend(extra.iter().cloned());
    list.push(format!("file://{}", file.display()));
    let spec = runtime::spawn::SpawnSpec::new(&exe, work)
        .args(list)
        .stdout(runtime::spawn::StdioMode::Piped)
        .stderr(runtime::spawn::StdioMode::Null);
    let mut child = runtime::spawn::spawn(spec).expect("start the browser");
    let mut out = String::new();
    if let Some(mut stdout) = child.take_stdout() {
        let _ =
            tokio::time::timeout(Duration::from_secs(60), stdout.read_to_string(&mut out)).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(30), child.wait()).await;
    out
}

/// How many pixels of two pictures of the same size agree (each channel within 24).
fn match_percent(a: &Path, b: &Path) -> f64 {
    let a = image::open(a).unwrap().to_rgb8();
    let b = image::open(b).unwrap().to_rgb8();
    let (w, h) = (a.width().min(b.width()), a.height().min(b.height()));
    let mut same = 0u64;
    for y in 0..h {
        for x in 0..w {
            let p = a.get_pixel(x, y);
            let q = b.get_pixel(x, y);
            if p.0
                .iter()
                .zip(q.0.iter())
                .all(|(c, d)| c.abs_diff(*d) <= 24)
            {
                same += 1;
            }
        }
    }
    let total = u64::from(a.width().max(b.width())) * u64::from(a.height().max(b.height()));
    same as f64 * 100.0 / total as f64
}

fn only_dir(parent: &Path) -> PathBuf {
    let dirs: Vec<PathBuf> = std::fs::read_dir(parent)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    assert_eq!(dirs.len(), 1, "one capture folder: {dirs:?}");
    dirs[0].clone()
}

// ----- tests -------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_capture_saves_the_page_styles_files_animations_and_screenshots() {
    if !have_browser() {
        return;
    }
    let secret = serve(HashMap::from([(
        "/secret.png".to_string(),
        Page {
            status: 200,
            mime: "image/png",
            body: png(10, 10, [255, 0, 0]),
        },
    )]))
    .await;
    let site = serve(site_pages(secret.port)).await;
    let work = tempfile::tempdir().unwrap();
    let approved = work.path().join("approved");
    std::fs::create_dir(&approved).unwrap();
    let chat = chat(&approved);
    // Only the site's port is reachable on loopback; the "secret" server's port is not.
    let server = server(&[site.port], work.path());
    let target = approved.join("designs");

    let request = json!({"url": site.url("/"), "folder": target.display().to_string()});
    let action = describe(&server, request.clone(), &chat).await;
    assert!(action.hard_limit.is_none(), "{:?}", action.hard_limit);
    assert_eq!(action.risk, policy::Risk::Normal);
    assert!(action.outbound && !action.read_only);
    let canonical = dunce::canonicalize(&approved).unwrap().join("designs");
    assert_eq!(
        action.scope,
        policy::Scope::SiteCapture {
            origin: format!("http://127.0.0.1:{}", site.port),
            folder: canonical.display().to_string(),
        }
    );
    assert!(
        action.summary.starts_with("Save a copy of how 127.0.0.1:"),
        "{}",
        action.summary
    );
    for part in [
        "scripts switched off",
        "At most 300 MB and 2,000 files",
        "does not click, type or send forms",
        "1440 and 390 pixels wide",
    ] {
        assert!(action.preview.contains(part), "{part}:\n{}", action.preview);
    }

    let progress: Arc<Mutex<Vec<String>>> = Arc::default();
    let progress2 = progress.clone();
    let out = server
        .call(
            "browser_capture_site",
            &args(request),
            &action,
            CallContext {
                chat: chat.clone(),
                cancel: CancellationToken::new(),
                progress: Some(Arc::new(move |line: String| {
                    progress2.lock().unwrap().push(line)
                })),
            },
        )
        .await;
    let text = out.joined_text();
    assert!(!out.is_error, "the capture failed: {text}");
    assert!(
        text.starts_with("Saved a copy of the front end of 127.0.0.1:"),
        "{text}"
    );
    assert!(text.contains("## How to rebuild it"), "{text}");
    let images = out
        .content
        .iter()
        .filter(|c| matches!(c, Content::Image { mime, .. } if mime == "image/jpeg"))
        .count();
    assert_eq!(
        images, 2,
        "the first screen at each width comes with the result"
    );
    assert!(
        progress
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("Scrolling")),
        "{:?}",
        progress.lock().unwrap()
    );

    let folder = only_dir(&canonical);
    let name = folder.file_name().unwrap().to_string_lossy().to_string();
    assert!(
        name.starts_with(&format!("127.0.0.1-{}-", site.port)),
        "{name}"
    );
    for f in ["index.html", "report.md", "animations.json"] {
        assert!(folder.join(f).is_file(), "{f}");
    }

    // The network rules held during the capture: nothing reached the loopback server that
    // is not allowed, and nothing was saved from it or from the private address.
    assert!(secret.requests().is_empty(), "{:?}", secret.requests());
    let requested = site.requests();
    for path in [
        "/",
        "/css/site.css",
        "/fonts/studio.woff2",
        "/img/lazy.png",
        "/img/hover.png",
    ] {
        assert!(
            requested.iter().any(|r| r == path),
            "{path} in {requested:?}"
        );
    }

    // Files: pictures (the lazy one and the one only named for hover too), the font, the
    // sprite without its script, scripts as text only.
    let assets: Vec<String> = std::fs::read_dir(folder.join("assets"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    for want in [
        "hero.png",
        "lazy.png",
        "dots.png",
        "bg.png",
        "hover.png",
        "studio.woff2",
        "sprite.svg",
    ] {
        assert!(assets.iter().any(|a| a == want), "{want} in {assets:?}");
    }
    assert!(
        !assets
            .iter()
            .any(|a| a.contains("secret") || a.contains("private")),
        "{assets:?}"
    );
    let sprite = std::fs::read_to_string(folder.join("assets/sprite.svg")).unwrap();
    assert!(
        !sprite.contains("script") && sprite.contains("<circle"),
        "{sprite}"
    );
    let scripts: Vec<String> = std::fs::read_dir(folder.join("scripts"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(scripts.iter().any(|s| s == "app.js.txt"), "{scripts:?}");
    assert!(scripts.iter().all(|s| s.ends_with(".txt")), "{scripts:?}");

    // Style sheets: every sheet saved, addresses pointing at the saved files.
    let styles: Vec<String> = std::fs::read_dir(folder.join("styles"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(styles.iter().any(|s| s == "site.css"), "{styles:?}");
    let site_css = std::fs::read_to_string(folder.join("styles/site.css")).unwrap();
    assert!(
        site_css.contains("url(\"../assets/studio.woff2\")"),
        "{site_css}"
    );
    assert!(site_css.contains("url(\"../assets/bg.png\")"), "{site_css}");
    assert!(
        site_css.contains("url(\"../assets/hover.png\")"),
        "{site_css}"
    );
    assert!(site_css.contains("@keyframes fade-up"), "{site_css}");
    let all_css: String = styles
        .iter()
        .map(|s| std::fs::read_to_string(folder.join("styles").join(s)).unwrap())
        .collect();
    assert!(
        all_css.contains(".css-in-js"),
        "rules added by a script are kept: {styles:?}"
    );

    // The page: no scripts, no handlers, the copy's own policy first, local files.
    let index = std::fs::read_to_string(folder.join("index.html")).unwrap();
    let lower = index.to_ascii_lowercase();
    assert!(!lower.contains("<script"), "{index}");
    assert!(
        !lower.contains("onclick") && !lower.contains("javascript:"),
        "{index}"
    );
    assert!(!lower.contains("http-equiv=\"refresh\""), "{index}");
    assert!(!lower.contains("noscript"), "{index}");
    assert!(
        lower.contains("http-equiv=\"content-security-policy\""),
        "{index}"
    );
    assert!(lower.contains("script-src 'none'"), "{index}");
    assert!(index.contains("href=\"styles/site.css\""), "{index}");
    assert!(index.contains("src=\"assets/hero.png\""), "{index}");
    assert!(index.contains("src=\"assets/lazy.png\""), "{index}");
    // The sprite's shapes are copied into the page (a page opened from disk cannot use
    // another file's), without the sprite's script.
    assert!(index.contains("<use href=\"#dot\">"), "{index}");
    assert!(index.contains("<symbol id=\"dot\""), "{index}");
    assert!(index.contains("data-runs=\"1\""), "{index}");

    // animations.json: the CSS animations, the transition rule, the hover rule and the
    // script animation with its keyframes and timing.
    let doc: Value =
        serde_json::from_slice(&std::fs::read(folder.join("animations.json")).unwrap()).unwrap();
    let running = doc["running"].as_array().unwrap();
    let find = |kind: &str, name: &str| {
        running
            .iter()
            .find(|a| a["kind"] == kind && a["name"] == name)
            .unwrap_or_else(|| {
                panic!(
                    "{kind} {name} in {}",
                    serde_json::to_string_pretty(running).unwrap()
                )
            })
    };
    let fade = find("css-animation", "fade-up");
    assert!(
        fade["target"].as_str().unwrap().contains("h1.fade-in"),
        "{fade}"
    );
    assert_eq!(fade["timing"]["duration"], 400);
    assert_eq!(fade["keyframes"][0]["opacity"], "0");
    let spin = find("css-animation", "spin");
    assert_eq!(spin["timing"]["iterations"], "infinite");
    let slide = find("script", "slide");
    assert!(
        slide["target"].as_str().unwrap().contains("#mover"),
        "{slide}"
    );
    assert_eq!(slide["timing"]["duration"], 2000);
    assert_eq!(slide["timing"]["iterations"], "infinite");
    assert_eq!(slide["timing"]["direction"], "alternate");
    assert_eq!(slide["timing"]["easing"], "ease-in-out");
    assert_eq!(slide["keyframes"][1]["transform"], "translateX(100px)");
    let keyframes: Vec<&str> = doc["keyframes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|k| k["name"].as_str())
        .collect();
    assert!(
        keyframes.contains(&"fade-up") && keyframes.contains(&"spin"),
        "{keyframes:?}"
    );
    let transitions = doc["transitions"].as_array().unwrap();
    assert!(
        transitions.iter().any(|t| t["selector"] == ".cta"
            && t["transition"] == "background-color 0.2s ease, transform 0.2s ease"),
        "{transitions:?}"
    );
    let states = doc["hover_focus_rules"].as_array().unwrap();
    assert!(
        states.iter().any(|s| s["selector"] == ".cta:hover"),
        "{states:?}"
    );

    // Screenshots at both widths, with the right widths.
    for (file, width) in [
        ("w1440-first-screen.png", 1440),
        ("w1440-full-page.png", 1440),
        ("w390-first-screen.png", 390),
        ("w390-full-page.png", 390),
    ] {
        let (w, h) = png_size(&folder.join("screenshots").join(file));
        assert_eq!(w, width, "{file}");
        assert!(h > 0, "{file}");
    }
    let (_, full_h) = png_size(&folder.join("screenshots/w1440-full-page.png"));
    assert!(
        full_h > 2400,
        "the full page is taller than one screen: {full_h}"
    );

    // The report: title, fonts, animations, what was blocked, the files and the method.
    let report = std::fs::read_to_string(folder.join("report.md")).unwrap();
    for part in [
        "# Capture of \"Studio Example\"",
        "## Page outline",
        "h1 \"Make something people love\"",
        "\"Studio Sans\"",
        "assets/studio.woff2",
        "#1a1a2e",
        "--brand: #3355ff",
        "max-width: 600px",
        "fade-up",
        "blocked by the browser's network rules",
        "10.255.255.1",
        "## Files",
        "## How to rebuild it",
        "logos, brand names or sign-in pages",
    ] {
        assert!(
            report.contains(part),
            "{part} missing from the report:\n{report}"
        );
    }

    // Opened offline from disk: the same look, and no script runs.
    let copy_shot = work.path().join("copy-1440.png");
    run_browser_on_file(
        &[
            "--window-size=1440,900".into(),
            "--virtual-time-budget=3000".into(),
            format!("--screenshot={}", copy_shot.display()),
        ],
        &folder.join("index.html"),
        work.path(),
    )
    .await;
    assert!(
        copy_shot.is_file(),
        "the browser saved a screenshot of the copy"
    );
    let percent = match_percent(
        &folder.join("screenshots/w1440-first-screen.png"),
        &copy_shot,
    );
    eprintln!(
        "capture e2e: the copy's first screen matches the original's at 1440 px: {percent:.2}%"
    );
    assert!(percent >= 90.0, "the copy looks different: {percent:.2}%");
    let dom = run_browser_on_file(
        &["--virtual-time-budget=3000".into(), "--dump-dom".into()],
        &folder.join("index.html"),
        work.path(),
    )
    .await;
    assert!(dom.contains("data-runs=\"1\""), "{dom}");
    assert!(
        !dom.contains("data-runs=\"2\""),
        "a script of the page ran in the copy:\n{dom}"
    );
    // To look at what was saved: CAPTURE_E2E_KEEP=<folder> keeps a copy there.
    if let Some(keep) = std::env::var_os("CAPTURE_E2E_KEEP") {
        let keep = PathBuf::from(keep);
        copy_tree(&folder, &keep.join("capture"));
        let _ = std::fs::copy(&copy_shot, keep.join("copy-opened-offline-1440.png"));
    }
    server.shutdown().await;
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_tree(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_capture_is_refused_outside_the_approved_folders_and_for_blocked_addresses() {
    if !have_browser() {
        return;
    }
    let work = tempfile::tempdir().unwrap();
    let approved = work.path().join("approved");
    let outside = work.path().join("outside");
    std::fs::create_dir(&approved).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let chat = chat(&approved);
    let server = server(&[], work.path());
    // A public address (no lookup needed) into a folder that was not approved: a hard
    // limit, no card.
    let a = describe(
        &server,
        json!({"url": "http://93.184.215.14/", "folder": outside.display().to_string()}),
        &chat,
    )
    .await;
    let why = a.hard_limit.expect("a hard limit");
    assert!(why.contains("Approved folders:"), "{why}");
    let a = describe(
        &server,
        json!({"url": "http://93.184.215.14/", "folder": "../outside"}),
        &chat,
    )
    .await;
    assert!(a.hard_limit.is_some());
    // Private and loopback addresses are hard limits, as for browser_navigate.
    for url in [
        "http://10.0.0.1/",
        "http://127.0.0.1:9/",
        "http://localhost/",
        "file:///etc/passwd",
    ] {
        match server
            .describe("browser_capture_site", &args(json!({"url": url})), &chat)
            .await
        {
            Described::Action(a) => assert!(a.hard_limit.is_some(), "{url}"),
            Described::Invalid(_) => {}
        }
    }
    // Without a page open and without url there is nothing to capture.
    match server
        .describe("browser_capture_site", &args(json!({})), &chat)
        .await
    {
        Described::Invalid(why) => assert!(why.contains("Give url"), "{why}"),
        Described::Action(a) => panic!("{a:?}"),
    }
    // A sensitive place always asks, and keeps asking in Allow all.
    let a = describe(
        &server,
        json!({"url": "http://93.184.215.14/", "folder": approved.join(".git").display().to_string()}),
        &chat,
    )
    .await;
    assert_eq!(a.risk, policy::Risk::AlwaysAsk);
    assert_eq!(a.keep_asking, Some(policy::KeepAsking::Sensitive));
    server.shutdown().await;
}

/// A capture of a real public site, run only when `CAPTURE_E2E_URL` names one (it needs the
/// internet). The copy is checked like the local one; `CAPTURE_E2E_KEEP` keeps it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_public_site_can_be_captured_when_asked() {
    let Some(url) = std::env::var_os("CAPTURE_E2E_URL").map(|u| u.to_string_lossy().to_string())
    else {
        eprintln!("skipping the public-site capture: set CAPTURE_E2E_URL to run it");
        return;
    };
    if !have_browser() {
        return;
    }
    let work = tempfile::tempdir().unwrap();
    let approved = work.path().join("approved");
    std::fs::create_dir(&approved).unwrap();
    let chat = chat(&approved);
    let server = server(&[], work.path());
    let request = json!({"url": url});
    let action = describe(&server, request.clone(), &chat).await;
    assert!(action.hard_limit.is_none(), "{:?}", action.hard_limit);
    let started = std::time::Instant::now();
    let out = server
        .call(
            "browser_capture_site",
            &args(request),
            &action,
            CallContext {
                chat: chat.clone(),
                cancel: CancellationToken::new(),
                progress: None,
            },
        )
        .await;
    let text = out.joined_text();
    assert!(!out.is_error, "the capture failed: {text}");
    eprintln!(
        "capture of {url} took {:.1} s\n{text}",
        started.elapsed().as_secs_f64()
    );
    let folder = only_dir(&dunce::canonicalize(&approved).unwrap());
    let index = std::fs::read_to_string(folder.join("index.html")).unwrap();
    assert!(!index.to_ascii_lowercase().contains("<script"));
    for file in ["w1440-first-screen.png", "w390-first-screen.png"] {
        assert!(folder.join("screenshots").join(file).is_file(), "{file}");
    }
    if let Some(keep) = std::env::var_os("CAPTURE_E2E_KEEP") {
        copy_tree(&folder, &PathBuf::from(keep).join("public"));
    }
    server.shutdown().await;
}

// ----- hostile pages (the independent review of round 1) --------------------------------

/// The pages of the independent review: each made an earlier copy load or run something
/// when opened. `beacon` is a loopback port the capture may not reach; every address a
/// copy must never use points there or at `attacker.example`.
fn hostile_pages(beacon: u16, ws: u16) -> HashMap<String, Page> {
    use base64::Engine;
    let b = format!("http://127.0.0.1:{beacon}");
    let svg_doc = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" onload=\"fetch('{b}/BEACON-A-svgdoc-onload')\"><script>fetch(\"{b}/BEACON-A-svgdoc-script\")</script><circle r=\"40\" cx=\"50\" cy=\"50\" fill=\"red\"/></svg>"
    );
    let svg64 = base64::engine::general_purpose::STANDARD.encode(svg_doc);
    let html = |s: String| Page {
        status: 200,
        mime: "text/html; charset=utf-8",
        body: s.into_bytes(),
    };
    let svg = |s: String| Page {
        status: 200,
        mime: "image/svg+xml",
        body: s.into_bytes(),
    };
    let page_a = format!(
        r#"<!DOCTYPE html><html lang="en"><head><title>Root swap</title><style>h1{{color:#c00}}</style></head><body><h1>Live page</h1>
<script>
(function(){{
  var root = document.createElement('div');
  root.innerHTML = '<h1>Copy of the page</h1><p>Some text.</p>';
  function el(ns, name, attrs){{ var e = document.createElementNS(ns, name); for (var k in attrs) e.setAttribute(k, attrs[k]); root.appendChild(e); return e; }}
  el('urn:x', 'embed', {{src: 'data:image/svg+xml;base64,{svg64}', type: 'image/svg+xml', width: '100', height: '100'}});
  el('urn:x', 'iframe', {{src: 'data:image/svg+xml;base64,{svg64}', width: '100', height: '100'}});
  el('urn:x', 'img', {{src: '{b}/BEACON-A-ns-img.png'}});
  el('http://www.w3.org/2000/svg', 'link', {{rel: 'stylesheet', href: '{b}/BEACON-A-svg-link.css'}});
  el('urn:x', 'meta', {{'http-equiv': 'refresh', content: '0;url={b}/BEACON-A-refresh'}});
  el('urn:x', 'img', {{src: 'x', onerror: "fetch('{b}/BEACON-A-onerror')"}});
  document.documentElement.replaceWith(root);
}})();
</script></body></html>"#
    );
    let page_b = r#"<!DOCTYPE html>
<html <body="1" lang="en"><head><title>Static page</title></head>
<body><h1>A static page</h1>
<svg width="40" height="40"><use href="/b/sprite.svg#dot"></use></svg>
<svg width="40" height="40"><use href="/b/sprite2.svg#dot2"></use></svg>
<p>No script on this page.</p></body></html>"#
        .to_string();
    let sprite_b = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><symbol id=\"dot\" viewBox=\"0 0 10 10\"><circle cx=\"5\" cy=\"5\" r=\"4\" fill=\"#33f\"/></symbol><1 a=\"<img x='>' src=x onerror=fetch('{b}/BEACON-B-sprite-onerror')>\"/></svg>"
    );
    let sprite_b2 = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><symbol id=\"dot2\" viewBox=\"0 0 10 10\"><circle cx=\"5\" cy=\"5\" r=\"4\" fill=\"#3a3\"/></symbol><_:script xmlns:_=\"http://www.w3.org/2000/svg\">fetch(\"{b}/BEACON-B-sprite2\")</_:script><title>&lt;img src=x onerror=fetch('{b}/BEACON-B-title')&gt;</title></svg>"
    );
    let c1 = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\"><rect width=\"20\" height=\"20\" fill=\"green\"/><_:script xmlns:_=\"http://www.w3.org/2000/svg\">document.documentElement.setAttribute(\"data-pwned\",\"C1\");fetch(\"{b}/BEACON-C1\")</_:script></svg>"
    );
    let c2 = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\"><rect width=\"20\" height=\"20\" fill=\"blue\"/><h.x:script xmlns:h.x=\"http://www.w3.org/1999/xhtml\">document.documentElement.setAttribute(\"data-pwned\",\"C2\");fetch(\"{b}/BEACON-C2\")</h.x:script></svg>"
    );
    let c3 = format!(
        "<?xml version=\"1.0\"?><!DOCTYPE svg [<!ENTITY x \"&#60;script&#62;document.documentElement.setAttribute(&#34;data-pwned&#34;,&#34;C3&#34;);fetch(&#34;{b}/BEACON-C3&#34;)&#60;/script&#62;\">]><svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\"><rect width=\"20\" height=\"20\" fill=\"orange\"/>&x;</svg>"
    );
    let c4 = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\" x=\">\" onload=\"document.documentElement.setAttribute('data-pwned','C4');fetch('{b}/BEACON-C4')\"><rect width=\"20\" height=\"20\"/></svg>"
    );
    let page_c =
        r#"<!DOCTYPE html><html><head><title>Pictures</title></head><body><h1>Pictures</h1>
<img src="/c/p1.svg" width="20" height="20"><img src="/c/p2.svg" width="20" height="20">
<img src="/c/p3.svg" width="20" height="20"><img src="/c/p4.svg" width="20" height="20">
</body></html>"#
            .to_string();
    let page_f = r#"<!DOCTYPE html><html><head><title>Files</title><link rel="stylesheet" href="/f/site.css"></head><body>
<img src="file://attacker.example/share/pic.png" width="10" height="10">
<div style="width:10px;height:10px;background:url('file://attacker.example/share/bg.png')"></div>
<video poster="file://attacker.example/share/poster.png" width="10" height="10"></video>
<a href="file:///etc/passwd">a local file</a><p class="x y">body text</p></body></html>"#
        .to_string();
    let css_f = "@import url(file://attacker.example/share/imp.css);\n.x { background: url(file://attacker.example/share/css.png); }\n.y { border-image: u\\72l(//attacker.example/share/esc.png) 30; }\n";
    let page_e = format!(
        r#"<!DOCTYPE html><html><head><title>Sockets</title></head><body><h1>Sockets</h1><p id="out">waiting</p>
<script>try {{ var ws = new WebSocket('ws://127.0.0.1:{ws}/ws-secret'); ws.onmessage = function(m){{ document.getElementById('out').textContent = 'LAN data: ' + m.data; }}; }} catch (e) {{}}</script></body></html>"#
    );
    // A redirect without a `Location` is shown as a page, so it gets the capture's policy.
    let page_g = format!(
        r#"<!DOCTYPE html><html><head><title>No location</title></head><body><h1>A redirect without a Location</h1><p id="out">waiting</p>
<script>try {{ var ws = new WebSocket('ws://127.0.0.1:{ws}/ws-302'); ws.onmessage = function(m){{ document.getElementById('out').textContent = 'LAN data: ' + m.data; }}; }} catch (e) {{}}</script></body></html>"#
    );
    HashMap::from([
        ("/a/".to_string(), html(page_a)),
        ("/b/".to_string(), html(page_b)),
        ("/b/sprite.svg".to_string(), svg(sprite_b)),
        ("/b/sprite2.svg".to_string(), svg(sprite_b2)),
        ("/c/".to_string(), html(page_c)),
        ("/c/p1.svg".to_string(), svg(c1)),
        ("/c/p2.svg".to_string(), svg(c2)),
        ("/c/p3.svg".to_string(), svg(c3)),
        ("/c/p4.svg".to_string(), svg(c4)),
        ("/f/".to_string(), html(page_f)),
        (
            "/f/site.css".to_string(),
            Page {
                status: 200,
                mime: "text/css",
                body: css_f.as_bytes().to_vec(),
            },
        ),
        ("/e/".to_string(), html(page_e)),
        (
            "/g/".to_string(),
            Page {
                status: 302,
                ..html(page_g)
            },
        ),
    ])
}

/// Captures `url` into a new folder below `into`; the result's error flag and text.
async fn capture_into(
    server: &BrowserServer,
    chat: &ChatContext,
    url: &str,
    into: &Path,
) -> (bool, String) {
    let request = json!({"url": url, "folder": into.display().to_string(), "widths": [1024]});
    let action = describe(server, request.clone(), chat).await;
    assert!(action.hard_limit.is_none(), "{:?}", action.hard_limit);
    let out = server
        .call(
            "browser_capture_site",
            &args(request),
            &action,
            CallContext {
                chat: chat.clone(),
                cancel: CancellationToken::new(),
                progress: None,
            },
        )
        .await;
    (out.is_error, out.joined_text())
}

/// Nothing in a saved copy can run, and nothing points anywhere but at its own files: the
/// copy is parsed as a browser parses it and every element and attribute is looked at
/// (page text may well quote markup; it is only text).
fn assert_inert_files(folder: &Path, beacon: u16, site: u16) {
    let index = read_copy(&folder.join("index.html"));
    assert!(
        index.contains("<html")
            && index.contains("<head><meta http-equiv=\"Content-Security-Policy\""),
        "the policy comes first: {index}"
    );
    let outside = |v: &str| {
        let l = v.trim().to_ascii_lowercase();
        l.contains("file:")
            || l.contains("javascript:")
            || l.contains("attacker.example")
            || l.contains("127.0.0.1")
            || l.contains(&format!(":{beacon}"))
            || l.contains(&format!(":{site}"))
            || l.contains("data:image/svg+xml")
            || l.starts_with("//")
            || l.starts_with("http:")
            || l.starts_with("https:")
    };
    let (doc, whole) = browser::capture::inert::parse(&index);
    assert!(whole);
    let mut problems: Vec<String> = Vec::new();
    for node in doc.root().descendants() {
        node.query(|n| {
            let dom_query::NodeData::Element(e) = &n.data else {
                return;
            };
            let name = e.name.local.as_ref().to_ascii_lowercase();
            if [
                "script",
                "noscript",
                "embed",
                "object",
                "applet",
                "iframe",
                "frame",
                "frameset",
                "base",
                "portal",
                "fencedframe",
                "foreignobject",
                "xmp",
                "plaintext",
            ]
            .contains(&name.as_str())
            {
                problems.push(format!("<{name}>"));
            }
            for a in &e.attrs {
                let an = a.name.local.as_ref().to_ascii_lowercase();
                let v = a.value.as_ref();
                if an.starts_with("on") {
                    problems.push(format!("<{name} {an}>"));
                }
                if name == "meta" && an == "content" && v == browser::capture::html::COPY_POLICY {
                    continue;
                }
                if name == "meta" && an == "http-equiv" && v != "Content-Security-Policy" {
                    problems.push(format!("<meta http-equiv={v}>"));
                }
                let link = (name == "a" || name == "area") && an == "href";
                if link {
                    if !(v.starts_with('#') || v.starts_with("https://example.com")) && outside(v) {
                        problems.push(format!("<{name} href={v}>"));
                    }
                } else if outside(v) {
                    problems.push(format!("<{name} {an}={v}>"));
                }
            }
        });
        // Style text: every address is the copy's own, a fragment, data: or about:invalid.
        if node.has_name("style") {
            let css = node.text().to_string();
            if outside(&css.replace("about:invalid", "")) {
                problems.push(format!("<style>{css}</style>"));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{}: {problems:?}\n{index}",
        folder.display()
    );
    for css in plain_sheets(folder) {
        for bad in ["file:", "attacker.example", "u\\72l", "127.0.0.1"] {
            assert!(!css.contains(bad), "{bad} in a saved style sheet: {css}");
        }
    }
}

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// A text file of the copy, which starts with a UTF-8 byte order mark (so a browser reads
/// it as UTF-8 whatever it declares), without the mark.
fn read_copy(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap();
    assert!(
        bytes.starts_with(BOM),
        "{} starts with a UTF-8 byte order mark",
        path.display()
    );
    String::from_utf8(bytes[BOM.len()..].to_vec()).unwrap()
}

/// The saved style sheets: each read as UTF-8 by a browser too (a byte order mark, no
/// `@charset`, no control characters but tab and line ends), so what the capture checked is
/// what a browser reads.
fn plain_sheets(folder: &Path) -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(folder.join("styles")) else {
        return Vec::new();
    };
    let mut sheets = Vec::new();
    for e in dir {
        let path = e.unwrap().path();
        let css = read_copy(&path);
        assert!(
            !css.trim_start()
                .to_ascii_lowercase()
                .starts_with("@charset"),
            "{}: {css}",
            path.display()
        );
        assert!(
            !css.chars()
                .any(|c| c.is_ascii_control() && !matches!(c, '\t' | '\n' | '\r' | '\x0c')),
            "{}: a control character: {css:?}",
            path.display()
        );
        sheets.push(css);
    }
    sheets
}

/// Opens `file` from disk in a separate headless browser driven over its debugging channel,
/// and returns every address it asked for outside the file's own folder (`file:` ones too,
/// which no proxy sees), besides `data:` and `about:` ones, and every WebSocket it opened.
async fn requests_outside(file: &Path, work: &Path) -> Vec<String> {
    let exe = browser::find_browser(None, None).unwrap().path;
    let profile = tempfile::tempdir_in(work).unwrap();
    let mut config = BrowserConfig::new(profile.path().join("profile"));
    config.headless = true;
    config.no_sandbox = true;
    config.connection = ConnectionMode::Pipe;
    let launched = browser::launch::launch(&exe, &config)
        .await
        .expect("start the browser");
    let (cdp, mut events) = browser::cdp::Cdp::start(launched.transport);
    let wait = Duration::from_secs(20);
    let target = cdp
        .call(
            "Target.createTarget",
            json!({"url": "about:blank"}),
            None,
            wait,
        )
        .await
        .unwrap();
    let attached = cdp
        .call(
            "Target.attachToTarget",
            json!({"targetId": target["targetId"], "flatten": true}),
            None,
            wait,
        )
        .await
        .unwrap();
    let session = attached["sessionId"].as_str().unwrap().to_string();
    cdp.call("Network.enable", json!({}), Some(&session), wait)
        .await
        .unwrap();
    cdp.call("Page.enable", json!({}), Some(&session), wait)
        .await
        .unwrap();
    let url = url::Url::from_file_path(file).unwrap().to_string();
    cdp.call("Page.navigate", json!({"url": url}), Some(&session), wait)
        .await
        .unwrap();
    // The recording runs until the page has loaded (a slow computer may take its time over
    // the pictures and sheets a page names), then 2 s more for what the loaded page still
    // asks for. A load that never comes ends the wait after 30 s, and what was seen by then
    // is checked as it is.
    let mut seen: Vec<String> = Vec::new();
    let mut loaded = false;
    let mut until = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let event = match tokio::time::timeout_at(until, events.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(_) if loaded => break,
            Err(_) => {
                eprintln!("{url} did not finish loading in 30 s; recording 2 s more");
                loaded = true;
                until = tokio::time::Instant::now() + Duration::from_secs(2);
                continue;
            }
        };
        if event.session_id.as_deref() != Some(session.as_str()) {
            continue;
        }
        let text = |p: &str| {
            event
                .params
                .pointer(p)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        match event.method.as_str() {
            "Network.requestWillBeSent" => seen.push(text("/request/url")),
            "Network.webSocketCreated" => seen.push(format!("WebSocket {}", text("/url"))),
            // Only the file's own load counts (not the blank page the tab opened with).
            "Page.loadEventFired" if !loaded && seen.contains(&url) => {
                loaded = true;
                until = tokio::time::Instant::now() + Duration::from_secs(2);
            }
            _ => {}
        }
    }
    drop(cdp);
    drop(launched.child);
    assert!(
        seen.contains(&url),
        "the recorder saw the file itself load: {seen:?}"
    );
    let own = url::Url::from_directory_path(file.parent().unwrap())
        .unwrap()
        .to_string();
    seen.retain(|u| !u.starts_with(&own) && !u.starts_with("data:") && !u.starts_with("about:"));
    seen
}

/// Opens `file` from disk in a separate browser run with every request sent to a
/// recording proxy, and returns its DOM and what reached the proxy or the beacon server.
async fn open_recorded(file: &Path, work: &Path, beacon: &Site) -> (String, Vec<String>) {
    let recorder = serve(HashMap::new()).await;
    let dom = run_browser_through(
        &["--virtual-time-budget=4000".into(), "--dump-dom".into()],
        file,
        work,
        &format!("127.0.0.1:{}", recorder.port),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut seen: Vec<String> = recorder
        .requests()
        .into_iter()
        // The browser's own background requests (time, updates) are not the copy's.
        .filter(|r| {
            !["google.com", "googleapis.com", "gvt1.com", "gstatic.com"]
                .iter()
                .any(|h| r.contains(h))
        })
        .collect();
    seen.extend(beacon.requests());
    (dom, seen)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hostile_pages_leave_an_inert_copy_that_loads_nothing() {
    if !have_browser() {
        return;
    }
    let beacon = serve(HashMap::new()).await;
    let ws = serve(HashMap::new()).await;
    let site = serve(hostile_pages(beacon.port, ws.port)).await;
    let work = tempfile::tempdir().unwrap();
    let approved = work.path().join("approved");
    std::fs::create_dir(&approved).unwrap();
    let chat = chat(&approved);
    let server = server(&[site.port], work.path());

    // The instrument works: a file that does load something shows up in the recording.
    let control = work.path().join("control.html");
    std::fs::write(
        &control,
        format!("<!DOCTYPE html><img src=\"http://attacker.example/control.png\"><img src=\"http://127.0.0.1:{}/BEACON-control\">", beacon.port),
    )
    .unwrap();
    let (_, seen) = open_recorded(&control, work.path(), &beacon).await;
    assert!(
        seen.iter()
            .any(|r| r.contains("attacker.example/control.png"))
            && seen.iter().any(|r| r.contains("BEACON-control")),
        "the recording proxy saw: {seen:?}"
    );
    beacon.log.lock().unwrap().clear();

    for page in ["a", "b", "c", "f", "e", "g"] {
        let into = approved.join(page);
        let (failed, text) =
            capture_into(&server, &chat, &site.url(&format!("/{page}/")), &into).await;
        assert!(!failed, "/{page}/: {text}");
        let folder = only_dir(&dunce::canonicalize(&into).unwrap());
        assert_inert_files(&folder, beacon.port, site.port);
        let (dom, seen) = open_recorded(&folder.join("index.html"), work.path(), &beacon).await;
        assert!(
            seen.is_empty(),
            "/{page}/: the copy made requests: {seen:?}"
        );
        assert!(!dom.contains("data-pwned"), "/{page}/: {dom}");
        let seen = requests_outside(&folder.join("index.html"), work.path()).await;
        assert!(seen.is_empty(), "/{page}/: the copy asked for {seen:?}");
        let index = read_copy(&folder.join("index.html"));
        match page {
            "a" => assert!(index.contains("Copy of the page"), "{index}"),
            "b" => {
                assert!(
                    index.starts_with("<!DOCTYPE html>\n<html lang=\"en\"><head>"),
                    "{index}"
                );
                assert!(
                    index.contains("<symbol id=\"dot2\""),
                    "the clean shapes are copied in: {index}"
                );
                assert!(
                    !index.contains("<symbol id=\"dot\""),
                    "a picture that is not well-formed XML is left out: {index}"
                );
                assert!(!index.contains("<img"), "{index}");
            }
            "c" => {
                let svgs: Vec<PathBuf> = std::fs::read_dir(folder.join("assets"))
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .filter(|p| p.extension().is_some_and(|x| x == "svg"))
                    .collect();
                assert_eq!(
                    svgs.len(),
                    3,
                    "the picture with a document type is left out: {svgs:?}"
                );
                for svg in svgs {
                    let text = std::fs::read_to_string(&svg).unwrap();
                    assert!(
                        !text.contains("script") && !text.contains("onload"),
                        "{text}"
                    );
                    let (dom, seen) = open_recorded(&svg, work.path(), &beacon).await;
                    assert!(seen.is_empty(), "{}: requests {seen:?}", svg.display());
                    assert!(!dom.contains("data-pwned"), "{}: {dom}", svg.display());
                    let seen = requests_outside(&svg, work.path()).await;
                    assert!(seen.is_empty(), "{}: asked for {seen:?}", svg.display());
                }
            }
            "e" | "g" => {
                assert!(index.contains("<p id=\"out\">waiting</p>"), "{index}");
                assert!(!index.contains("LAN data"), "{index}");
            }
            _ => {}
        }
    }
    // The capture itself never reached the beacon, and the policy kept the WebSocket from
    // even connecting.
    assert!(beacon.requests().is_empty(), "{:?}", beacon.requests());
    assert!(
        ws.requests().is_empty(),
        "the WebSocket connected: {:?}",
        ws.requests()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_websocket_that_starts_stops_the_capture_and_nothing_is_saved() {
    if !have_browser() {
        return;
    }
    let beacon = serve(HashMap::new()).await;
    let ws = serve(HashMap::new()).await;
    let site = serve(hostile_pages(beacon.port, ws.port)).await;
    let work = tempfile::tempdir().unwrap();
    let approved = work.path().join("approved");
    std::fs::create_dir(&approved).unwrap();
    let chat = chat(&approved);
    // Without the blocking policy (test hook), the WebSocket starts.
    let server = server_with(&[site.port], work.path(), true);
    let into = approved.join("e");
    let (failed, text) = capture_into(&server, &chat, &site.url("/e/"), &into).await;
    assert!(failed, "the capture went on: {text}");
    assert!(
        text.contains("opened a WebSocket to ws://127.0.0.1:")
            && text.contains("nothing was saved"),
        "{text}"
    );
    assert!(
        ws.requests().iter().any(|r| r.starts_with("/ws-secret")),
        "the WebSocket did start here: {:?}",
        ws.requests()
    );
    let left = std::fs::read_dir(&into).map(|d| d.count()).unwrap_or(0);
    assert_eq!(left, 0, "no capture folder is left");
}

/// The path part of `path`'s `file:` address: `/tmp/x/marks` here, `/C:/Users/x/marks` on
/// Windows, so `//localhost{this}/red.png` names a file in that folder on every system.
fn file_url_path(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().path().to_string()
}

/// Pages whose CSS hides addresses from a check that reads it differently than a browser
/// does, and responses asked for as pictures that are not pictures (the round-1
/// verification's V1, V2, U and U2). `marks` is a folder outside every copy: a copy that
/// names a file there would load it when opened (`//localhost/...` is a `file:` address).
fn trick_pages(beacon: u16, marks: &Path) -> HashMap<String, Page> {
    let b = format!("http://127.0.0.1:{beacon}");
    let mark = file_url_path(marks);
    let page = |mime: &'static str, body: String| Page {
        status: 200,
        mime,
        body: body.into_bytes(),
    };
    // V1: an address reached through var() (or env(), or a function made up) inside
    // image-set() and its relatives, in the page, an element's style, a saved sheet, an SVG
    // style element and an SVG picture.
    let v1 = format!(
        r#"<!DOCTYPE html><html lang="en"><head><title>V1 var image-set</title>
<link rel="stylesheet" href="/v1/site.css">
<style>
:root {{ --u1: "//attacker.example/share/V1-inline.png"; --l1: "//localhost{mark}/red.png"; }}
.a {{ width: 60px; height: 60px; background-image: image-set(var(--u1) 1x); }}
.l {{ width: 60px; height: 60px; background-image: image-set(var(--l1) 1x); }}
.h {{ width: 60px; height: 60px; --hh: "{b}/BEACON-V1-http.png"; background-image: -webkit-image-set(var(--hh) 1x); }}
.e {{ width: 60px; height: 60px; background-image: image-set(env(--nothing, "//localhost{mark}/red.png") 1x); }}
.x {{ width: 60px; height: 60px; background-image: cross-fade(var(--l1) 50%, url(/v1/ok.png)); }}
.ok {{ width: 60px; height: 60px; background-image: image-set(url(/v1/ok.png) 1x, "/v1/ok.png" 2x); }}
</style></head><body><h1>V1</h1>
<div class="a">a</div><div class="l">l</div><div class="h">h</div><div class="e">e</div><div class="x">x</div><div class="ok">ok</div>
<div style="--u3: '//attacker.example/share/V1-attr.png'; width:60px;height:60px;background-image:image-set(var(--u3) 1x)">attr</div>
<div style="--l3: '//localhost{mark}/green.png'; width:60px;height:60px;background-image:image-set(var(--l3) 1x)">attr-l</div>
<div class="d">d</div><div class="dl">dl</div>
<svg width="70" height="70"><style>.sv {{ --sv: "//localhost{mark}/magenta.png"; background-image: image-set(var(--sv) 1x); }}</style><rect class="sv" width="10" height="10"/></svg>
<img src="/v1/pic.svg" width="40" height="40">
</body></html>"#
    );
    let v1_css = format!(
        r#":root {{ --u4: "//attacker.example/share/V1-sheet.png"; --l4: "//localhost{mark}/blue.png"; }}
.d {{ width:60px; height:60px; background-image: image-set(var(--u4) 1x); }}
.dl {{ width:60px; height:60px; background-image: image(var(--l4)); }}
"#
    );
    let v1_svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" style="--u:'//localhost{mark}/yellow.png';background-image:image-set(var(--u) 1x)"><style>svg {{ --v: "//localhost{mark}/yellow.png"; }} rect {{ mask-image: image-set(var(--v) 1x); cursor: image-set(made-up(--v) 1x), auto; }}</style><rect width="100" height="100" fill="green"/></svg>"#
    );
    // V2: a sheet that declares ISO-2022-JP, where ESC ( B is invisible: read as UTF-8 the
    // address is inside a comment, read as ISO-2022-JP the comment ends before it.
    let hidden = |decl: String| format!("/* *\x1b(B/{decl}/**/");
    let v2 = format!(
        r#"<!DOCTYPE html><html lang="en"><head><title>V2 charset</title>
<link rel="stylesheet" href="/v2/site.css">
<style>@charset "ISO-2022-JP";
.j {{ width:60px; height:60px; outline: 1px solid #000; {} }}
.k {{ width:60px; height:60px; outline: 1px solid #000; {} }}
</style></head><body><h1>V2</h1><div class="j">j</div><div class="k">k</div><div class="m">m</div><div class="n">n</div>
</body></html>"#,
        hidden(format!(
            "background-image:url(//localhost{mark}/orange.png);"
        )),
        hidden("background-image:url(//attacker.example/share/V2-inline.png);".into()),
    );
    let v2_css = format!(
        "@charset \"ISO-2022-JP\";\n{}\n.m {{ width:60px; height:60px; outline: 1px solid #000; {} }}\n.n {{ width:60px; height:60px; {} }}\n",
        hidden("@import url(//attacker.example/share/V2-import.css);".into()),
        hidden(format!("background-image:url(//localhost{mark}/cyan.png);")),
        hidden("background-image:url(//attacker.example/share/V2-sheet.png);".into()),
    );
    // U: a disk image and a page asked for as pictures; U2: a disk image fetched by a script
    // and served as image/x-cd-image.
    let mut iso = vec![0u8; 64 * 1024];
    iso[0x8000..0x8006].copy_from_slice(b"\x01CD001");
    iso[0x8028..0x8028 + 11].copy_from_slice(b"HARNESSTEST");
    let u = r#"<!DOCTYPE html><html><head><title>U</title></head><body><h1>U</h1><img src="/u/disk" width="10" height="10"><img src="/u/page" width="10" height="10"></body></html>"#;
    let u2 = r#"<!DOCTYPE html><html><head><title>U2</title></head><body><h1>U2</h1><script>fetch("/u/installer").then(r => r.arrayBuffer()).then(b => { document.body.dataset.got = b.byteLength; });</script></body></html>"#;
    HashMap::from([
        ("/v1/".into(), page("text/html; charset=utf-8", v1)),
        ("/v1/site.css".into(), page("text/css", v1_css)),
        ("/v1/pic.svg".into(), page("image/svg+xml", v1_svg)),
        (
            "/v1/ok.png".into(),
            Page {
                status: 200,
                mime: "image/png",
                body: png(8, 8, [20, 160, 60]),
            },
        ),
        ("/v2/".into(), page("text/html; charset=utf-8", v2)),
        ("/v2/site.css".into(), page("text/css", v2_css)),
        ("/u/".into(), page("text/html; charset=utf-8", u.into())),
        (
            "/u/disk".into(),
            Page {
                status: 200,
                mime: "application/octet-stream",
                body: iso.clone(),
            },
        ),
        (
            "/u/page".into(),
            page(
                "text/html",
                "<!DOCTYPE html><html><body><script>document.title='ran'</script>a page served for a picture</body></html>".into(),
            ),
        ),
        ("/u2/".into(), page("text/html; charset=utf-8", u2.into())),
        (
            "/u/installer".into(),
            Page {
                status: 200,
                mime: "image/x-cd-image",
                body: iso,
            },
        ),
    ])
}

/// Every file of a capture folder, below it.
fn files_below(folder: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut todo = vec![folder.to_path_buf()];
    while let Some(dir) = todo.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let path = e.unwrap().path();
            if path.is_dir() {
                todo.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

fn holds(path: &Path, needle: &[u8]) -> bool {
    std::fs::read(path)
        .unwrap()
        .windows(needle.len())
        .any(|w| w == needle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn css_tricks_and_odd_pictures_leave_a_copy_that_loads_nothing() {
    if !have_browser() {
        return;
    }
    let beacon = serve(HashMap::new()).await;
    let work = tempfile::tempdir().unwrap();
    let marks = work.path().join("marks");
    std::fs::create_dir(&marks).unwrap();
    for (name, rgb) in [
        ("red", [255, 0, 0]),
        ("green", [0, 255, 0]),
        ("blue", [0, 0, 255]),
        ("magenta", [255, 0, 255]),
        ("yellow", [255, 255, 0]),
        ("orange", [255, 160, 0]),
        ("cyan", [0, 255, 255]),
    ] {
        std::fs::write(marks.join(format!("{name}.png")), png(8, 8, rgb)).unwrap();
    }
    let site = serve(trick_pages(beacon.port, &marks)).await;
    let approved = work.path().join("approved");
    std::fs::create_dir(&approved).unwrap();
    let chat = chat(&approved);
    let server = server(&[site.port], work.path());

    // The instrument works, on the very tricks below left as they are: a file outside the
    // copy's folder named through var() in image-set(), and one hidden from a UTF-8 reading
    // in a sheet that declares ISO-2022-JP, are loaded and show up, as does another
    // computer's address.
    let control = work.path().join("control");
    std::fs::create_dir(&control).unwrap();
    std::fs::write(
        control.join("index.html"),
        format!(
            "<!DOCTYPE html><link rel=\"stylesheet\" href=\"iso.css\"><style>:root {{ --l: \"//localhost{}/red.png\"; }} .v {{ width:9px; height:9px; background-image: image-set(var(--l) 1x); }}</style><div class=\"v\"></div><div class=\"m\"></div><img src=\"//attacker.example/share/control.png\">",
            file_url_path(&marks)
        ),
    )
    .unwrap();
    std::fs::write(
        control.join("iso.css"),
        format!(
            "@charset \"ISO-2022-JP\";\n.m {{ width:9px; height:9px; /* *\x1b(B/background-image:url(//localhost{}/cyan.png);/**/ }}\n",
            file_url_path(&marks)
        ),
    )
    .unwrap();
    let seen = requests_outside(&control.join("index.html"), work.path()).await;
    assert!(
        seen.iter().any(|u| u.ends_with("/marks/red.png"))
            && seen.iter().any(|u| u.ends_with("/marks/cyan.png"))
            && seen.iter().any(|u| u.contains("attacker.example")),
        "the recorder saw: {seen:?}"
    );

    for page in ["v1", "v2", "u", "u2"] {
        let into = approved.join(page);
        let (failed, text) =
            capture_into(&server, &chat, &site.url(&format!("/{page}/")), &into).await;
        assert!(!failed, "/{page}/: {text}");
        let folder = only_dir(&dunce::canonicalize(&into).unwrap());
        let sheets = plain_sheets(&folder);
        let seen = requests_outside(&folder.join("index.html"), work.path()).await;
        assert!(seen.is_empty(), "/{page}/: the copy asked for {seen:?}");
        let index = read_copy(&folder.join("index.html"));
        let report = std::fs::read_to_string(folder.join("report.md")).unwrap();
        let all_css = format!("{index}\n{}", sheets.join("\n"));
        match page {
            "v1" => {
                for made in ["var(", "env(", "made-up(", "image(", "cross-fade("] {
                    let inside = all_css
                        .match_indices("image-set(")
                        .any(|(i, _)| all_css[i..].split(')').next().unwrap_or("").contains(made));
                    assert!(
                        !inside,
                        "{made} is left inside an image function: {all_css}"
                    );
                }
                assert!(
                    !all_css.contains("image(var(") && !all_css.contains("cross-fade(var("),
                    "{all_css}"
                );
                assert!(
                    all_css.contains("assets/ok.png"),
                    "a literal address in image-set() is kept: {all_css}"
                );
                let svgs: Vec<PathBuf> = files_below(&folder)
                    .into_iter()
                    .filter(|p| p.extension().is_some_and(|x| x == "svg"))
                    .collect();
                assert!(!svgs.is_empty(), "the SVG picture is saved");
                for svg in svgs {
                    let seen = requests_outside(&svg, work.path()).await;
                    assert!(seen.is_empty(), "{}: asked for {seen:?}", svg.display());
                    let text = std::fs::read_to_string(&svg).unwrap();
                    assert!(
                        !text.contains("var(--u)") && !text.contains("made-up("),
                        "{text}"
                    );
                }
            }
            "v2" => {
                assert!(!all_css.contains('\x1b'), "{all_css:?}");
                assert!(
                    !all_css.to_ascii_lowercase().contains("@charset"),
                    "{all_css}"
                );
            }
            _ => {
                for file in files_below(&folder) {
                    assert!(
                        !holds(&file, b"HARNESSTEST") && !holds(&file, b"a page served for"),
                        "{} holds what was asked for as a picture but is none",
                        file.display()
                    );
                    assert!(
                        file.extension().is_none_or(|x| x != "img"),
                        "{}",
                        file.display()
                    );
                }
                assert!(
                    report.contains("not a picture format the copy keeps"),
                    "/{page}/: {report}"
                );
            }
        }
    }
    assert!(beacon.requests().is_empty(), "{:?}", beacon.requests());
}

/// Pages whose workers ask a device on the network for its data and hand it to the page: a
/// shared worker (fetch and XHR), a service worker (when it installs and when the page
/// writes to it; it also remembers what it fetched), and a dedicated worker.
/// (The round-1 verification's /wsh2/, /wsw/ and /wd/.)
fn worker_pages(secret: u16) -> HashMap<String, Page> {
    let s = format!("http://127.0.0.1:{secret}");
    let page = |title: &str, code: &str| {
        Page {
        status: 200,
        mime: "text/html; charset=utf-8",
        body: format!(
            r#"<!DOCTYPE html><html lang="en"><head><title>{title}</title></head><body><h1>{title}</h1><p id="out">waiting</p><script>
function say(t) {{ document.getElementById('out').textContent += ' | ' + t; }}
{code}
</script></body></html>"#
        )
        .into_bytes(),
    }
    };
    let js = |code: String| Page {
        status: 200,
        mime: "application/javascript",
        body: code.into_bytes(),
    };
    let then = |what: &str, tell: &str| {
        format!(
            "fetch('{s}/LAN-{what}').then(function (r) {{ return r.text(); }}).then(function (t) {{ {tell}('{what}: ' + t); }}, function () {{ {tell}('{what} failed'); }});"
        )
    };
    let shared = format!(
        "var ports = [], got = []; onconnect = function (e) {{ var p = e.ports[0]; ports.push(p); p.start(); got.forEach(function (g) {{ p.postMessage(g); }}); }}; function send(t) {{ got.push(t); ports.forEach(function (p) {{ p.postMessage(t); }}); }} {} var x = new XMLHttpRequest(); x.open('GET', '{s}/LAN-shared-xhr'); x.onload = function () {{ send('shared-xhr: ' + x.responseText); }}; x.onloadend = function () {{ if (!x.status) send('shared-xhr failed'); }}; x.send();",
        then("shared-fetch", "send")
    );
    let service = format!(
        "function tell(t) {{ self.clients.matchAll({{includeUncontrolled: true}}).then(function (cs) {{ cs.forEach(function (c) {{ c.postMessage(t); }}); }}); }} self.addEventListener('install', function (e) {{ self.skipWaiting(); e.waitUntil(fetch('{s}/LAN-sw-install').then(function (r) {{ return r.text(); }}).then(function (t) {{ self.lan = t; tell('sw-install: ' + t); }}, function () {{ tell('sw-install failed'); }})); }}); self.addEventListener('activate', function (e) {{ e.waitUntil(self.clients.claim()); }}); self.addEventListener('message', function (e) {{ var reply = function (t) {{ e.source.postMessage(t); }}; if (self.lan) reply('remembered: ' + self.lan); {} }});",
        then("sw-message", "reply")
    );
    let register = |dir: &str| {
        format!(
            "navigator.serviceWorker.onmessage = function (m) {{ say(m.data); }}; navigator.serviceWorker.register('/{dir}/sw.js').then(function () {{ return navigator.serviceWorker.ready; }}).then(function (r) {{ r.active.postMessage('hello'); }}, function (e) {{ say('no service worker: ' + e); }});"
        )
    };
    HashMap::from([
        (
            "/wsh/".into(),
            page(
                "Shared",
                "var w = new SharedWorker('/wsh/shared.js'); w.port.onmessage = function (m) { say(m.data); }; w.port.start();",
            ),
        ),
        ("/wsh/shared.js".into(), js(shared)),
        ("/wsw/".into(), page("Service", &register("wsw"))),
        ("/wsw/sw.js".into(), js(service.clone())),
        ("/wsp/".into(), page("Service before", &register("wsp"))),
        ("/wsp/sw.js".into(), js(service)),
        (
            "/wd/".into(),
            page(
                "Dedicated",
                "var w = new Worker('/wd/worker.js'); w.onmessage = function (m) { say(m.data); };",
            ),
        ),
        (
            "/wd/worker.js".into(),
            js(then("dedicated-fetch", "postMessage")),
        ),
    ])
}

async fn navigate(server: &BrowserServer, chat: &ChatContext, url: &str) {
    let a = json!({ "url": url });
    let action = match server
        .describe("browser_navigate", &args(a.clone()), chat)
        .await
    {
        Described::Action(action) => action,
        Described::Invalid(why) => panic!("not describable: {why}"),
    };
    let out = server
        .call(
            "browser_navigate",
            &args(a),
            &action,
            CallContext {
                chat: chat.clone(),
                cancel: CancellationToken::new(),
                progress: None,
            },
        )
        .await;
    assert!(!out.is_error, "{}", out.joined_text());
}

/// Nothing a capture saved or said holds the device's data.
fn assert_no_secret(folder: &Path, text: &str) {
    assert!(!text.contains("LAN-SECRET"), "{text}");
    for file in files_below(folder) {
        assert!(
            !holds(&file, b"LAN-SECRET"),
            "{} holds the device's data: {}",
            file.display(),
            String::from_utf8_lossy(&std::fs::read(&file).unwrap())
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_of_workers_are_checked_and_workers_that_ran_before_are_stopped() {
    if !have_browser() {
        return;
    }
    // A "device on the network": loopback, on a port the browser's rules do not allow.
    let secret = serve(
        [
            "shared-fetch",
            "shared-xhr",
            "sw-install",
            "sw-message",
            "dedicated-fetch",
        ]
        .into_iter()
        .map(|what| {
            (
                format!("/LAN-{what}"),
                Page {
                    status: 200,
                    mime: "text/plain",
                    body: format!("LAN-SECRET-{what}").into_bytes(),
                },
            )
        })
        .collect(),
    )
    .await;
    let site = serve(worker_pages(secret.port)).await;
    let work = tempfile::tempdir().unwrap();
    let approved = work.path().join("approved");
    std::fs::create_dir(&approved).unwrap();
    let chat = chat(&approved);
    let server = server(&[site.port], work.path());

    // A service worker does not run during a capture: the browser cannot be made to pause
    // the requests of one installed before, so none is let run. The others run, checked.
    for (page, refused) in [
        ("wsh", &["shared-fetch failed", "shared-xhr failed"][..]),
        ("wsw", &[][..]),
        ("wd", &["dedicated-fetch failed"][..]),
    ] {
        let into = approved.join(page);
        let (failed, text) =
            capture_into(&server, &chat, &site.url(&format!("/{page}/")), &into).await;
        assert!(!failed, "/{page}/: {text}");
        let folder = only_dir(&dunce::canonicalize(&into).unwrap());
        assert_no_secret(&folder, &text);
        assert_inert_files(&folder, secret.port, site.port);
        let index = read_copy(&folder.join("index.html"));
        let report = std::fs::read_to_string(folder.join("report.md")).unwrap();
        for what in refused {
            // The worker ran (it was not held), and its request was refused by the checks.
            assert!(index.contains(what), "/{page}/: {what}: {index}");
            let name = what.trim_end_matches(" failed");
            assert!(
                report.contains(&format!("LAN-{name}")),
                "/{page}/: the report lists the refused request: {report}"
            );
        }
        if page == "wsw" {
            assert!(index.contains("<p id=\"out\">waiting</p>"), "{index}");
            assert!(
                report.contains("service worker was not let run"),
                "{report}"
            );
        } else {
            assert!(!report.contains("service worker"), "{report}");
        }
    }
    assert!(
        secret.requests().is_empty(),
        "a worker's request got past the checks: {:?}",
        secret.requests()
    );

    // Outside a capture a worker's requests are not checked one by one, so a service worker
    // that ran before may hold what it fetched then (and one installed before would not
    // have its requests paused); the capture stops it first and does not let it run again.
    navigate(&server, &chat, &site.url("/wsp/")).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !secret.requests().iter().any(|r| r == "/LAN-sw-message") {
        assert!(
            std::time::Instant::now() < deadline,
            "the service worker did not run before the capture: {:?}",
            secret.requests()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let before = secret.requests().len();
    let into = approved.join("wsp");
    let (failed, text) = capture_into(&server, &chat, &site.url("/wsp/"), &into).await;
    assert!(!failed, "/wsp/: {text}");
    let folder = only_dir(&dunce::canonicalize(&into).unwrap());
    assert_no_secret(&folder, &text);
    let index = read_copy(&folder.join("index.html"));
    assert!(index.contains("<p id=\"out\">waiting</p>"), "{index}");
    let report = std::fs::read_to_string(folder.join("report.md")).unwrap();
    assert!(
        report.contains("service worker was not let run"),
        "{report}"
    );
    assert_eq!(
        secret.requests().len(),
        before,
        "a request got past the checks during the capture: {:?}",
        secret.requests()
    );
    server.shutdown().await;
}
