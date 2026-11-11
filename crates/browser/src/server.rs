//! The built-in `browser` server (ARCHITECTURE.md 9.3, 13.3): `browser_navigate`,
//! `browser_snapshot`, `browser_find`, `browser_read_text`, `browser_click`,
//! `browser_type`, `browser_fill_form`, `browser_press_key`, `browser_screenshot`,
//! `browser_back`.
//!
//! `describe` turns arguments into a policy action scoped to the site (scheme + host +
//! port) the action touches; blocked schemes and addresses are hard limits; D8 submit
//! intent makes an action always-ask. `call` re-checks the frame's actual origin, the
//! element and the intent immediately before the side effect and refuses on any change.
//! Page content goes back to the model as plain tool output (untrusted data; the agent
//! loop wraps it).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mcp_host::{BuiltinServer, CallContext, ChatContext, Content, Described, ToolDef, ToolOutput};
use policy::sanitize::{one_line, visible};
use policy::{Action, Risk, Scope};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::approval::NavigationApprover;
use crate::config::BrowserConfig;
use crate::discovery::{self, BrowserUnavailable, FoundBrowser};
use crate::intent::{self, ElementInfo, SubmitIntent, SubmitReason};
use crate::keys;
use crate::netpolicy::{self, AddressPolicy, DnsCache};
use crate::page::device_to_css;
use crate::session::{ActiveCall, Browser, RefTarget};
use crate::snapshot::budget_chars;

/// Told to the model when a tool needs a page and the browser is not open.
const NOT_OPEN: &str = "The browser is not open yet. Use browser_navigate to open a page first.";
/// Told to the model when a reference is unknown.
const UNKNOWN_REF: &str = "Unknown element reference. References come from the latest browser_snapshot or browser_find of the current page; take a new snapshot.";

/// The built-in browser server. One browser, started on first use and shared by the
/// chats that use it; calls run one at a time.
pub struct BrowserServer {
    inner: Arc<Inner>,
}

struct Inner {
    config: Mutex<Arc<BrowserConfig>>,
    approver: Arc<dyn NavigationApprover>,
    browser: tokio::sync::Mutex<Option<Arc<Browser>>>,
    calls: tokio::sync::Mutex<()>,
    dns: Arc<DnsCache>,
    epoch: AtomicU64,
}

impl Drop for Inner {
    /// Dropping the server without [`BrowserServer::shutdown`] still ends everything:
    /// held requests are released (their tokens are children of the shutdown token) and
    /// the browser's process tree is killed.
    fn drop(&mut self) {
        if let Ok(slot) = self.browser.try_lock()
            && let Some(b) = slot.as_ref()
        {
            b.kill_now();
        }
    }
}

impl std::fmt::Debug for BrowserServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserServer").finish_non_exhaustive()
    }
}

fn s<'a>(args: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    args.get(k).and_then(Value::as_str)
}

fn payload_str<'a>(p: &'a Value, k: &str) -> Option<&'a str> {
    p.get(k).and_then(Value::as_str)
}

fn host_of(origin: &str) -> String {
    Url::parse(origin)
        .ok()
        .and_then(|u| policy::sanitize::display_host(&u))
        .unwrap_or_else(|| origin.to_string())
}

fn target_json(t: &RefTarget) -> Value {
    json!({"session": t.session, "frame": t.frame_id, "backend": t.backend})
}

fn target_from(p: &Value) -> Option<RefTarget> {
    Some(RefTarget {
        session: p.get("session")?.as_str()?.to_string(),
        frame_id: p.get("frame")?.as_str()?.to_string(),
        backend: p.get("backend")?.as_i64()?,
    })
}

#[allow(clippy::too_many_arguments)]
fn action(
    tool: &str,
    summary: String,
    preview: String,
    origin: &str,
    risk: Risk,
    read_only: bool,
    outbound: bool,
    payload: Value,
) -> Described {
    // A frame with no site of its own (a sandboxed or srcdoc frame has the opaque origin
    // "null") would share one grant scope with every such frame on every site, so nothing
    // that acts there can be covered by a grant.
    let opaque = origin == "null" && !read_only;
    let risk = if opaque { Risk::AlwaysAsk } else { risk };
    let preview = if opaque {
        format!(
            "{preview}\nThe target is in a frame with no site of its own (sandboxed or local content), so this always asks."
        )
    } else {
        preview
    };
    Described::Action(Action {
        server: "browser".into(),
        tool: tool.into(),
        summary: one_line(&summary, 200),
        preview: visible(&preview),
        scope: Scope::Site {
            origin: origin.to_string(),
        },
        risk,
        read_only,
        outbound,
        hard_limit: None,
        payload,
    })
}

fn hard_limit(tool: &str, url: &str, why: String) -> Described {
    Described::Action(Action {
        server: "browser".into(),
        tool: tool.into(),
        summary: one_line(&format!("Blocked: {url}"), 200),
        preview: visible(&why),
        scope: Scope::Site {
            origin: "blocked".into(),
        },
        risk: Risk::Normal,
        read_only: true,
        outbound: false,
        hard_limit: Some(visible(&why)),
        payload: Value::Null,
    })
}

fn element_words(info: &ElementInfo) -> String {
    let kind = if info.is_editable {
        "field"
    } else if info.is_link {
        "link"
    } else if info.is_button_like || info.is_submit_control {
        "button"
    } else if info.tag == "select" {
        "list"
    } else {
        "element"
    };
    if info.label.is_empty() {
        kind.to_string()
    } else {
        format!(
            "{kind} \"{}\"",
            crate::snapshot::clean_text(&info.label, 80)
        )
    }
}

/// The submit intent for an action with `reason` on an element described by `info`, in
/// a frame of `origin`: a form's action and method when there is a form, else the frame's
/// own origin with any method.
fn intent_for(
    reason: SubmitReason,
    tab: &str,
    info: Option<&ElementInfo>,
    origin: &str,
) -> SubmitIntent {
    let form = info.filter(|i| i.in_form);
    let method = form
        .and_then(|i| i.form_method.clone())
        .filter(|m| !m.eq_ignore_ascii_case("dialog"))
        .map(|m| m.to_ascii_uppercase());
    let target = form
        .and_then(|i| i.form_action.as_deref())
        .map(netpolicy::origin_of_str)
        .filter(|o| o != "null")
        .unwrap_or_else(|| origin.to_string());
    SubmitIntent {
        reason,
        tab: tab.to_string(),
        method,
        target_origin: target,
    }
}

/// How `browser_fill_form` fills an element: `check` (checkbox, radio button), `select`
/// (drop-down list) or `text`; `None` for anything else (buttons, links…).
fn fill_kind(info: &ElementInfo) -> Option<&'static str> {
    if info.is_submit_control || info.is_button_like {
        None
    } else if info.tag == "input" && matches!(info.input_type.as_str(), "checkbox" | "radio") {
        Some("check")
    } else if info.tag == "select" {
        Some("select")
    } else if info.is_editable {
        Some("text")
    } else {
        None
    }
}

/// Whether an element is still the kind of field a fill was approved for.
fn still_fillable_as(kind: Option<&str>, info: &ElementInfo) -> bool {
    kind.is_some() && fill_kind(info) == kind
}

