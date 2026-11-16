//! Running a capture (ARCHITECTURE.md 9.3, `browser_capture_site`): opening the page under
//! the capture's request checks, scrolling, reading, screenshots, and saving.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::capture::assets::{Budget, Caps, Kind, SkipReason, megabytes, thousands};
use crate::capture::netlog::{CaptureLog, NetEntry};
use crate::capture::report::{FontLine, ReportInput, page_text, page_url};
use crate::capture::write::Writer;
use crate::capture::{
    ANIMATIONS_JS, COLLECT_JS, FETCH_MISSING_JS, GLOBALS_JS, SCROLL_JS, anim, assets, css, html,
    names, report,
};
use crate::netpolicy::AddressPolicy;
use crate::session::{Browser, CMD};

/// The tallest full-page screenshot, in CSS pixels (the browser's own limit for one picture).
const MAX_SHOT_HEIGHT: u64 = 16_384;
/// The furthest the capture scrolls.
const MAX_SCROLL: u64 = 60_000;
/// Style sheets read at most (imports included).
const MAX_SHEETS: usize = 300;
/// Pictures named by style sheets but never loaded that the capture asks for at most.
const MAX_MISSING: usize = 150;
/// Scripts whose start is read to recognise libraries.
const MAX_SCRIPT_SAMPLES: usize = 200;
const SCRIPT_SAMPLE_BYTES: usize = 64 * 1024;

/// What the approved action asks for.
#[derive(Debug, Clone)]
pub(crate) struct Plan {
    pub url: Url,
    /// The approved folder the capture folder is created in (canonical).
    pub folder: PathBuf,
    pub widths: Vec<u32>,
    pub scroll: bool,
    pub caps: Caps,
    pub policy: AddressPolicy,
}

/// What a finished capture hands back.
#[derive(Debug)]
pub(crate) struct Captured {
    pub folder: PathBuf,
    pub report: String,
    /// The first screen at each width, as JPEG (base64), for the model to look at.
    pub first_screens: Vec<(u32, String)>,
    pub files: usize,
    pub bytes: u64,
}

/// Puts the browser into capture mode for one tab and takes it out again: every request is
/// checked (not only page loads) and recorded, the cache and service workers are bypassed so
/// every file is fetched under those checks. Dropped without [`Guard::finish`] (Stop), it
/// restores the browser on a task of its own.
struct Guard {
    b: Arc<Browser>,
    session: String,
    done: bool,
}

impl Guard {
    async fn start(b: &Arc<Browser>, session: &str) -> Self {
        b.lock().capture = Some(CaptureLog::new(session));
        b.set_capture_checks(true).await;
        Self {
            b: b.clone(),
            session: session.to_string(),
            done: false,
        }
    }

    async fn finish(mut self) {
        self.done = true;
        self.b.lock().capture = None;
        restore(&self.b, &self.session).await;
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        self.b.lock().capture = None;
        let b = self.b.clone();
        let session = self.session.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move { restore(&b, &session).await });
        }
    }
}

async fn restore(b: &Browser, session: &str) {
    // A capture that started meanwhile keeps its own settings.
    if b.lock().capture.is_some() {
        return;
    }
    b.set_capture_checks(false).await;
    for (method, params) in [
        ("Emulation.clearDeviceMetricsOverride", json!({})),
        ("Emulation.setTouchEmulationEnabled", json!({"enabled": false})),
        ("Network.setCacheDisabled", json!({"cacheDisabled": false})),
        ("Network.setBypassServiceWorker", json!({"bypass": false})),
        ("Network.disable", json!({})),
    ] {
        let _ = b.cdp.call(method, params, Some(session), CMD).await;
    }
}

fn cdp_text(e: crate::cdp::CdpError) -> String {
    match e {
        crate::cdp::CdpError::Closed => "the browser closed".into(),
        other => other.to_string(),
    }
}

async fn cdp(b: &Browser, session: &str, method: &str, params: Value) -> Result<Value, String> {
    b.cdp
        .call(method, params, Some(session), CMD)
        .await
        .map_err(cdp_text)
}

async fn call_fn(
    b: &Browser,
    session: &str,
    world: i64,
    function: &str,
    arg: Value,
    timeout: Duration,
) -> Result<Value, String> {
    let r = b
        .cdp
        .call(
            "Runtime.callFunctionOn",
            json!({
                "functionDeclaration": function,
                "executionContextId": world,
                "arguments": [{"value": arg}],
                "returnByValue": true,
                "awaitPromise": true,
            }),
            Some(session),
            timeout,
        )
        .await
        .map_err(|e| format!("could not read the page ({})", cdp_text(e)))?;
    if let Some(ex) = r.get("exceptionDetails") {
        let text = ex
            .pointer("/exception/description")
            .or_else(|| ex.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("script error");
        return Err(format!(
            "could not read the page ({})",
            policy::sanitize::one_line(text, 200)
        ));
    }
    Ok(r.pointer("/result/value").cloned().unwrap_or(Value::Null))
}

/// The screen height used with a width: a laptop, a tablet or a phone.
fn screen_for(width: u32) -> (u32, bool) {
    if width >= 1024 {
        (900, false)
    } else if width >= 600 {
        (1024, true)
    } else {
        (844, true)
    }
}

async fn set_width(b: &Browser, session: &str, width: u32) -> Result<(), String> {
    let (height, mobile) = screen_for(width);
    cdp(
        b,
        session,
        "Emulation.setDeviceMetricsOverride",
        json!({"width": width, "height": height, "deviceScaleFactor": 1, "mobile": mobile}),
    )
    .await?;
    let touch = if mobile {
        json!({"enabled": true, "maxTouchPoints": 5})
    } else {
        json!({"enabled": false})
    };
    let _ = cdp(b, session, "Emulation.setTouchEmulationEnabled", touch).await;
    Ok(())
}

/// Waits until the page has made no request for `calm`, at most `max`.
async fn quiet(b: &Browser, calm: Duration, max: Duration) {
    let end = Instant::now() + max;
    let state = || {
        b.lock()
            .capture
            .as_ref()
            .map(|c| (c.changes, c.pending()))
            .unwrap_or((0, 0))
    };
    let mut last = state();
    let mut since = Instant::now();
    while Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let now = state();
        if now.0 != last.0 {
            last = now;
            since = Instant::now();
            continue;
        }
        if (now.1 == 0 && since.elapsed() >= calm) || since.elapsed() >= calm * 3 {
            return;
        }
    }
}

