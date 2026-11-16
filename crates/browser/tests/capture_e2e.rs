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
                    Some(p) => ("200 OK", p.mime, p.body.clone()),
                    None => ("404 Not Found", "text/plain", b"not found".to_vec()),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
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
        pages.insert(path.to_string(), Page { mime, body });
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
        mime: "text/html; charset=utf-8",
        body: s.into_bytes(),
    };
    let svg = |s: String| Page {
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
                mime: "text/css",
                body: css_f.as_bytes().to_vec(),
            },
        ),
        ("/e/".to_string(), html(page_e)),
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
    let index = std::fs::read_to_string(folder.join("index.html")).unwrap();
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
    if let Ok(dir) = std::fs::read_dir(folder.join("styles")) {
        for e in dir {
            let css = std::fs::read_to_string(e.unwrap().path()).unwrap();
            for bad in ["file:", "attacker.example", "u\\72l", "127.0.0.1"] {
                assert!(!css.contains(bad), "{bad} in a saved style sheet: {css}");
            }
        }
    }
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

    for page in ["a", "b", "c", "f", "e"] {
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
        let index = std::fs::read_to_string(folder.join("index.html")).unwrap();
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
                }
            }
            "e" => {
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