/// Told to the model when it asks for a clipboard shortcut.
fn clipboard_refusal(key: &str, command: &str) -> String {
    format!(
        "{} is the {command} shortcut. The browser tool never uses the clipboard: paste would hand the user's clipboard to the page, and copy or cut would overwrite it. Use browser_type to enter text.",
        one_line(key, 40)
    )
}

fn intent_preview(intent: &SubmitIntent) -> String {
    format!(
        "{}\nIt may send data to {} ({}). A submission anywhere else will ask again.",
        intent.reason.describe(),
        intent.target_origin,
        intent.method.as_deref().unwrap_or("any method")
    )
}

impl BrowserServer {
    /// A server that starts the browser described by `config` on first use. Held
    /// navigations that need the user go to `approver`.
    pub fn new(config: BrowserConfig, approver: Arc<dyn NavigationApprover>) -> Self {
        #[cfg(feature = "test-hooks")]
        let dns = DnsCache::with_overrides(&config.test_dns_overrides);
        #[cfg(not(feature = "test-hooks"))]
        let dns = DnsCache::default();
        Self {
            inner: Arc::new(Inner {
                config: Mutex::new(Arc::new(config)),
                approver,
                browser: tokio::sync::Mutex::new(None),
                calls: tokio::sync::Mutex::new(()),
                dns: Arc::new(dns),
                epoch: AtomicU64::new(0),
            }),
        }
    }