async fn snapshot(b: &Browser, session: &str, world: i64, log: &mut anim::AnimationLog, when: &str) {
    if let Ok(v) = call_fn(b, session, world, ANIMATIONS_JS, json!({"max": 300}), CMD).await {
        log.add_snapshot(&v, when);
    }
}

/// Scrolls to `y`; returns (position, page height, screen height, scrolls inside an
/// element).
async fn scroll_to(b: &Browser, session: &str, world: i64, y: u64) -> Result<(u64, u64, u64, bool), String> {
    let r = call_fn(b, session, world, SCROLL_JS, json!({"y": y}), CMD).await?;
    let n = |k: &str| r.get(k).and_then(Value::as_f64).map(|f| f.max(0.0) as u64).unwrap_or(0);
    Ok((
        n("y"),
        n("height"),
        n("viewport").max(1),
        r.get("inner").and_then(Value::as_bool).unwrap_or(false),
    ))
}

/// Scrolls through the whole page, step by step, so late pictures load and effects that
/// start on scroll run, reading the running animations at each step.
#[allow(clippy::too_many_arguments)]
async fn scroll_through(
    b: &Browser,
    session: &str,
    world: i64,
    log: &mut anim::AnimationLog,
    pause: Duration,
    max_steps: usize,
    max_time: Duration,
    label: &str,
    notes: &mut Vec<String>,
) -> Result<bool, String> {
    let start = Instant::now();
    let mut y = 0u64;
    let mut inner = false;
    for _ in 0..max_steps {
        let (at, height, screen, scrolls_inside) = scroll_to(b, session, world, y).await?;
        inner |= scrolls_inside;
        tokio::time::sleep(pause).await;
        snapshot(b, session, world, log, &format!("{label} (at {} px)", thousands(at))).await;
        if at + screen + 2 >= height {
            break;
        }
        if start.elapsed() > max_time {
            notes.push(format!(
                "The page is long: it was scrolled for {} seconds, to {} px of {} px.",
                max_time.as_secs(),
                thousands(at),
                thousands(height)
            ));
            break;
        }
        y = at + (screen * 85 / 100).max(200);
        if y > MAX_SCROLL {
            notes.push(format!("Scrolling stopped at {} px.", thousands(MAX_SCROLL)));
            break;
        }
    }
    tokio::time::sleep(pause).await;
    Ok(inner)
}

/// One screenshot.
struct Shot {
    width: u32,
    full: bool,
    data: Vec<u8>,
    ext: &'static str,
    size: (u32, u32),
}

async fn capture_picture(b: &Browser, session: &str, params: Value) -> Result<String, String> {
    let r = b
        .cdp
        .call("Page.captureScreenshot", params, Some(session), Duration::from_secs(60))
        .await
        .map_err(cdp_text)?;
    r.get("data")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "the browser returned no picture".into())
}

fn decode(b64: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(b64).ok()
}

/// The first screen and the full page at `width` (the page at its top).
#[allow(clippy::too_many_arguments)]
async fn take_shots(
    b: &Browser,
    session: &str,
    world: i64,
    width: u32,
    per_file: u64,
    shots: &mut Vec<Shot>,
    first_screens: &mut Vec<(u32, String)>,
    notes: &mut Vec<String>,
) -> Result<(), String> {
    let first = capture_picture(b, session, json!({"format": "png", "captureBeyondViewport": false})).await?;
    let size = crate::page::png_size(&first).unwrap_or((width, 0));
    if let Some(data) = decode(&first) {
        shots.push(Shot {
            width,
            full: false,
            data,
            ext: "png",
            size,
        });
    }
    if let Ok(jpg) = capture_picture(
        b,
        session,
        json!({"format": "jpeg", "quality": 70, "captureBeyondViewport": false}),
    )
    .await
    {
        first_screens.push((width, jpg));
    }
    let (_, height, _, _) = scroll_to(b, session, world, 0).await?;
    if height > MAX_SHOT_HEIGHT {
        notes.push(format!(
            "The full-page screenshot {width} px wide stops at {} px of the page's {} px.",
            thousands(MAX_SHOT_HEIGHT),
            thousands(height)
        ));
    }
    let full_height = height.clamp(1, MAX_SHOT_HEIGHT);
    let clip = json!({"x": 0, "y": 0, "width": width, "height": full_height, "scale": 1});
    let full = capture_picture(
        b,
        session,
        json!({"format": "png", "captureBeyondViewport": true, "clip": clip}),
    )
    .await?;
    let mut data = decode(&full).unwrap_or_default();
    let mut ext = "png";
    let mut size = crate::page::png_size(&full).unwrap_or((width, full_height as u32));
    if data.len() as u64 > per_file {
        // A very long, detailed page: a JPEG of it fits the limit for one file.
        if let Ok(jpg) = capture_picture(
            b,
            session,
            json!({"format": "jpeg", "quality": 80, "captureBeyondViewport": true, "clip": clip}),
        )
        .await
        {
            data = decode(&jpg).unwrap_or_default();
            ext = "jpg";
            size = (width, full_height as u32);
        }
    }
    shots.push(Shot {
        width,
        full: true,
        data,
        ext,
        size,
    });
    Ok(())
}

/// A style sheet of the page, ready to be rewritten and saved.
struct Sheet {
    /// Index in the page's `styles` table (`None` for a sheet named by `@import`).
    index: Option<usize>,
    /// Address relative addresses in it resolve against.
    base: Url,
    /// Address it came from (empty for inline and adopted sheets).
    url: String,
    text: String,
    stem: String,
    /// Where it was saved, once it was.
    path: Option<String>,
}

fn strip_fragment(u: &str) -> &str {
    u.split('#').next().unwrap_or(u)
}

