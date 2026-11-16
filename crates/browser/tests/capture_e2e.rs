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
    let mut config = BrowserConfig::new(profile.join("profile"));
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
        // Offline: every web request goes to a closed port.
        "--proxy-server=127.0.0.1:9".into(),
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