    fn config(&self) -> Arc<BrowserConfig> {
        self.inner
            .config
            .lock()
            .map(|c| c.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    fn address_policy(&self) -> AddressPolicy {
        let c = self.config();
        AddressPolicy {
            allow_private: c.allow_private_networks,
            loopback_ports: c.loopback_ports(),
        }
    }

    /// Which browser would be used, or why there is none (for the tool menu and
    /// Settings; section 15).
    pub fn availability(&self) -> Result<FoundBrowser, BrowserUnavailable> {
        let config = self.config();
        discovery::find_browser(
            config.browser_path.as_deref(),
            config.own_browser_dir.as_deref(),
        )
    }

    /// Applies new settings. A running browser is closed (held requests are failed);
    /// the next call starts one with the new settings.
    pub async fn reconfigure(&self, config: BrowserConfig) {
        if let Ok(mut c) = self.inner.config.lock() {
            *c = Arc::new(config);
        }
        self.shutdown().await;
    }

    /// Fails every held request, closes the browser and kills its process tree. Call it
    /// when the app quits; dropping the server kills the process tree as well.
    pub async fn shutdown(&self) {
        let browser = self.inner.browser.lock().await.take();
        if let Some(b) = browser {
            b.close().await;
        }
    }

    /// Whether a browser is running right now.
    pub async fn is_running(&self) -> bool {
        self.inner
            .browser
            .lock()
            .await
            .as_ref()
            .is_some_and(|b| b.alive())
    }

    async fn running(&self) -> Option<Arc<Browser>> {
        self.inner
            .browser
            .lock()
            .await
            .as_ref()
            .filter(|b| b.alive())
            .cloned()
    }

    async fn ensure_browser(&self) -> Result<Arc<Browser>, String> {
        let mut slot = self.inner.browser.lock().await;
        if let Some(b) = slot.as_ref().filter(|b| b.alive()) {
            return Ok(b.clone());
        }
        if let Some(old) = slot.take() {
            old.close().await;
        }
        let config = self.config();
        let found = discovery::find_browser(
            config.browser_path.as_deref(),
            config.own_browser_dir.as_deref(),
        )
        .map_err(|e| e.for_model())?;
        let launched = crate::launch::launch(&found.path, &config)
            .await
            .map_err(|e| format!("The browser could not be started: {e}"))?;
        let epoch = self.inner.epoch.fetch_add(1, Ordering::Relaxed) + 1;
        let browser = Browser::connect(
            launched,
            config,
            self.inner.approver.clone(),
            self.inner.dns.clone(),
            epoch,
        )
        .await
        .map_err(|e| format!("The browser could not be started: {e}"))?;
        *slot = Some(browser.clone());
        Ok(browser)
    }

    /// Tests only: requests held right now.
    #[cfg(feature = "test-hooks")]
    pub async fn held_requests(&self) -> usize {
        match self.running().await {
            Some(b) => b.lock().held_count(),
            None => 0,
        }
    }

    /// Tests only: the active tab's URL as the browser tracks it.
    #[cfg(feature = "test-hooks")]
    pub async fn active_url(&self) -> Option<String> {
        let b = self.running().await?;
        let s = b.lock();
        let tab = s.active.clone()?;
        s.main_frame(&tab).map(|f| f.url.clone())
    }

    /// Tests only: how many cross-site frame sessions are attached.
    #[cfg(feature = "test-hooks")]
    pub async fn frame_sessions(&self) -> usize {
        match self.running().await {
            Some(b) => b
                .lock()
                .sessions
                .values()
                .filter(|i| i.kind == crate::session::TargetKind::Frame)
                .count(),
            None => 0,
        }
    }

    /// Tests only: crashes the active tab's renderer (`Page.crash`).
    #[cfg(feature = "test-hooks")]
    pub async fn crash_active_tab(&self) {
        let Some(b) = self.running().await else {
            return;
        };
        let session = {
            let s = b.lock();
            s.active
                .as_deref()
                .and_then(|t| s.tab(t))
                .map(|t| t.session.clone())
        };
        if let Some(session) = session {
            b.cdp.send_nowait("Page.crash", json!({}), Some(&session));
        }
    }

    /// Tests only: how many tabs are open.
    #[cfg(feature = "test-hooks")]
    pub async fn tab_count(&self) -> usize {
        match self.running().await {
            Some(b) => b.lock().tabs.len(),
            None => 0,
        }
    }

    // ----- describe helpers -------------------------------------------------------------

    /// The running browser and its active tab, with the main frame's actual origin.
    async fn page(&self) -> Result<(Arc<Browser>, String, String, String), String> {
        let b = self.running().await.ok_or(NOT_OPEN)?;
        let tab = b.lock().active.clone().ok_or(NOT_OPEN)?;
        let info = b.tab_info(&tab)?;
        let (href, origin) = b.actual_origin(&info.session, &tab).await?;
        Ok((b, tab, href, origin))
    }

    fn lookup_ref(b: &Browser, tab: &str, r: &str) -> Result<RefTarget, String> {
        b.lock()
            .tab(tab)
            .and_then(|t| t.refs.get(r).cloned())
            .ok_or_else(|| UNKNOWN_REF.to_string())
    }

    async fn describe_navigate(&self, tool: &str, args: &Map<String, Value>) -> Described {
        let Some(raw) = s(args, "url") else {
            return Described::Invalid("url is required".into());
        };
        let url = match netpolicy::parse_user_url(raw) {
            Ok(u) => u,
            Err(e) => return Described::Invalid(e),
        };
        let shown = one_line(url.as_str(), 500);
        let ips = match netpolicy::check_url(&url, &self.address_policy(), &self.inner.dns).await {
            Ok(ips) => ips,
            Err(why) => return hard_limit(tool, &shown, why),
        };
        let origin = netpolicy::origin_of(&url).unwrap_or_else(|| "null".into());
        let host = policy::sanitize::display_host(&url).unwrap_or_default();
        let mut preview = format!(
            "Open in the agent's browser:\n{shown}\nSite: {origin}\nAddress: {}",
            ips.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        if let Some(b) = self.running().await {
            let current = {
                let st = b.lock();
                st.active
                    .as_deref()
                    .and_then(|t| st.main_frame(t))
                    .map(|f| f.origin.clone())
            };
            if let Some(current) = current.filter(|c| c != "null" && *c != origin) {
                preview.push_str(&format!("\nLeaves the current site {current}."));
            }
        }
        action(
            tool,
            format!("Open {host}"),
            preview,
            &origin,
            Risk::Normal,
            false,
            true,
            json!({"url": url.as_str(), "origin": origin}),
        )
    }

    async fn describe_page_read(&self, tool: &str, args: &Map<String, Value>) -> Described {
        let (b, tab, href, origin) = match self.page().await {
            Ok(p) => p,
            Err(e) => return Described::Invalid(e),
        };
        let mut frame_target: Option<RefTarget> = None;
        let mut frame_origin = origin.clone();
        if tool == "browser_read_text"
            && let Some(r) = s(args, "ref")
        {
            let t = match Self::lookup_ref(&b, &tab, r) {
                Ok(t) => t,
                Err(e) => return Described::Invalid(e),
            };
            match b.actual_origin(&t.session, &t.frame_id).await {
                Ok((_, o)) => frame_origin = o,
                Err(e) => return Described::Invalid(e),
            }
            frame_target = Some(t);
        }
        let host = host_of(&frame_origin);
        let summary = match tool {
            "browser_snapshot" => format!("Read the page on {host}"),
            "browser_find" => format!(
                "Find \"{}\" on {host}",
                crate::snapshot::clean_text(s(args, "query").unwrap_or(""), 60)
            ),
            "browser_screenshot" => format!("Screenshot of {host}"),
            _ => format!("Read text on {host}"),
        };
        let preview = format!(
            "{summary}\nPage: {}\nSite: {frame_origin}",
            one_line(&href, 500)
        );
        let (session, frame) = match &frame_target {
            Some(t) => (t.session.clone(), t.frame_id.clone()),
            None => (
                b.tab_info(&tab).map(|i| i.session).unwrap_or_default(),
                tab.clone(),
            ),
        };
        action(
            tool,
            summary,
            preview,
            &frame_origin,
            Risk::Normal,
            true,
            false,
            json!({
                "epoch": b.epoch, "tab": tab, "origin": frame_origin,
                "session": session, "frame": frame,
                "backend": frame_target.as_ref().map(|t| t.backend),
            }),
        )
    }

    async fn describe_click(&self, tool: &str, args: &Map<String, Value>) -> Described {
        let (b, tab, href, _) = match self.page().await {
            Ok(p) => p,
            Err(e) => return Described::Invalid(e),
        };
        let mut point: Option<(f64, f64)> = None;
        let target = if let Some(r) = s(args, "ref") {
            match Self::lookup_ref(&b, &tab, r) {
                Ok(t) => t,
                Err(e) => return Described::Invalid(e),
            }
        } else if let (Some(x), Some(y)) = (
            args.get("x").and_then(Value::as_f64),
            args.get("y").and_then(Value::as_f64),
        ) {
            let stored = b.lock().tab(&tab).and_then(|t| t.screenshot_dpr);
            let dpr = match stored {
                Some(d) => d,
                None => b.device_pixel_ratio(&tab).await,
            };
            let (cx, cy) = device_to_css(x, y, dpr);
            point = Some((cx, cy));
            match b.element_at(&tab, cx, cy).await {
                Ok(t) => t,
                Err(e) => return Described::Invalid(e),
            }
        } else {
            return Described::Invalid("give either ref, or x and y from a screenshot".into());
        };
        let origin = match b.actual_origin(&target.session, &target.frame_id).await {
            Ok((_, o)) => o,
            Err(e) => return Described::Invalid(e),
        };
        let (info, _) = match b.element_info(&target).await {
            Ok(i) => i,
            Err(e) => return Described::Invalid(e),
        };
        let typed = b.lock().typed_on(&tab);
        let reason = intent::click_intent(&info, typed);
        let what = element_words(&info);
        let host = host_of(&origin);
        let mut preview = format!(
            "Click the {what}\nPage: {}\nSite: {origin}",
            one_line(&href, 500)
        );
        if let Some((x, y)) = point {
            preview.push_str(&format!("\nAt ({x:.0}, {y:.0}) in CSS pixels."));
        }
        if let Some(href) = info.href.as_deref().filter(|_| info.is_link) {
            preview.push_str(&format!("\nLink to: {}", one_line(href, 500)));
        }
        let intent = reason.map(|r| intent_for(r, &tab, Some(&info), &origin));
        let (risk, summary) = match &intent {
            Some(i) => {
                preview.push('\n');
                preview.push_str(&intent_preview(i));
                (
                    Risk::AlwaysAsk,
                    format!(
                        "Click the {what} on {host} (may submit to {})",
                        host_of(&i.target_origin)
                    ),
                )
            }
            None => (Risk::Normal, format!("Click the {what} on {host}")),
        };
        let mut payload = target_json(&target);
        payload["epoch"] = json!(b.epoch);
        payload["tab"] = json!(tab);
        payload["origin"] = json!(origin);
        payload["intent"] = json!(intent);
        payload["point"] = json!(point.map(|(x, y)| [x, y]));
        action(tool, summary, preview, &origin, risk, false, true, payload)
    }

    async fn describe_type(&self, tool: &str, args: &Map<String, Value>) -> Described {
        let (b, tab, href, _) = match self.page().await {
            Ok(p) => p,
            Err(e) => return Described::Invalid(e),
        };
        let (Some(r), Some(text)) = (s(args, "ref"), s(args, "text")) else {
            return Described::Invalid("ref and text are required".into());
        };
        let target = match Self::lookup_ref(&b, &tab, r) {
            Ok(t) => t,
            Err(e) => return Described::Invalid(e),
        };
        let origin = match b.actual_origin(&target.session, &target.frame_id).await {
            Ok((_, o)) => o,
            Err(e) => return Described::Invalid(e),
        };
        let (info, _) = match b.element_info(&target).await {
            Ok(i) => i,
            Err(e) => return Described::Invalid(e),
        };
        if !info.is_editable {
            return Described::Invalid(format!(
                "{r} is a {}, not a text field; use browser_click for it",
                element_words(&info)
            ));
        }
        let submit = args.get("submit").and_then(Value::as_bool).unwrap_or(false);
        let clear = args.get("clear").and_then(Value::as_bool).unwrap_or(true);
        let host = host_of(&origin);
        let what = element_words(&info);
        let mut preview = format!(
            "Type into the {what}{}:\n{text}\nPage: {}\nSite: {origin}\nTyping shows the text to the page, whatever happens next.",
            if info.input_type == "password" {
                " (a password field)"
            } else {
                ""
            },
            one_line(&href, 500)
        );
        let intent = submit.then(|| {
            let reason = if info.in_form {
                SubmitReason::EnterInFormField
            } else {
                SubmitReason::EnterAfterTyping
            };
            intent_for(reason, &tab, Some(&info), &origin)
        });
        let (risk, summary) = match &intent {
            Some(i) => {
                preview.push_str("\nThen presses Enter.\n");
                preview.push_str(&intent_preview(i));
                (
                    Risk::AlwaysAsk,
                    format!("Type into the {what} on {host} and press Enter"),
                )
            }
            None => (Risk::Normal, format!("Type into the {what} on {host}")),
        };
        let mut payload = target_json(&target);
        payload["epoch"] = json!(b.epoch);
        payload["tab"] = json!(tab);
        payload["origin"] = json!(origin);
        payload["text"] = json!(text);
        payload["clear"] = json!(clear);
        payload["intent"] = json!(intent);
        action(tool, summary, preview, &origin, risk, false, true, payload)
    }

    async fn describe_fill(&self, tool: &str, args: &Map<String, Value>) -> Described {
        let (b, tab, href, _) = match self.page().await {
            Ok(p) => p,
            Err(e) => return Described::Invalid(e),
        };
        let Some(fields) = args.get("fields").and_then(Value::as_array) else {
            return Described::Invalid("fields is required".into());
        };
        if fields.is_empty() || fields.len() > 30 {
            return Described::Invalid("give between 1 and 30 fields".into());
        }
        let mut origin: Option<String> = None;
        let mut out_fields = Vec::new();
        let mut lines = Vec::new();
        for f in fields {
            let (Some(r), Some(value)) = (
                f.get("ref").and_then(Value::as_str),
                f.get("value").map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                }),
            ) else {
                return Described::Invalid("every field needs ref and value".into());
            };
            let target = match Self::lookup_ref(&b, &tab, r) {
                Ok(t) => t,
                Err(e) => return Described::Invalid(e),
            };
            let o = match b.actual_origin(&target.session, &target.frame_id).await {
                Ok((_, o)) => o,
                Err(e) => return Described::Invalid(e),
            };
            if origin.as_ref().is_some_and(|prev| *prev != o) {
                return Described::Invalid(
                    "these fields belong to different sites; fill the fields of one site per call"
                        .into(),
                );
            }
            origin = Some(o);
            let (info, _) = match b.element_info(&target).await {
                Ok(i) => i,
                Err(e) => return Described::Invalid(e),
            };
            let Some(kind) = fill_kind(&info) else {
                return Described::Invalid(format!(
                    "{r} is a {}, which cannot be filled; use browser_click",
                    element_words(&info)
                ));
            };
            lines.push(format!("{}: {value}", element_words(&info)));
            let mut entry = target_json(&target);
            entry["ref"] = json!(r);
            entry["value"] = json!(value);
            entry["kind"] = json!(kind);
            out_fields.push(entry);
        }
        let origin = origin.unwrap_or_else(|| "null".into());
        let host = host_of(&origin);
        let preview = format!(
            "Fill {} field(s):\n{}\nPage: {}\nSite: {origin}\nFilling shows the values to the page; it does not submit.",
            lines.len(),
            lines.join("\n"),
            one_line(&href, 500)
        );
        action(
            tool,
            format!("Fill {} field(s) on {host}", lines.len()),
            preview,
            &origin,
            Risk::Normal,
            false,
            true,
            json!({"epoch": b.epoch, "tab": tab, "origin": origin, "fields": out_fields}),
        )
    }

    async fn describe_key(&self, tool: &str, args: &Map<String, Value>) -> Described {
        let (b, tab, main_href, main_origin) = match self.page().await {
            Ok(p) => p,
            Err(e) => return Described::Invalid(e),
        };
        let Some(key) = s(args, "key") else {
            return Described::Invalid("key is required".into());
        };
        let press = match keys::parse_key(key) {
            Ok(k) => k,
            Err(e) => return Described::Invalid(e),
        };
        if let Some(command) = press.clipboard_command() {
            return Described::Invalid(clipboard_refusal(key, command));
        }
        let focused = match b.focused_info(&tab).await {
            Ok(f) => f,
            Err(e) => return Described::Invalid(e),
        };
        // The key goes to the frame holding the focus: that frame's site is what it
        // touches, and what the grant and the call-time check are about.
        let (session, frame) = match &focused {
            Some(f) => (f.session.clone(), f.frame.clone()),
            None => (
                b.tab_info(&tab).map(|i| i.session).unwrap_or_default(),
                tab.clone(),
            ),
        };
        let (href, origin) = if frame == tab {
            (main_href, main_origin)
        } else {
            match b.actual_origin(&session, &frame).await {
                Ok(v) => v,
                Err(e) => return Described::Invalid(e),
            }
        };
        let info = focused.as_ref().map(|f| &f.info);
        let typed = b.lock().typed_on(&tab);
        let mut reason = intent::key_intent(key, info, typed);
        if reason.is_none() && info.is_some_and(|i| i.opaque_frame) {
            // Where the key lands cannot be looked at, so it cannot be scoped either.
            reason = Some(SubmitReason::OpaqueFocus);
        }
        let host = host_of(&origin);
        let shown_key = one_line(key, 40);
        let target_words = info
            .filter(|f| !f.opaque_frame)
            .map(element_words)
            .unwrap_or_else(|| "page".into());
        let mut preview = format!(
            "Press {shown_key} on the {target_words}\nPage: {}\nSite: {origin}",
            one_line(&href, 500)
        );
        if frame != tab {
            preview.push_str("\nThe focus is inside an embedded frame.");
        }
        let intent = reason.map(|r| intent_for(r, &tab, info, &origin));
        let (risk, summary) = match &intent {
            Some(i) => {
                preview.push('\n');
                preview.push_str(&intent_preview(i));
                (
                    Risk::AlwaysAsk,
                    format!("Press {shown_key} on {host} (may submit)"),
                )
            }
            None => (Risk::Normal, format!("Press {shown_key} on {host}")),
        };
        action(
            tool,
            summary,
            preview,
            &origin,
            risk,
            false,
            true,
            json!({
                "epoch": b.epoch, "tab": tab, "origin": origin, "key": key, "intent": intent,
                "session": session, "frame": frame,
            }),
        )
    }

    async fn describe_back(&self, tool: &str) -> Described {
        let b = match self.running().await {
            Some(b) => b,
            None => return Described::Invalid(NOT_OPEN.into()),
        };
        let Some(tab) = b.lock().active.clone() else {
            return Described::Invalid(NOT_OPEN.into());
        };
        match b.previous_entry(&tab).await {
            Err(e) => Described::Invalid(e),
            Ok(Some((entry, url_str))) => {
                let shown = one_line(&url_str, 500);
                let Ok(url) = Url::parse(&url_str) else {
                    return hard_limit(
                        tool,
                        &shown,
                        "that history entry has no valid address".into(),
                    );
                };
                if url.scheme() == "about" {
                    return Described::Invalid("There is no earlier page to go back to.".into());
                }
                if let Err(why) =
                    netpolicy::check_url(&url, &self.address_policy(), &self.inner.dns).await
                {
                    return hard_limit(tool, &shown, why);
                }
                let origin = netpolicy::origin_of(&url).unwrap_or_else(|| "null".into());
                let host = host_of(&origin);
                action(
                    tool,
                    format!("Go back to {host}"),
                    format!("Go back to the previous page:\n{shown}\nSite: {origin}"),
                    &origin,
                    Risk::Normal,
                    false,
                    true,
                    json!({"epoch": b.epoch, "tab": tab, "origin": origin, "entry": entry, "url": url_str}),
                )
            }
            Ok(None) => {
                let opener = {
                    let st = b.lock();
                    st.tab(&tab)
                        .and_then(|t| t.opener.clone())
                        .filter(|o| st.tab(o).is_some())
                        .map(|o| {
                            let origin = st
                                .main_frame(&o)
                                .map(|f| f.origin.clone())
                                .unwrap_or_else(|| "null".into());
                            (o, origin)
                        })
                };
                match opener {
                    Some((opener, origin)) => action(
                        tool,
                        format!("Close this pop-up and return to {}", host_of(&origin)),
                        format!(
                            "Close the pop-up tab and return to the tab that opened it.\nSite: {origin}"
                        ),
                        &origin,
                        Risk::Normal,
                        false,
                        false,
                        json!({"epoch": b.epoch, "tab": tab, "origin": origin, "close": true, "opener": opener}),
                    ),
                    None => Described::Invalid("There is no earlier page to go back to.".into()),
                }
            }
        }
    }
}

/// Builds the tool list.
fn tool_defs() -> Vec<ToolDef> {
    let def = |name: &str, desc: &str, props: Value, required: Vec<&str>, read_only: bool| {
        ToolDef {
            name: name.into(),
            description: desc.into(),
            input_schema: json!({"type": "object", "properties": props, "required": required, "additionalProperties": false}),
            destructive: Some(false),
            read_only: Some(read_only),
        }
    };
    let r = json!({"type": "string", "description": "Element reference from browser_snapshot or browser_find, e.g. \"e12\""});
    vec![
        def(
            "browser_navigate",
            "Open a web page (http or https) in the agent's browser and return a snapshot of it.",
            json!({"url": {"type": "string", "description": "The address, e.g. https://example.com"}}),
            vec!["url"],
            false,
        ),
        def(
            "browser_snapshot",
            "List what is visible on the current page as an accessibility tree, with element references (ref=e12) for click and type. full=true lists the whole page.",
            json!({"full": {"type": "boolean", "description": "Include elements outside the view"}}),
            vec![],
            true,
        ),
        def(
            "browser_find",
            "Find elements on the current page whose text, label or role contains the given words; returns only those, with references.",
            json!({"query": {"type": "string", "minLength": 1}}),
            vec!["query"],
            true,
        ),
        def(
            "browser_read_text",
            "Read the text of the current page (its main content), or of one element. Long text comes in parts: pass offset to continue.",
            json!({"ref": r, "offset": {"type": "integer", "minimum": 0}}),
            vec![],
            true,
        ),
        def(
            "browser_click",
            "Click an element by reference, or a point from the latest browser_screenshot (x, y in screenshot pixels). Clicks that may submit a form always ask the user.",
            json!({"ref": r, "x": {"type": "number", "minimum": 0}, "y": {"type": "number", "minimum": 0}}),
            vec![],
            false,
        ),
        def(
            "browser_type",
            "Type text into a text field. Replaces what the field holds unless clear=false. submit=true presses Enter afterwards (always asks the user).",
            json!({"ref": r, "text": {"type": "string"}, "clear": {"type": "boolean"}, "submit": {"type": "boolean"}}),
            vec!["ref", "text"],
            false,
        ),
        def(
            "browser_fill_form",
            "Fill several fields at once: text fields get the value, checkboxes and radio buttons \"true\" or \"false\", drop-down lists an option's label or value. Does not submit.",
            json!({"fields": {"type": "array", "minItems": 1, "maxItems": 30, "items": {
                "type": "object",
                "properties": {"ref": r, "value": {"type": "string"}},
                "required": ["ref", "value"],
                "additionalProperties": false
            }}}),
            vec!["fields"],
            false,
        ),
        def(
            "browser_press_key",
            "Press a key on the focused element or the page, e.g. Enter, Tab, Escape, ArrowDown, PageDown, Control+a. Enter that may submit a form always asks the user.",
            json!({"key": {"type": "string"}}),
            vec!["key"],
            false,
        ),
        def(
            "browser_screenshot",
            "Take a PNG screenshot of what the current page shows. Coordinates in it can be passed to browser_click.",
            json!({}),
            vec![],
            true,
        ),
        def(
            "browser_back",
            "Go back to the previous page in the current tab (in a pop-up with no history: close it and return to the tab that opened it).",
            json!({}),
            vec![],
            false,
        ),
    ]
}

/// Names of the tools that change the page or send something.
fn checks_typed_intent(tool: &str) -> bool {
    matches!(tool, "browser_click" | "browser_press_key")
}

#[async_trait]
impl BuiltinServer for BrowserServer {
    fn name(&self) -> &str {
        "browser"
    }