/// Responses by address (every address of their redirects), for those the capture may use.
fn by_url(entries: &[NetEntry]) -> HashMap<String, usize> {
    let mut map = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        for u in &e.urls {
            map.entry(strip_fragment(u).to_string()).or_insert(i);
        }
    }
    map
}

/// The body of a response, decoded.
async fn body(b: &Browser, session: &str, request_id: &str) -> Result<Vec<u8>, String> {
    let r = b
        .cdp
        .call(
            "Network.getResponseBody",
            json!({"requestId": request_id}),
            Some(session),
            Duration::from_secs(30),
        )
        .await
        .map_err(cdp_text)?;
    let text = r.get("body").and_then(Value::as_str).unwrap_or("");
    if r.get("base64Encoded").and_then(Value::as_bool) == Some(true) {
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(|_| "the browser returned a body that could not be read".to_string())
    } else {
        Ok(text.as_bytes().to_vec())
    }
}

/// Whether the capture may use a response: it passed the request checks during the
/// capture, it finished, and it is a whole, successful one. `Err` says why not.
fn usable(e: &NetEntry, checked: &HashSet<String>, policy: &AddressPolicy) -> Result<(), SkipReason> {
    if !checked.contains(&e.request_id) {
        return Err(SkipReason::Blocked(
            "it was not checked by the capture's request checks".into(),
        ));
    }
    if let Some(f) = &e.failed {
        return Err(SkipReason::Failed(policy::sanitize::one_line(f, 80)));
    }
    if !e.finished {
        return Err(SkipReason::Failed("it had not finished loading".into()));
    }
    if e.status == 206 {
        return Err(SkipReason::Partial);
    }
    if !(200..300).contains(&e.status) {
        return Err(SkipReason::Failed(format!("the server answered {}", e.status)));
    }
    // The browser resolves names itself: a site that answered from a private address on a
    // direct connection (the port the address names) is refused even though its name
    // looked public when it was checked (ARCHITECTURE.md 13.6, DNS rebinding).
    if let (Some(ip), Some(port)) = (&e.remote_ip, e.remote_port)
        && let Ok(url) = Url::parse(e.final_url())
        && url.port_or_known_default() == Some(port)
        && let Ok(addr) = ip.trim_matches(|c| c == '[' || c == ']').parse()
        && let Err(why) = policy.check_ip(addr, port)
    {
        return Err(SkipReason::Blocked(policy::sanitize::one_line(&why, 160)));
    }
    Ok(())
}

/// Runs the whole capture in the tab `tab`.
pub(crate) async fn run(
    b: &Arc<Browser>,
    tab: &str,
    plan: &Plan,
    cancel: &CancellationToken,
    progress: &(dyn Fn(String) + Send + Sync),
) -> Result<Captured, String> {
    let session = b.tab_session(tab)?;
    let guard = Guard::start(b, &session).await;
    let result = work(b, tab, &session, plan, cancel, progress).await;
    guard.finish().await;
    result
}