    fn tools(&self) -> Vec<ToolDef> {
        tool_defs()
    }

    async fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        _chat: &ChatContext,
    ) -> Described {
        // No side effects here (describe may run for a call that is never approved, and
        // for one chat while another chat's call is running): which chat gets the cards
        // for held requests is decided by the calls that actually run.
        if self.running().await.is_none()
            && tool == "browser_navigate"
            && let Err(e) = self.availability()
        {
            return Described::Invalid(e.for_model());
        }
        match tool {
            "browser_navigate" => self.describe_navigate(tool, args).await,
            "browser_snapshot" | "browser_find" | "browser_read_text" | "browser_screenshot" => {
                self.describe_page_read(tool, args).await
            }
            "browser_click" => self.describe_click(tool, args).await,
            "browser_type" => self.describe_type(tool, args).await,
            "browser_fill_form" => self.describe_fill(tool, args).await,
            "browser_press_key" => self.describe_key(tool, args).await,
            "browser_back" => self.describe_back(tool).await,
            other => Described::Invalid(format!("the browser server has no tool named {other}")),
        }
    }

    async fn call(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        action: &Action,
        ctx: CallContext,
    ) -> ToolOutput {
        // One call at a time; Stop works while waiting for the turn and while the
        // browser starts.
        let _one_at_a_time = tokio::select! {
            guard = self.inner.calls.lock() => guard,
            _ = ctx.cancel.cancelled() => return ToolOutput::error("Stopped by the user."),
        };
        let p = &action.payload;
        let browser = if tool == "browser_navigate" {
            let started = tokio::select! {
                r = self.ensure_browser() => r,
                _ = ctx.cancel.cancelled() => return ToolOutput::error("Stopped by the user."),
            };
            match started {
                Ok(b) => b,
                Err(e) => return ToolOutput::error(e),
            }
        } else {
            match self.running().await {
                Some(b) if Some(b.epoch) == p.get("epoch").and_then(Value::as_u64) => b,
                Some(_) => {
                    return ToolOutput::error(
                        "The browser restarted since this action was shown. Take a new snapshot.",
                    );
                }
                None => return ToolOutput::error(NOT_OPEN),
            }
        };
        browser.set_chat(ctx.chat.chat_id);
        let tab = match payload_str(p, "tab") {
            Some(t) => t.to_string(),
            None => match browser.ensure_tab().await {
                Ok(t) => t,
                Err(e) => return ToolOutput::error(e),
            },
        };
        let intent: Option<SubmitIntent> = p
            .get("intent")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok());
        let expected_navigation = match tool {
            "browser_navigate" | "browser_back" => {
                payload_str(p, "origin").map(|o| (o.to_string(), tab.clone()))
            }
            _ => None,
        };
        let cancel = ctx.cancel.clone();
        browser.begin_call(
            ctx.chat.chat_id,
            ActiveCall {
                chat_id: ctx.chat.chat_id,
                tool: tool.to_string(),
                tab: tab.clone(),
                intent,
                expected_navigation,
                cancel: cancel.clone(),
            },
        );
        let budget = budget_chars(ctx.chat.result_cap_tokens);
        let work = self.run(&browser, tool, args, action, &tab, budget, &cancel);
        let result = tokio::select! {
            r = work => r,
            _ = cancel.cancelled() => Err(Stopped),
        };
        browser.end_call();
        let mut output = match result {
            Ok(out) => out,
            Err(Stopped) => {
                // Stop: fail what is held for this chat (and this tab) and stop loading,
                // without waiting for the browser.
                browser.release_held_on_stop(&tab, ctx.chat.chat_id);
                if let Ok(info) = browser.tab_info(&tab) {
                    browser
                        .cdp
                        .send_nowait("Page.stopLoading", json!({}), Some(&info.session));
                }
                ToolOutput::error("Stopped by the user.")
            }
        };
        let notes = browser.take_notes();
        if !notes.is_empty() {
            let text = format!(
                "\nNotes from the browser:\n{}",
                notes
                    .iter()
                    .map(|n| format!("- {n}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            match output.content.iter_mut().find_map(|c| match c {
                Content::Text { text } => Some(text),
                Content::Image { .. } => None,
            }) {
                Some(t) => t.push_str(&text),
                None => output.content.push(Content::Text { text }),
            }
        }
        output
    }
}

/// The call was stopped.
struct Stopped;

impl BrowserServer {
    /// Runs one approved action. Every branch re-checks what was approved immediately
    /// before its side effect.
    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        b: &Arc<Browser>,
        tool: &str,
        args: &Map<String, Value>,
        action: &Action,
        tab: &str,
        budget: usize,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput, Stopped> {
        let p = &action.payload;
        let approved_origin = payload_str(p, "origin").unwrap_or("null").to_string();
        // 13.3: each read, click and type is checked against the frame's actual origin.
        let check_origin = |session: String, frame: String| {
            let approved = approved_origin.clone();
            async move {
                let (href, actual) = b.actual_origin(&session, &frame).await?;
                if actual != approved {
                    return Err(format!(
                        "Refused: the page is now on {actual} ({}), not {approved} as approved. Take a new snapshot and try again.",
                        one_line(&href, 200)
                    ));
                }
                Ok(())
            }
        };
        // Navigating reloads a crashed tab; every other tool refuses on one.
        let main_session = match if tool == "browser_navigate" {
            b.tab_session(tab)
        } else {
            b.tab_info(tab).map(|i| i.session)
        } {
            Ok(session) => session,
            Err(e) => return Ok(ToolOutput::error(e)),
        };
        let out: Result<ToolOutput, String> = async {
            match tool {
                "browser_navigate" => {
                    let url_str = payload_str(p, "url").ok_or("internal error: no URL was approved")?;
                    let url = Url::parse(url_str).map_err(|e| e.to_string())?;
                    // The address may resolve differently now; check again right before.
                    netpolicy::check_url(&url, &self.address_policy(), &self.inner.dns).await?;
                    let note = b.navigate(tab, url.as_str(), cancel).await?;
                    let snap = b.snapshot(tab, false, budget).await?;
                    let mut text = String::new();
                    if let Some(n) = note {
                        text.push_str(&n);
                        text.push('\n');
                    }
                    text.push_str(&snap);
                    Ok(ToolOutput::text(text))
                }
                "browser_snapshot" | "browser_find" | "browser_screenshot" => {
                    check_origin(main_session.clone(), tab.to_string()).await?;
                    match tool {
                        "browser_snapshot" => {
                            let full = args.get("full").and_then(Value::as_bool).unwrap_or(false);
                            Ok(ToolOutput::text(b.snapshot(tab, full, budget).await?))
                        }
                        "browser_find" => {
                            let q = s(args, "query").unwrap_or("");
                            Ok(ToolOutput::text(b.find(tab, q, budget).await?))
                        }
                        _ => {
                            let (data, dpr) = b.screenshot(tab).await?;
                            let size = crate::page::png_size(&data)
                                .map(|(w, h)| format!("{w}×{h} pixels"))
                                .unwrap_or_else(|| "an image".into());
                            let url = b.tab_info(tab).map(|i| i.url).unwrap_or_default();
                            Ok(ToolOutput {
                                content: vec![
                                    Content::Text {
                                        text: format!(
                                            "Screenshot of {} ({size}, device pixel ratio {dpr}). To click a point, pass its x and y in these image pixels to browser_click.",
                                            crate::snapshot::clean_text(&url, 300)
                                        ),
                                    },
                                    Content::Image {
                                        mime: "image/png".into(),
                                        data_base64: data,
                                    },
                                ],
                                ..Default::default()
                            })
                        }
                    }
                }
                "browser_read_text" => {
                    let session = payload_str(p, "session").unwrap_or(&main_session).to_string();
                    let frame = payload_str(p, "frame").unwrap_or(tab).to_string();
                    check_origin(session, frame).await?;
                    let target = target_from(p);
                    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
                    Ok(ToolOutput::text(b.read_text(tab, target.as_ref(), offset, budget).await?))
                }
                "browser_click" => {
                    let target = target_from(p).ok_or("internal error: no element was approved")?;
                    check_origin(target.session.clone(), target.frame_id.clone()).await?;
                    let (info, _) = b.element_info(&target).await?;
                    self.recheck_intent(b, tool, &info, tab, action)?;
                    let (x, y) = match p.get("point").and_then(Value::as_array) {
                        Some(pt) if pt.len() == 2 => {
                            let (x, y) = (pt[0].as_f64().unwrap_or(0.0), pt[1].as_f64().unwrap_or(0.0));
                            let now = b.element_at(tab, x, y).await?;
                            if now != target {
                                return Err("Refused: a different element is at that point now. Take a new screenshot.".into());
                            }
                            (x, y)
                        }
                        _ => {
                            let (x, y) = b.point_of(&target).await?;
                            b.check_not_covered(tab, &target, x, y).await?;
                            (x, y)
                        }
                    };
                    let (nav, stop) = seqs(b, tab);
                    b.click_element(tab, &target, x, y).await?;
                    let navigated = b.settle_after_action(tab, nav, stop, cancel).await?;
                    self.after_action(b, format!("Clicked the {}.", element_words(&info)), navigated, budget)
                        .await
                }
                "browser_type" => {
                    let target = target_from(p).ok_or("internal error: no field was approved")?;
                    check_origin(target.session.clone(), target.frame_id.clone()).await?;
                    let text = payload_str(p, "text").unwrap_or("");
                    let clear = p.get("clear").and_then(Value::as_bool).unwrap_or(true);
                    let (info, _) = b.element_info(&target).await?;
                    if !info.is_editable {
                        return Err("Refused: that element is no longer a text field. Take a new snapshot.".into());
                    }
                    b.type_into(tab, &target, text, clear).await?;
                    mark_typed(b, tab);
                    let mut msg = format!(
                        "Typed {} character(s) into the {}.",
                        text.chars().count(),
                        element_words(&info)
                    );
                    if p.get("intent").is_some_and(|v| !v.is_null()) {
                        let (nav, stop) = seqs(b, tab);
                        b.press(tab, &keys::parse_key("Enter")?).await?;
                        let navigated = b.settle_after_action(tab, nav, stop, cancel).await?;
                        msg.push_str(" Pressed Enter.");
                        return self.after_action(b, msg, navigated, budget).await;
                    }
                    Ok(ToolOutput::text(msg))
                }
                "browser_fill_form" => {
                    let fields = p.get("fields").and_then(Value::as_array).cloned().unwrap_or_default();
                    let mut done = Vec::new();
                    for f in &fields {
                        let target = target_from(f).ok_or("internal error: a field was not approved")?;
                        check_origin(target.session.clone(), target.frame_id.clone()).await?;
                        let value = f.get("value").and_then(Value::as_str).unwrap_or("");
                        let r = f.get("ref").and_then(Value::as_str).unwrap_or("?");
                        let kind = f.get("kind").and_then(Value::as_str);
                        // The page may have turned the field into something else since the
                        // card was shown (a checkbox into a submit button, say).
                        let (info, checked) = b.element_info(&target).await?;
                        if !still_fillable_as(kind, &info) {
                            return Err(format!(
                                "Refused: {r} is now a {}, not the field that was approved. Take a new snapshot.",
                                element_words(&info)
                            ));
                        }
                        match kind {
                            Some("check") => {
                                let want = matches!(value.trim().to_ascii_lowercase().as_str(), "true" | "on" | "yes" | "1" | "checked");
                                if checked != Some(want) {
                                    let (x, y) = b.point_of(&target).await?;
                                    b.check_not_covered(tab, &target, x, y).await?;
                                    b.click_element(tab, &target, x, y).await?;
                                }
                                done.push(format!("{r}: {}", if want { "checked" } else { "unchecked" }));
                            }
                            Some("select") => {
                                b.select_option(&target, value).await?;
                                done.push(format!("{r}: chose \"{}\"", crate::snapshot::clean_text(value, 60)));
                            }
                            _ => {
                                b.type_into(tab, &target, value, true).await?;
                                done.push(format!("{r}: typed {} character(s)", value.chars().count()));
                            }
                        }
                    }
                    mark_typed(b, tab);
                    Ok(ToolOutput::text(format!("Filled {} field(s):\n{}", done.len(), done.join("\n"))))
                }
                "browser_press_key" => {
                    let session = payload_str(p, "session").unwrap_or(&main_session).to_string();
                    let frame = payload_str(p, "frame").unwrap_or(tab).to_string();
                    check_origin(session.clone(), frame.clone()).await?;
                    let key = payload_str(p, "key").unwrap_or("");
                    let press = keys::parse_key(key)?;
                    if let Some(command) = press.clipboard_command() {
                        return Err(clipboard_refusal(key, command));
                    }
                    // The focus may have moved since the card was shown.
                    let focused = b.focused_info(tab).await?;
                    let (now_session, now_frame) = focused
                        .as_ref()
                        .map(|f| (f.session.as_str(), f.frame.as_str()))
                        .unwrap_or((main_session.as_str(), tab));
                    if now_session != session || now_frame != frame {
                        return Err("Refused: the focus moved to another frame since this was approved. Describe the key press again.".into());
                    }
                    let info = focused.as_ref().map(|f| &f.info);
                    let typed = b.lock().typed_on(tab);
                    let may_submit = intent::key_intent(key, info, typed).is_some()
                        || info.is_some_and(|i| i.opaque_frame);
                    if action.risk == Risk::Normal && may_submit {
                        return Err("Refused: the focus moved to something that may submit a form since this was approved. Describe the key press again.".into());
                    }
                    let (nav, stop) = seqs(b, tab);
                    b.press(tab, &press).await?;
                    // A character typed into a field is typing (D8: a button or Enter
                    // after typing on the page asks).
                    if press.text.is_some() && info.is_some_and(|i| i.is_editable || i.opaque_frame) {
                        mark_typed(b, tab);
                    }
                    let navigated = b.settle_after_action(tab, nav, stop, cancel).await?;
                    self.after_action(b, format!("Pressed {}.", one_line(key, 40)), navigated, budget).await
                }
                "browser_back" => {
                    if p.get("close").and_then(Value::as_bool) == Some(true) {
                        let opener = payload_str(p, "opener").unwrap_or("").to_string();
                        b.cdp
                            .call("Target.closeTarget", json!({"targetId": tab}), None, crate::session::CMD)
                            .await
                            .map_err(|e| e.to_string())?;
                        let _ = b
                            .wait_for(
                                |s| s.tab(tab).is_none(),
                                std::time::Instant::now() + std::time::Duration::from_secs(5),
                                cancel,
                            )
                            .await;
                        {
                            let mut st = b.lock();
                            if st.tab(&opener).is_some() {
                                st.active = Some(opener.clone());
                            }
                        }
                        let snap = b.snapshot(&opener, false, budget).await?;
                        return Ok(ToolOutput::text(format!("Closed the pop-up.\n{snap}")));
                    }
                    let entry = p.get("entry").and_then(Value::as_i64).ok_or("internal error: no history entry")?;
                    let approved_url = payload_str(p, "url").unwrap_or("");
                    match b.previous_entry(tab).await? {
                        Some((id, url)) if id == entry && url == approved_url => {}
                        _ => return Err("Refused: the tab's history changed since this was approved. Describe browser_back again.".into()),
                    }
                    b.go_to_entry(tab, entry, cancel).await?;
                    let snap = b.snapshot(tab, false, budget).await?;
                    Ok(ToolOutput::text(format!("Went back.\n{snap}")))
                }
                other => Err(format!("the browser server has no tool named {other}")),
            }
        }
        .await;
        match out {
            Ok(o) => Ok(o),
            Err(e) if e == "Stopped by the user." => Err(Stopped),
            Err(e) => Ok(ToolOutput::error(e)),
        }
    }