#[allow(clippy::too_many_lines)]
async fn work(
    b: &Arc<Browser>,
    tab: &str,
    session: &str,
    plan: &Plan,
    cancel: &CancellationToken,
    progress: &(dyn Fn(String) + Send + Sync),
) -> Result<Captured, String> {
    let mut notes: Vec<String> = Vec::new();
    cdp(
        b,
        session,
        "Network.enable",
        json!({
            "maxTotalBufferSize": 400_000_000u64,
            "maxResourceBufferSize": assets::MAX_FILE_BYTES + 64 * 1024,
            "maxPostDataSize": 0
        }),
    )
    .await?;
    let _ = cdp(b, session, "Network.setCacheDisabled", json!({"cacheDisabled": true})).await;
    let _ = cdp(b, session, "Network.setBypassServiceWorker", json!({"bypass": true})).await;
    let first_width = plan.widths.first().copied().unwrap_or(1440);
    set_width(b, session, first_width).await?;

    progress(format!("Opening {}", policy::sanitize::one_line(plan.url.as_str(), 200)));
    if let Some(note) = b.navigate(tab, plan.url.as_str(), cancel).await? {
        notes.push(note);
    }
    quiet(b, Duration::from_millis(600), Duration::from_secs(5)).await;
    let world = crate::session::isolated_world(&b.cdp, session, tab)
        .await
        .map_err(|e| format!("could not read the page ({})", cdp_text(e)))?;
    let _ = tokio::time::timeout(
        Duration::from_secs(4),
        b.cdp.call(
            "Runtime.evaluate",
            json!({"expression": "document.fonts.ready.then(() => true)", "contextId": world, "awaitPromise": true, "returnByValue": true}),
            Some(session),
            CMD,
        ),
    )
    .await;

    let mut anims = anim::AnimationLog::default();
    snapshot(b, session, world, &mut anims, "at the top").await;
    let mut inner_scroll = false;
    if plan.scroll {
        progress("Scrolling through the page so late pictures and scroll effects load".into());
        inner_scroll = scroll_through(
            b,
            session,
            world,
            &mut anims,
            Duration::from_millis(350),
            80,
            Duration::from_secs(25),
            "while scrolling",
            &mut notes,
        )
        .await?;
        scroll_to(b, session, world, 0).await?;
        tokio::time::sleep(Duration::from_millis(600)).await;
        quiet(b, Duration::from_millis(400), Duration::from_secs(3)).await;
        snapshot(b, session, world, &mut anims, "back at the top").await;
    }
    if inner_scroll {
        notes.push("The page scrolls inside one element, so its full-page screenshots show only the first screen.".into());
    }
    let globals = b
        .cdp
        .call(
            "Runtime.evaluate",
            json!({"expression": GLOBALS_JS, "returnByValue": true, "timeout": 2000}),
            Some(session),
            Duration::from_secs(5),
        )
        .await
        .ok()
        .and_then(|r| r.pointer("/result/value").cloned())
        .unwrap_or_else(|| json!({}));

    progress("Reading the page".into());
    let nonce: String = uuid::Uuid::new_v4().simple().to_string().chars().take(16).collect();
    let page = call_fn(
        b,
        session,
        world,
        COLLECT_JS,
        json!({"nonce": nonce, "csp": html::COPY_POLICY}),
        Duration::from_secs(90),
    )
    .await?;
    let page_html = page.get("html").and_then(Value::as_str).unwrap_or("");
    if page_html.is_empty() {
        return Err("The page could not be read (it has no document to copy).".into());
    }
    if page.get("cut").and_then(Value::as_bool) == Some(true) {
        notes.push(format!(
            "The page has more than {} elements; the copy stops there.",
            thousands(page.get("nodes").and_then(Value::as_u64).unwrap_or(0))
        ));
    }
    let page_href = page
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or(plan.url.as_str())
        .to_string();
    let page_base = page
        .get("base")
        .and_then(Value::as_str)
        .and_then(|u| Url::parse(u).ok())
        .or_else(|| Url::parse(&page_href).ok())
        .unwrap_or_else(|| plan.url.clone());
    if netpolicy_origin(&page_href) != netpolicy_origin(plan.url.as_str()) {
        notes.push(format!("The page moved to {} while it loaded.", page_url(&page_href)));
    }

    let mut shots: Vec<Shot> = Vec::new();
    let mut first_screens: Vec<(u32, String)> = Vec::new();
    progress(format!("Taking screenshots {first_width} pixels wide"));
    take_shots(b, session, world, first_width, plan.caps.per_file, &mut shots, &mut first_screens, &mut notes).await?;

    // Stills of canvases (drawn by scripts, so not in the copy otherwise).
    let mut canvas_stills: Vec<Option<Vec<u8>>> = Vec::new();
    for c in page.get("canvases").and_then(Value::as_array).cloned().unwrap_or_default() {
        let n = |k: &str| c.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let clip = json!({"x": n("x").max(0.0), "y": n("y").max(0.0), "width": n("w").max(1.0), "height": n("h").max(1.0), "scale": 1});
        let still = capture_picture(b, session, json!({"format": "png", "captureBeyondViewport": true, "clip": clip}))
            .await
            .ok()
            .and_then(|d| decode(&d));
        canvas_stills.push(still);
    }

    // Style sheets: their own bytes where the browser has them, else what the page holds.
    let mut sheets = gather_sheets(b, session, &page, &page_base).await;
    // Pictures the sheets name that the page never loaded (hover states, other widths).
    let missing = missing_pictures(b, &sheets);
    if !missing.is_empty() {
        progress(format!("Asking for {} picture(s) the style sheets name", missing.len()));
        let _ = call_fn(
            b,
            session,
            world,
            FETCH_MISSING_JS,
            json!({"urls": missing, "ms": 8000}),
            Duration::from_secs(15),
        )
        .await;
        quiet(b, Duration::from_millis(300), Duration::from_secs(2)).await;
    }

    for &width in plan.widths.iter().skip(1) {
        progress(format!("Taking screenshots {width} pixels wide"));
        set_width(b, session, width).await?;
        tokio::time::sleep(Duration::from_millis(700)).await;
        quiet(b, Duration::from_millis(400), Duration::from_secs(3)).await;
        if plan.scroll {
            scroll_through(
                b,
                session,
                world,
                &mut anims,
                Duration::from_millis(200),
                60,
                Duration::from_secs(15),
                &format!("{width} px wide, while scrolling"),
                &mut notes,
            )
            .await?;
            scroll_to(b, session, world, 0).await?;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        snapshot(b, session, world, &mut anims, &format!("{width} px wide, at the top")).await;
        take_shots(b, session, world, width, plan.caps.per_file, &mut shots, &mut first_screens, &mut notes).await?;
    }
    if plan.widths.len() > 1 {
        set_width(b, session, first_width).await?;
        let _ = scroll_to(b, session, world, 0).await;
    }

    // ---- saving ------------------------------------------------------------------------
    progress("Saving the copy".into());
    let (entries, checked, blocked, log_full) = {
        let s = b.lock();
        match s.capture.as_ref() {
            Some(c) => (
                c.ordered()
                    .into_iter()
                    .filter(|e| e.frame_id.is_empty() || e.frame_id == tab)
                    .cloned()
                    .collect::<Vec<_>>(),
                c.checked.clone(),
                c.blocked.clone(),
                c.full,
            ),
            None => (Vec::new(), HashSet::new(), Vec::new(), false),
        }
    };
    let folder_name = names::capture_folder_name(&plan.url, chrono::Local::now());
    let mut w = Writer::create(&plan.folder, &folder_name)?;
    let mut budget = Budget::new(plan.caps);
    let mut skipped: Vec<(String, String)> = Vec::new();
    // The page, the report and animations.json are always written; count them first.
    budget.force(page_html.len() as u64 + 64 * 1024);
    budget.files += 2;

    // Screenshots.
    let mut screenshot_lines: Vec<(String, u32, u32)> = Vec::new();
    for shot in &shots {
        let label = if shot.full { "full-page" } else { "first-screen" };
        let name = format!("w{}-{label}.{}", shot.width, shot.ext);
        match budget.take(shot.data.len() as u64) {
            Ok(()) => {
                let rel = w.write("screenshots", &name, &shot.data)?;
                screenshot_lines.push((rel, shot.size.0, shot.size.1));
            }
            Err(why) => skipped.push((format!("screenshots/{name}"), why.words(&plan.caps))),
        }
    }

    // Style sheets come first among the files: room is set aside for them now, and they
    // are written once the files they name are saved.
    let mut style_room: Vec<bool> = Vec::new();
    for sheet in &sheets {
        match budget.take(sheet.text.len() as u64 + 1024) {
            Ok(()) => style_room.push(true),
            Err(why) => {
                let what = if sheet.url.is_empty() {
                    format!("style sheet {}", sheet.stem)
                } else {
                    page_url(&sheet.url)
                };
                skipped.push((what, why.words(&plan.caps)));
                style_room.push(false);
            }
        }
    }

    // Pictures, fonts, icons, animation files and videos.
    let mut saved: HashMap<String, String> = HashMap::new();
    let mut lottie_files = 0usize;
    let mut candidates: Vec<(Kind, &NetEntry)> = entries
        .iter()
        .filter(|e| e.final_url().starts_with("http"))
        .filter_map(|e| {
            let kind = assets::kind_of(&e.resource_type, &e.mime, e.final_url())?;
            (kind != Kind::StyleSheet && kind != Kind::Script).then_some((kind, e))
        })
        .collect();
    candidates.sort_by_key(|(k, e)| (k.priority(), e.order));
    let saving_started = Instant::now();
    let was_blocked = |e: &NetEntry| e.urls.iter().any(|u| blocked.iter().any(|(b, _)| b == u));
    for (kind, e) in candidates {
        let label = page_url(e.final_url());
        if let Err(why) = usable(e, &checked, &plan.policy) {
            // Requests the checks refused are listed under "Not captured" with the reason;
            // data the page failed to fetch is not part of its look.
            if !was_blocked(e) && (!matches!(why, SkipReason::Failed(_)) || kind != Kind::Animation) {
                skipped.push((label, why.words(&plan.caps)));
            }
            continue;
        }
        if saving_started.elapsed() > Duration::from_secs(180) {
            skipped.push((label, "saving took too long, so the rest was left out".into()));
            continue;
        }
        let estimate = e.decoded_len.max(e.encoded_len);
        if estimate > plan.caps.per_file {
            skipped.push((label, SkipReason::TooLarge.words(&plan.caps)));
            continue;
        }
        if let Err(why) = budget.take(estimate) {
            skipped.push((label, why.words(&plan.caps)));
            continue;
        }
        let fetched = body(b, session, &e.request_id).await;
        budget.give_back(estimate);
        let mut bytes = match fetched {
            Ok(bytes) => bytes,
            Err(_) => {
                skipped.push((label, SkipReason::Gone.words(&plan.caps)));
                continue;
            }
        };
        if let Err(why) = budget.take(bytes.len() as u64) {
            skipped.push((label, why.words(&plan.caps)));
            continue;
        }
        let head = &bytes[..bytes.len().min(64)];
        let ext = assets::extension_for(kind, &e.mime, e.final_url(), head);
        if kind == Kind::Animation {
            if ext != "json" || !assets::is_lottie(&bytes) {
                // Data the page fetched, not part of its look: not kept, not listed.
                budget.give_back(bytes.len() as u64);
                continue;
            }
            lottie_files += 1;
        }
        if ext == "svg" {
            match assets::neutralise_svg(&String::from_utf8_lossy(&bytes)) {
                Some(clean) => bytes = clean.into_bytes(),
                None => {
                    budget.give_back(bytes.len() as u64);
                    skipped.push((label, SkipReason::UnsafePicture.words(&plan.caps)));
                    continue;
                }
            }
        }
        let fallback = match kind {
            Kind::Font => "font",
            Kind::Animation => "animation",
            Kind::Video => "video",
            Kind::Icon => "icon",
            _ => "picture",
        };
        let name = w.name("assets", &names::stem_of_url(e.final_url(), fallback), ext);
        let rel = w.write("assets", &name, &bytes)?;
        for u in &e.urls {
            saved.insert(strip_fragment(u).to_string(), rel.clone());
        }
    }

    // Stills of canvases.
    let mut canvas_paths: Vec<Option<String>> = Vec::new();
    for (i, still) in canvas_stills.iter().enumerate() {
        let path = match still {
            Some(data) if budget.take(data.len() as u64).is_ok() => {
                let name = w.name("assets", &format!("canvas-{}", i + 1), "png");
                Some(w.write("assets", &name, data)?)
            }
            _ => None,
        };
        canvas_paths.push(path);
    }

    // Style sheets: names first (an @import names another sheet), then the rewritten text.
    for (sheet, room) in sheets.iter_mut().zip(&style_room) {
        if *room {
            let name = w.name("styles", &sheet.stem, "css");
            sheet.path = Some(format!("styles/{name}"));
        }
    }
    let sheet_by_url: HashMap<String, String> = sheets
        .iter()
        .filter(|s| !s.url.is_empty())
        .filter_map(|s| {
            let file = s.path.as_ref()?.trim_start_matches("styles/").to_string();
            Some((strip_fragment(&s.url).to_string(), file))
        })
        .collect();
    let mut facts: Vec<(String, css::SheetFacts)> = Vec::new();
    let mut style_paths: HashMap<usize, String> = HashMap::new();
    for sheet in &sheets {
        let Some(path) = sheet.path.clone() else {
            continue;
        };
        let refs = css::find_urls(&sheet.text, &sheet.base);
        let rewritten = css::rewrite(&sheet.text, &refs, |r| {
            let (file, frag) = match r.url.find('#') {
                Some(i) => (&r.url[..i], &r.url[i..]),
                None => (r.url.as_str(), ""),
            };
            if r.import {
                sheet_by_url.get(file).map(|f| format!("{f}{frag}"))
            } else {
                saved.get(file).map(|p| format!("../{p}{frag}"))
            }
        });
        let file_name = path.trim_start_matches("styles/");
        let rel = w.write("styles", file_name, rewritten.as_bytes())?;
        facts.push((rel.clone(), css::facts(&sheet.text, &sheet.base)));
        if let Some(i) = sheet.index {
            style_paths.insert(i, rel);
        }
    }

    // Scripts, for reference: saved as text, never linked. Their starts tell the libraries.
    let mut samples: Vec<(String, String)> = Vec::new();
    let page_scripts = page.get("scripts").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut script_entries: Vec<&NetEntry> = entries
        .iter()
        .filter(|e| e.resource_type == "Script" && e.final_url().starts_with("http"))
        .collect();
    script_entries.sort_by_key(|e| e.order);
    for e in script_entries {
        let label = page_url(e.final_url());
        if let Err(why) = usable(e, &checked, &plan.policy) {
            if !was_blocked(e) {
                skipped.push((label, why.words(&plan.caps)));
            }
            continue;
        }
        let Ok(bytes) = body(b, session, &e.request_id).await else {
            skipped.push((label, SkipReason::Gone.words(&plan.caps)));
            continue;
        };
        if samples.len() < MAX_SCRIPT_SAMPLES {
            let head = &bytes[..bytes.len().min(SCRIPT_SAMPLE_BYTES)];
            samples.push((e.final_url().to_string(), String::from_utf8_lossy(head).into_owned()));
        }
        match budget.take(bytes.len() as u64) {
            Ok(()) => {
                let name = w.name("scripts", &names::stem_of_url(e.final_url(), "script"), "js.txt");
                w.write("scripts", &name, &bytes)?;
            }
            Err(why) => skipped.push((label, why.words(&plan.caps))),
        }
    }
    let mut inline_n = 0usize;
    for s in &page_scripts {
        let text = s.get("inline").and_then(Value::as_str).unwrap_or("");
        if text.trim().is_empty() {
            continue;
        }
        inline_n += 1;
        if samples.len() < MAX_SCRIPT_SAMPLES {
            samples.push((String::new(), text.chars().take(SCRIPT_SAMPLE_BYTES).collect()));
        }
        let kind = s.get("type").and_then(Value::as_str).unwrap_or("");
        let ext = if kind.contains("json") { "json.txt" } else { "js.txt" };
        if budget.take(text.len() as u64).is_ok() {
            let name = w.name("scripts", &format!("inline-{inline_n}"), ext);
            w.write("scripts", &name, text.as_bytes())?;
        }
    }

    // index.html.
    let urls_table: Vec<String> = string_list(&page, "urls");
    let targets = html::Targets {
        urls: urls_table.iter().map(|u| saved.get(strip_fragment(u)).cloned()).collect(),
        url_originals: urls_table.clone(),
        styles: page
            .get("styles")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .enumerate()
                    .map(|(i, s)| match style_paths.get(&i) {
                        Some(p) => p.clone(),
                        None => s
                            .get("href")
                            .and_then(Value::as_str)
                            .filter(|h| !h.is_empty())
                            .map(str::to_string)
                            .unwrap_or_else(|| "about:invalid".into()),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        srcsets: page
            .get("srcsets")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .map(|cands| {
                        let pairs: Vec<(String, String)> = cands
                            .as_array()
                            .map(|c| {
                                c.iter()
                                    .filter_map(|p| {
                                        let p = p.as_array()?;
                                        Some((
                                            p.first()?.as_str()?.to_string(),
                                            p.get(1).and_then(Value::as_str).unwrap_or("").to_string(),
                                        ))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        html::srcset(&pairs, &saved)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        canvases: canvas_paths.clone(),
    };
    let copy = html::replace_tokens(page_html, &nonce, &targets);
    if html::has_active_content(&copy) {
        notes.push("index.html was not saved: something that could run was still in it after the scripts were removed.".into());
    } else {
        w.write("", "index.html", copy.as_bytes())?;
    }

    // animations.json.
    let script_refs: Vec<anim::ScriptSample<'_>> = samples
        .iter()
        .map(|(u, t)| anim::ScriptSample { url: u, text: t })
        .collect();
    let webgl = page
        .get("canvases")
        .and_then(Value::as_array)
        .map(|c| c.len())
        .unwrap_or(0);
    let webgl = if samples.iter().any(|(_, t)| t.contains("webgl") || t.contains("WebGL")) {
        webgl
    } else {
        0
    };
    let markers = page.get("markers").cloned().unwrap_or(Value::Null);
    let libraries = anim::detect_libraries(&globals, &markers, &script_refs, lottie_files, webgl);
    let captured_at = chrono::Local::now();
    let doc = anim::document(
        &page_href,
        &captured_at.to_rfc3339(),
        &anims,
        &facts,
        &libraries,
    );
    let doc_bytes = serde_json::to_vec_pretty(&doc).unwrap_or_default();
    w.write("", "animations.json", &doc_bytes)?;

    // report.md.
    let not_captured = not_captured_lines(&page, &blocked, log_full, &canvas_paths);
    let fonts = font_lines(&page, &facts, &saved);
    let mut all_media: Vec<String> = Vec::new();
    let mut custom = Vec::new();
    let mut keyframes = Vec::new();
    let (mut states, mut transitions, mut scroll_driven) = (0, 0, 0);
    for (_, f) in &facts {
        all_media.extend(f.media.iter().cloned());
        custom.extend(f.custom_properties.iter().cloned());
        keyframes.extend(f.keyframes.iter().map(|k| k.0.clone()));
        states += f.states.len();
        transitions += f.transitions.len();
        scroll_driven += f.scroll_driven.len();
    }
    keyframes.dedup();
    let examples: Vec<(String, String, String)> = anims
        .running()
        .iter()
        .take(10)
        .map(|a| {
            let kind = match a.get("kind").and_then(Value::as_str) {
                Some("css-animation") => "CSS animation",
                Some("css-transition") => "CSS transition",
                _ => "script animation",
            };
            (
                kind.to_string(),
                a.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                a.get("target").and_then(Value::as_str).unwrap_or("").to_string(),
            )
        })
        .collect();
    let host = policy::sanitize::display_host(&plan.url).unwrap_or_default();
    let folder_text = w.root().display().to_string();
    let skipped_lines: Vec<(String, String)> = skipped.clone();
    let captured_text = captured_at.format("%Y-%m-%d %H:%M").to_string();
    let title = page.get("title").and_then(Value::as_str).unwrap_or("").to_string();
    let input = ReportInput {
        title: &title,
        url: &page_href,
        host: &host,
        folder: &folder_text,
        captured_at: &captured_text,
        widths: &plan.widths,
        page: Some(&page),
        fonts,
        breakpoints: css::breakpoints(all_media.iter()),
        custom_properties: custom.into_iter().take(40).collect(),
        animation_counts: anims.counts(),
        animation_examples: examples,
        keyframes,
        state_rules: states,
        transition_rules: transitions,
        scroll_driven,
        libraries: &libraries,
        not_captured,
        files: w.files.clone(),
        skipped: skipped_lines,
        screenshots: screenshot_lines,
        notes: notes.clone(),
    };
    let report_text = report::build(&input);
    w.write("", "report.md", report_text.as_bytes())?;
    let bytes: u64 = w.files.iter().map(|f| f.bytes).sum();
    Ok(Captured {
        folder: w.root().to_path_buf(),
        report: report_text,
        first_screens,
        files: w.files.len(),
        bytes,
    })
}

fn netpolicy_origin(u: &str) -> String {
    crate::netpolicy::origin_of_str(u)
}

fn string_list(v: &Value, k: &str) -> Vec<String> {
    v.get(k)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| x.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The page's style sheets, in its order, then the sheets they import.
async fn gather_sheets(b: &Browser, session: &str, page: &Value, page_base: &Url) -> Vec<Sheet> {
    let (entries, checked) = {
        let s = b.lock();
        match s.capture.as_ref() {
            Some(c) => (
                c.ordered().into_iter().cloned().collect::<Vec<_>>(),
                c.checked.clone(),
            ),
            None => (Vec::new(), HashSet::new()),
        }
    };
    let index = by_url(&entries);
    let fetch = |url: &str| -> Option<&NetEntry> {
        let e = &entries[*index.get(strip_fragment(url))?];
        let is_css = e.resource_type == "Stylesheet" || e.mime.contains("css");
        (is_css && checked.contains(&e.request_id) && e.finished && (200..300).contains(&e.status))
            .then_some(e)
    };
    let mut sheets: Vec<Sheet> = Vec::new();
    let (mut inline_n, mut adopted_n) = (0usize, 0usize);
    let styles = page.get("styles").and_then(Value::as_array).cloned().unwrap_or_default();
    for (i, s) in styles.iter().enumerate() {
        let kind = s.get("kind").and_then(Value::as_str).unwrap_or("");
        let page_text = s.get("text").and_then(Value::as_str).unwrap_or("").to_string();
        match kind {
            "link" => {
                let href = s.get("href").and_then(Value::as_str).unwrap_or("").to_string();
                let (text, base) = match fetch(&href) {
                    Some(e) => match body(b, session, &e.request_id).await {
                        Ok(bytes) => (
                            String::from_utf8_lossy(&bytes).into_owned(),
                            Url::parse(e.final_url()).ok(),
                        ),
                        Err(_) => (page_text.clone(), None),
                    },
                    None => (page_text.clone(), None),
                };
                if text.is_empty() {
                    continue;
                }
                let base = base
                    .or_else(|| Url::parse(&href).ok())
                    .unwrap_or_else(|| page_base.clone());
                sheets.push(Sheet {
                    index: Some(i),
                    stem: names::stem_of_url(&href, "styles"),
                    url: href,
                    base,
                    text,
                    path: None,
                });
            }
            "inline" | "adopted" => {
                let stem = if kind == "inline" {
                    inline_n += 1;
                    format!("inline-{inline_n}")
                } else {
                    adopted_n += 1;
                    format!("adopted-{adopted_n}")
                };
                sheets.push(Sheet {
                    index: Some(i),
                    stem,
                    url: String::new(),
                    base: page_base.clone(),
                    text: page_text,
                    path: None,
                });
            }
            _ => {}
        }
    }
    // Imports, breadth first, each sheet once.
    let mut seen: HashSet<String> = sheets
        .iter()
        .filter(|s| !s.url.is_empty())
        .map(|s| strip_fragment(&s.url).to_string())
        .collect();
    let mut i = 0;
    while i < sheets.len() && sheets.len() < MAX_SHEETS {
        let imports: Vec<String> = css::find_urls(&sheets[i].text, &sheets[i].base)
            .into_iter()
            .filter(|r| r.import)
            .map(|r| strip_fragment(&r.url).to_string())
            .collect();
        for url in imports {
            if !seen.insert(url.clone()) || sheets.len() >= MAX_SHEETS {
                continue;
            }
            let Some(e) = fetch(&url) else {
                continue;
            };
            let Ok(bytes) = body(b, session, &e.request_id).await else {
                continue;
            };
            let base = Url::parse(e.final_url()).unwrap_or_else(|_| page_base.clone());
            sheets.push(Sheet {
                index: None,
                stem: names::stem_of_url(&url, "styles"),
                url,
                base,
                text: String::from_utf8_lossy(&bytes).into_owned(),
                path: None,
            });
        }
        i += 1;
    }
    sheets
}

/// Pictures the style sheets name that the page never asked for.
fn missing_pictures(b: &Browser, sheets: &[Sheet]) -> Vec<String> {
    let known: HashSet<String> = b
        .lock()
        .capture
        .as_ref()
        .map(|c| {
            c.entries
                .values()
                .flat_map(|e| e.urls.iter().map(|u| strip_fragment(u).to_string()))
                .collect()
        })
        .unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    for s in sheets {
        for r in css::find_urls(&s.text, &s.base) {
            if r.import {
                continue;
            }
            let url = strip_fragment(&r.url).to_string();
            let picture = names::extension_of_url(&url).is_some_and(|e| {
                matches!(e.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "svg" | "ico" | "bmp")
            });
            if picture && !known.contains(&url) && !out.contains(&url) {
                out.push(url);
                if out.len() >= MAX_MISSING {
                    return out;
                }
            }
        }
    }
    out
}

/// The lines of "Not captured".
fn not_captured_lines(
    page: &Value,
    blocked: &[(String, String)],
    log_full: bool,
    canvas_paths: &[Option<String>],
) -> Vec<String> {
    let mut lines = Vec::new();
    let frames = page.get("frames").and_then(Value::as_array).cloned().unwrap_or_default();
    if !frames.is_empty() {
        let list = frames
            .iter()
            .take(5)
            .filter_map(|f| f.get("src").and_then(Value::as_str))
            .filter(|s| !s.is_empty())
            .map(page_url)
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "{} frame(s) (another page shown inside this one); they are empty in the copy{}",
            frames.len(),
            if list.is_empty() { String::new() } else { format!(": {list}") }
        ));
    }
    let canvases = page.get("markers").and_then(|m| m.get("canvases")).and_then(Value::as_u64).unwrap_or(0);
    if canvases > 0 {
        let stills = canvas_paths.iter().filter(|p| p.is_some()).count();
        lines.push(format!(
            "{canvases} canvas drawing(s) made by scripts (2D or WebGL): they do not move in the copy; {stills} still picture(s) of them were saved"
        ));
    }
    let streams = page
        .get("media")
        .and_then(Value::as_array)
        .map(|m| m.iter().filter(|x| x.get("stream").and_then(Value::as_bool) == Some(true)).count())
        .unwrap_or(0);
    if streams > 0 {
        lines.push(format!("{streams} video stream(s) loaded in parts by a script: only their poster pictures are kept"));
    }
    if !blocked.is_empty() {
        let list = blocked
            .iter()
            .take(6)
            .map(|(u, why)| format!("{} ({})", page_url(u), page_text(why, 120)))
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(format!(
            "{} request(s) blocked by the browser's network rules (a private or local address, or a blocked kind of address): {list}",
            blocked.len()
        ));
    }
    if log_full {
        lines.push("The page made more requests than the capture records; later files are missing.".into());
    }
    lines
}

/// The fonts the page uses, with the files saved for them.
fn font_lines(page: &Value, facts: &[(String, css::SheetFacts)], saved: &HashMap<String, String>) -> Vec<FontLine> {
    let mut lines: Vec<FontLine> = Vec::new();
    let used = page
        .pointer("/stats/fonts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for e in used {
        let Some(a) = e.as_array() else {
            continue;
        };
        let key = a.first().and_then(Value::as_str).unwrap_or("");
        let count = a.get(1).and_then(Value::as_u64).unwrap_or(0);
        let key = key.strip_suffix(" italic").unwrap_or(key);
        let (family, weight) = key.rsplit_once(' ').unwrap_or((key, ""));
        if family.is_empty() {
            continue;
        }
        match lines.iter_mut().find(|l| l.family.eq_ignore_ascii_case(family)) {
            Some(l) => {
                l.used += count;
                if !weight.is_empty() && !l.weights.iter().any(|w| w == weight) {
                    l.weights.push(weight.to_string());
                }
            }
            None => lines.push(FontLine {
                family: family.to_string(),
                weights: if weight.is_empty() { vec![] } else { vec![weight.to_string()] },
                used: count,
                ..Default::default()
            }),
        }
    }
    for (_, f) in facts {
        for face in &f.font_faces {
            let line = match lines.iter_mut().position(|l| l.family.eq_ignore_ascii_case(&face.family)) {
                Some(i) => &mut lines[i],
                None => {
                    if lines.len() >= 20 {
                        continue;
                    }
                    lines.push(FontLine {
                        family: face.family.clone(),
                        ..Default::default()
                    });
                    lines.last_mut().expect("just pushed")
                }
            };
            line.web_font = true;
            for src in &face.sources {
                if let Some(p) = saved.get(strip_fragment(src))
                    && !line.files.contains(p)
                {
                    line.files.push(p.clone());
                    if let Some(h) = Url::parse(src).ok().as_ref().and_then(policy::sanitize::display_host)
                        && !line.hosts.contains(&h)
                    {
                        line.hosts.push(h);
                    }
                }
            }
        }
    }
    lines
}

/// The result text: what was saved and the report, held to `budget` characters.
pub(crate) fn result_text(c: &Captured, host: &str, budget: usize) -> String {
    let head = format!(
        "Saved a copy of the front end of {} in {} ({} files, {}). The first screen at each width is shown below; the full report is report.md in that folder.\n\n",
        page_text(host, 80),
        policy::sanitize::visible(&c.folder.display().to_string()),
        thousands(c.files as u64),
        megabytes(c.bytes)
    );
    let room = budget.saturating_sub(head.chars().count()).max(800);
    format!("{head}{}", report::cap(c.report.clone(), room))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screens_get_a_height_for_their_width() {
        assert_eq!(screen_for(1440), (900, false));
        assert_eq!(screen_for(768), (1024, true));
        assert_eq!(screen_for(390), (844, true));
    }

    #[test]
    fn only_checked_whole_successful_responses_are_used() {
        let policy = AddressPolicy::default();
        let mut checked = HashSet::new();
        checked.insert("1".to_string());
        let ok = NetEntry {
            request_id: "1".into(),
            urls: vec!["https://a.example/x.png".into()],
            status: 200,
            finished: true,
            remote_ip: Some("93.184.215.14".into()),
            remote_port: Some(443),
            ..Default::default()
        };
        assert!(usable(&ok, &checked, &policy).is_ok());
        let unchecked = NetEntry {
            request_id: "2".into(),
            ..ok.clone()
        };
        assert!(matches!(usable(&unchecked, &checked, &policy), Err(SkipReason::Blocked(_))));
        let partial = NetEntry { status: 206, ..ok.clone() };
        assert_eq!(usable(&partial, &checked, &policy), Err(SkipReason::Partial));
        let failed = NetEntry { failed: Some("net::ERR_ABORTED".into()), ..ok.clone() };
        assert!(matches!(usable(&failed, &checked, &policy), Err(SkipReason::Failed(_))));
        // A public name that answered from a private address on a direct connection.
        let rebound = NetEntry {
            remote_ip: Some("192.168.1.1".into()),
            ..ok.clone()
        };
        assert!(matches!(usable(&rebound, &checked, &policy), Err(SkipReason::Blocked(_))));
        // Through a proxy (another port) the address says nothing about the site.
        let proxied = NetEntry {
            remote_ip: Some("10.0.0.8".into()),
            remote_port: Some(3128),
            ..ok.clone()
        };
        assert!(usable(&proxied, &checked, &policy).is_ok());
        let v6 = NetEntry {
            remote_ip: Some("[::1]".into()),
            ..ok
        };
        assert!(usable(&v6, &checked, &policy).is_err());
    }

    #[test]
    fn the_result_text_is_bounded() {
        let c = Captured {
            folder: PathBuf::from("/home/u/site/a.example-2026-11-16-153012"),
            report: "# Capture\n".to_string() + &"line of the report\n".repeat(10_000),
            first_screens: vec![],
            files: 12,
            bytes: 3 * 1024 * 1024,
        };
        let t = result_text(&c, "a.example", 6000);
        assert!(t.starts_with("Saved a copy of the front end of a.example in /home/u/site/a.example-2026-11-16-153012 (12 files, 3.0 MB)."), "{t}");
        assert!(t.chars().count() <= 6000, "{}", t.chars().count());
        assert!(t.contains("[The rest of this report was cut"));
    }
}