    /// A click approved without a card must still not be a submission now: the page may
    /// have turned the element into a submit button since the card was shown.
    fn recheck_intent(
        &self,
        b: &Browser,
        tool: &str,
        info: &ElementInfo,
        tab: &str,
        action: &Action,
    ) -> Result<(), String> {
        if !checks_typed_intent(tool) || action.risk == Risk::AlwaysAsk {
            return Ok(());
        }
        let typed = b.lock().typed_on(tab);
        if intent::click_intent(info, typed).is_some() {
            return Err("Refused: that element may submit a form now (it changed since this was approved). Describe the click again.".into());
        }
        Ok(())
    }

    async fn after_action(
        &self,
        b: &Arc<Browser>,
        mut msg: String,
        navigated: bool,
        budget: usize,
    ) -> Result<ToolOutput, String> {
        let tab = b.lock().active.clone().ok_or("no browser tab is open")?;
        if navigated {
            let url = b.tab_info(&tab).map(|i| i.url).unwrap_or_default();
            msg.push_str(&format!(
                " The page changed to {}.\n",
                crate::snapshot::clean_text(&url, 300)
            ));
            msg.push_str(&b.snapshot(&tab, false, budget).await?);
        } else {
            msg.push_str(" Take a browser_snapshot to see the result.");
        }
        Ok(ToolOutput::text(msg))
    }
}

fn seqs(b: &Browser, tab: &str) -> (u64, u64) {
    b.lock()
        .tab(tab)
        .map(|t| (t.nav_seq, t.stop_seq))
        .unwrap_or((0, 0))
}

fn mark_typed(b: &Browser, tab: &str) {
    let mut s = b.lock();
    let loader = s.main_frame(tab).map(|f| f.loader_id.clone());
    if let Some(t) = s.tab_mut(tab) {
        t.typed_loader = loader;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_the_fixed_ten() {
        let names: Vec<String> = tool_defs().into_iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            [
                "browser_navigate",
                "browser_snapshot",
                "browser_find",
                "browser_read_text",
                "browser_click",
                "browser_type",
                "browser_fill_form",
                "browser_press_key",
                "browser_screenshot",
                "browser_back"
            ]
        );
        for t in tool_defs() {
            assert_eq!(t.input_schema["type"], "object");
            assert_eq!(t.input_schema["additionalProperties"], false);
            assert!(t.description.len() < 300, "{}", t.name);
        }
    }

    #[test]
    fn read_only_marks() {
        let ro: Vec<String> = tool_defs()
            .into_iter()
            .filter(|t| t.read_only == Some(true))
            .map(|t| t.name)
            .collect();
        assert_eq!(
            ro,
            [
                "browser_snapshot",
                "browser_find",
                "browser_read_text",
                "browser_screenshot"
            ]
        );
    }

    #[test]
    fn intents_use_the_form_target_when_there_is_one() {
        let form = ElementInfo {
            in_form: true,
            form_method: Some("post".into()),
            form_action: Some("https://pay.example/charge".into()),
            ..Default::default()
        };
        let i = intent_for(
            SubmitReason::SubmitControl,
            "T",
            Some(&form),
            "https://shop.example",
        );
        assert_eq!(i.target_origin, "https://pay.example");
        assert_eq!(i.method.as_deref(), Some("POST"));
        let loose = ElementInfo::default();
        let i = intent_for(
            SubmitReason::ButtonAfterTyping,
            "T",
            Some(&loose),
            "https://shop.example",
        );
        assert_eq!(i.target_origin, "https://shop.example");
        assert_eq!(i.method, None);
        let dialog = ElementInfo {
            in_form: true,
            form_method: Some("dialog".into()),
            ..Default::default()
        };
        assert_eq!(
            intent_for(
                SubmitReason::SubmitControl,
                "T",
                Some(&dialog),
                "https://a.example"
            )
            .method,
            None
        );
        let p = intent_preview(&i);
        assert!(
            p.contains("https://shop.example") && p.contains("any method"),
            "{p}"
        );
    }

    #[test]
    fn acting_in_a_frame_without_a_site_always_asks() {
        let act = |origin: &str, read_only: bool| {
            let Described::Action(a) = action(
                "browser_click",
                "Click".into(),
                "Click the button".into(),
                origin,
                Risk::Normal,
                read_only,
                !read_only,
                Value::Null,
            ) else {
                panic!("expected an action");
            };
            a
        };
        let a = act("null", false);
        assert_eq!(a.risk, Risk::AlwaysAsk);
        assert!(a.preview.contains("no site of its own"), "{}", a.preview);
        assert_eq!(act("https://a.example", false).risk, Risk::Normal);
        assert_eq!(act("null", true).risk, Risk::Normal, "reads are unchanged");
    }

    #[test]
    fn fills_are_only_for_fields_and_rechecked_by_kind() {
        let checkbox = ElementInfo {
            tag: "input".into(),
            input_type: "checkbox".into(),
            ..Default::default()
        };
        let text = ElementInfo {
            tag: "input".into(),
            input_type: "text".into(),
            is_editable: true,
            ..Default::default()
        };
        let select = ElementInfo {
            tag: "select".into(),
            ..Default::default()
        };
        let submit = ElementInfo {
            tag: "input".into(),
            input_type: "submit".into(),
            is_submit_control: true,
            is_button_like: true,
            in_form: true,
            ..Default::default()
        };
        let button = ElementInfo {
            tag: "button".into(),
            input_type: "button".into(),
            is_button_like: true,
            ..Default::default()
        };
        assert_eq!(fill_kind(&checkbox), Some("check"));
        assert_eq!(fill_kind(&text), Some("text"));
        assert_eq!(fill_kind(&select), Some("select"));
        assert_eq!(fill_kind(&submit), None);
        assert_eq!(fill_kind(&button), None);
        assert!(still_fillable_as(Some("check"), &checkbox));
        // The page turned the approved checkbox into a submit button, or a field into a
        // list: refused.
        assert!(!still_fillable_as(Some("check"), &submit));
        assert!(!still_fillable_as(Some("text"), &select));
        assert!(!still_fillable_as(None, &text));
    }

    #[test]
    fn hard_limits_carry_the_reason_and_no_scope_of_a_real_site() {
        let Described::Action(a) = hard_limit(
            "browser_navigate",
            "file:///etc/passwd",
            "file: URLs are blocked".into(),
        ) else {
            panic!("expected an action");
        };
        assert_eq!(a.hard_limit.as_deref(), Some("file: URLs are blocked"));
        assert_eq!(
            a.scope,
            Scope::Site {
                origin: "blocked".into()
            }
        );
    }

    #[test]
    fn element_words_are_short_and_escaped() {
        let b = ElementInfo {
            is_button_like: true,
            label: "Pay \"now\"".into(),
            ..Default::default()
        };
        assert_eq!(element_words(&b), "button \"Pay \\\"now\\\"\"");
        assert_eq!(element_words(&ElementInfo::default()), "element");
    }

    #[test]
    fn action_text_goes_through_the_sanitiser() {
        let Described::Action(a) = action(
            "browser_click",
            "Click\u{202E}evil\nline".into(),
            "pre\u{200B}view".into(),
            "https://a.example",
            Risk::Normal,
            false,
            true,
            Value::Null,
        ) else {
            panic!("expected an action");
        };
        assert_eq!(a.summary, "Click\\u{202E}evil\\nline");
        assert_eq!(a.preview, "pre\\u{200B}view");
        assert_eq!(
            a.scope,
            Scope::Site {
                origin: "https://a.example".into()
            }
        );
    }
}
