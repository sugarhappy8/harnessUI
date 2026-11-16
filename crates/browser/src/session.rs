//! One running browser: its targets, frames and held requests, and the event loop that
//! enforces where the agent may go (ARCHITECTURE.md 9.3, 13.3, D8).
//!
//! - Every page, pop-up and cross-site iframe is attached as a flattened session with
//!   `waitForDebuggerOnStart`, so nothing runs before the Fetch domain is enabled on it.
//! - Every document request (main frame, sub-frames, pop-ups, each redirect hop) is
//!   paused and checked for scheme and for the address class of the resolved host, then
//!   for the D8 and site rules ([`crate::intent::decide`]). Held requests are always
//!   released: continued on approval, failed on Deny, Stop, timeout, crash or shutdown.
//! - File choosers are cancelled and downloads denied, each with a tool message.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use runtime::spawn::ManagedChild;
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::approval::{HeldNavigation, NavigationApprover};
use crate::cdp::{Cdp, CdpError, Event};
use crate::config::BrowserConfig;
use crate::intent::{self, CallView, HoldKind, PausedRequest, SubmitIntent, Verdict};
use crate::netpolicy::{self, AddressPolicy, DnsCache};

/// Timeout for ordinary CDP commands.
pub(crate) const CMD: Duration = Duration::from_secs(15);

/// [0.0.7 P7] The policy the capture's tab gets while a capture runs: the page's own
/// requests over HTTP(S) go on (each is checked), WebSocket connections, which the checks
/// cannot see, are refused.
pub(crate) const CAPTURE_POLICY: &str = "connect-src http: https: blob: data:";
/// [0.0.7 P7] A document larger than this keeps its own response (and no added policy).
const MAX_FULFILLED_BASE64: usize = 24 * 1024 * 1024;

/// How long a browser that was just started may take to answer for the first time. A
/// first start on a busy machine with a cold disk has been seen to take more than 20
/// seconds; a browser that answers sooner is used at once, so this only bounds a failure.
const STARTUP: Duration = Duration::from_secs(60);

/// How long a page-reported form submission is remembered for matching its request.
// TODO(M6): Chrome reports no `Page.frameRequestedNavigation` for a GET form that opens a
// new tab (`target=_blank`), so such a submission is not recognised as one: a GET to the
// granted site in the new tab goes through without a card (one that leaves the site is
// held as leaving it; POST is always held). A submit control is still always-ask by
// intent; the gap is a script-submitted GET form, like the GET links D8 lists.
const FORM_SUBMISSION_WINDOW: Duration = Duration::from_secs(10);
/// Notes kept for the next tool result at most.
const MAX_NOTES: usize = 20;

/// What kind of target a session belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetKind {
    /// A tab or pop-up.
    Page,
    /// A cross-site (out-of-process) iframe.
    Frame,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionInfo {
    pub kind: TargetKind,
    /// The tab (page target id) the session belongs to.
    pub tab: String,
    /// Set-up is complete: the session's requests are paused and checked (`Fetch.enable`
    /// was answered), it reports its page events, and its frame tree is recorded. A tab
    /// is attached before that; no tool call may use it in between.
    pub ready: bool,
}

/// Where a reference points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefTarget {
    pub session: String,
    pub frame_id: String,
    pub backend: i64,
}

/// Element references of one tab (`e1`, `e2`…), stable while the document lives.
#[derive(Debug, Default)]
pub(crate) struct RefTable {
    by_key: HashMap<(String, i64), String>,
    by_ref: HashMap<String, RefTarget>,
    next: u64,
}

impl RefTable {
    pub fn alloc(&mut self, session: &str, frame_id: &str, backend: i64) -> String {
        let key = (session.to_string(), backend);
        if let Some(r) = self.by_key.get(&key) {
            return r.clone();
        }
        self.next += 1;
        let r = format!("e{}", self.next);
        self.by_key.insert(key, r.clone());
        self.by_ref.insert(
            r.clone(),
            RefTarget {
                session: session.to_string(),
                frame_id: frame_id.to_string(),
                backend,
            },
        );
        r
    }

    pub fn get(&self, r: &str) -> Option<&RefTarget> {
        self.by_ref.get(r.trim())
    }

    pub fn clear(&mut self) {
        self.by_key.clear();
        self.by_ref.clear();
    }
}

#[derive(Debug)]
pub(crate) struct Tab {
    pub target_id: String,
    pub session: String,
    pub opener: Option<String>,
    /// The site the tab's grant covers: the origin of the last approved navigation.
    pub granted_site: Option<String>,
    /// The agent typed on the document with this loader id.
    pub typed_loader: Option<String>,
    pub crashed: bool,
    pub refs: RefTable,
    /// devicePixelRatio when the last screenshot was taken.
    pub screenshot_dpr: Option<f64>,
    /// Bumped when the main frame starts or requests a navigation.
    pub nav_seq: u64,
    /// Bumped when the main frame stops loading.
    pub stop_seq: u64,
    /// Loader id of the last main-frame `load` lifecycle event.
    pub loaded_loader: Option<String>,
    /// The main frame is loading (between `frameStartedLoading` and `frameStoppedLoading`).
    pub loading: bool,
    /// When the main frame last reported a navigation event.
    pub last_nav_event: Option<Instant>,
}

#[derive(Debug, Clone)]
pub(crate) struct Frame {
    pub session: String,
    pub parent: Option<String>,
    pub tab: String,
    pub url: String,
    pub origin: String,
    pub loader_id: String,
    pub blocked: Option<String>,
    /// Order of appearance, for listing frames in document order.
    pub order: u64,
}

#[derive(Debug)]
struct Held {
    session: String,
    request_id: String,
    tab: String,
    /// The chat whose card it raised.
    chat_id: i64,
    token: CancellationToken,
}

/// Whom a held request is attributed to.
#[derive(Debug, Clone)]
pub(crate) struct HoldOwner {
    /// The chat that sees the card.
    pub chat_id: i64,
    /// The tool whose call caused it, when it is that call's tab or a pop-up it opened.
    pub during_tool: Option<String>,
    /// Stop of the call in progress fails it.
    pub call_cancel: Option<CancellationToken>,
}

/// The call in progress.
#[derive(Debug, Clone)]
pub(crate) struct ActiveCall {
    /// The chat the call belongs to.
    pub chat_id: i64,
    pub tool: String,
    pub tab: String,
    pub intent: Option<SubmitIntent>,
    pub expected_navigation: Option<(String, String)>,
    pub cancel: CancellationToken,
}

// TODO(M6): one browser and one profile serve every chat; tabs, cookies and the notes
// below are shared between chats. A browser context per chat (Target.createBrowserContext)
// would separate them.
#[derive(Debug, Default)]
pub(crate) struct State {
    pub tabs: Vec<Tab>,
    pub active: Option<String>,
    pub sessions: HashMap<String, SessionInfo>,
    pub frames: HashMap<String, Frame>,
    frame_order: u64,
    held: HashMap<String, Held>,
    pub notes: Vec<String>,
    pub call: Option<ActiveCall>,
    pub chat_id: i64,
    form_submissions: Vec<(String, String, Instant)>,
    released: VecDeque<String>,
    /// Recent block reasons: (URL, reason, when).
    blocks: VecDeque<(String, String, Instant)>,
    /// [0.0.7 P7] While `browser_capture_site` runs: which requests passed the checks and
    /// what each became (`crate::capture::netlog`).
    pub capture: Option<crate::capture::netlog::CaptureLog>,
}

impl State {
    pub fn tab(&self, id: &str) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.target_id == id)
    }

    /// The tab whose requests come from `session`: its own, or, for a worker a capture
    /// watches (its requests are paused on its own session), the capture's tab.
    pub(crate) fn tab_of(&self, session: &str) -> Option<String> {
        if let Some(info) = self.sessions.get(session) {
            return Some(info.tab.clone());
        }
        self.capture
            .as_ref()
            .filter(|c| c.workers.iter().any(|w| w == session))
            .map(|c| c.tab.clone())
    }

    pub fn tab_mut(&mut self, id: &str) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.target_id == id)
    }

    pub fn note(&mut self, note: String) {
        if !self.notes.contains(&note) && self.notes.len() < MAX_NOTES {
            self.notes.push(note);
        }
    }

    fn mark_released(&mut self, request_id: &str) {
        self.released.push_back(request_id.to_string());
        while self.released.len() > 64 {
            self.released.pop_front();
        }
    }

    fn record_block(&mut self, url: &str, why: &str) {
        self.blocks
            .push_back((url.to_string(), why.to_string(), Instant::now()));
        while self.blocks.len() > 20 {
            self.blocks.pop_front();
        }
    }

    /// The most recent block reason recorded since `since`.
    pub fn block_since(&self, since: Instant) -> Option<String> {
        self.blocks
            .iter()
            .rev()
            .find(|(_, _, at)| *at >= since)
            .map(|(url, why, _)| format!("{} ({why})", policy::sanitize::one_line(url, 200)))
    }

    pub fn held_for_tab(&self, tab: &str) -> usize {
        self.held.values().filter(|h| h.tab == tab).count()
    }

    /// Tests only (the `test-hooks` feature): requests held right now.
    #[cfg(feature = "test-hooks")]
    pub fn held_count(&self) -> usize {
        self.held.len()
    }

    /// Who a request held for `tab` belongs to. While a call runs (calls run one at a
    /// time) every hold goes to that call's chat and is failed by its Stop, whatever tab
    /// it came from; outside a call it goes to the chat that ran the last call.
    pub(crate) fn hold_owner(&self, tab: &str) -> HoldOwner {
        match &self.call {
            Some(c) => {
                let related = c.tab == tab
                    || self.tab(tab).and_then(|t| t.opener.as_deref()) == Some(c.tab.as_str());
                HoldOwner {
                    chat_id: c.chat_id,
                    during_tool: related.then(|| c.tool.clone()),
                    call_cancel: Some(c.cancel.clone()),
                }
            }
            None => HoldOwner {
                chat_id: self.chat_id,
                during_tool: None,
                call_cancel: None,
            },
        }
    }

    /// Cancels every hold of `tab` or of `chat_id` (Stop).
    fn cancel_holds_on_stop(&self, tab: &str, chat_id: i64) -> usize {
        let mut n = 0;
        for held in self
            .held
            .values()
            .filter(|h| h.tab == tab || h.chat_id == chat_id)
        {
            held.token.cancel();
            n += 1;
        }
        n
    }

    /// Frames of `tab` other than its main frame, in order of appearance.
    pub fn child_frames(&self, tab: &str) -> Vec<(String, Frame)> {
        let mut frames: Vec<(String, Frame)> = self
            .frames
            .iter()
            .filter(|(id, f)| f.tab == tab && id.as_str() != tab && f.parent.is_some())
            .map(|(id, f)| (id.clone(), f.clone()))
            .collect();
        frames.sort_by_key(|(_, f)| f.order);
        frames
    }

    /// The tab's main frame.
    pub fn main_frame(&self, tab: &str) -> Option<&Frame> {
        self.frames.get(tab)
    }

    /// Whether the agent typed on the tab's current document.
    pub fn typed_on(&self, tab: &str) -> bool {
        let Some(t) = self.tab(tab) else {
            return false;
        };
        match (&t.typed_loader, self.main_frame(tab)) {
            (Some(typed), Some(frame)) => *typed == frame.loader_id,
            _ => false,
        }
    }
}

/// [0.0.7 P7] Whether a shared or service worker runs a web page's script (and not one of
/// the browser's own, such as a built-in extension's `chrome-extension:` worker).
fn web_worker(url: &str) -> bool {
    ["http:", "https:", "data:", "blob:"]
        .iter()
        .any(|scheme| url.get(..scheme.len()).is_some_and(|u| u.eq_ignore_ascii_case(scheme)))
}

/// The running browser.
pub(crate) struct Browser {
    pub cdp: Cdp,
    child: tokio::sync::Mutex<Option<ManagedChild>>,
    state: Mutex<State>,
    changed: watch::Sender<u64>,
    pub shutdown: CancellationToken,
    pub config: Arc<BrowserConfig>,
    approver: Arc<dyn NavigationApprover>,
    dns: Arc<DnsCache>,
    policy: AddressPolicy,
    pub stderr_tail: Arc<Mutex<String>>,
    /// Distinguishes this browser from a later restart, so actions described against
    /// an earlier one are refused.
    pub epoch: u64,
    /// [0.0.7 P7] Every request of every target is paused and checked, not only page loads:
    /// on while `browser_capture_site` runs, so every file it saves came through the checks.
    capture_checks: std::sync::atomic::AtomicBool,
}

impl Browser {
    /// Connects to a launched browser, sets up auto-attach and download denial, and waits
    /// for the first tab.
    pub async fn connect(
        launched: crate::launch::Launched,
        config: Arc<BrowserConfig>,
        approver: Arc<dyn NavigationApprover>,
        dns: Arc<DnsCache>,
        epoch: u64,
    ) -> Result<Arc<Self>, String> {
        let crate::launch::Launched {
            child,
            transport,
            stderr_tail,
        } = launched;
        let (cdp, events) = Cdp::start(transport);
        let policy = AddressPolicy {
            allow_private: config.allow_private_networks,
            loopback_ports: config.loopback_ports(),
        };
        let browser = Arc::new(Self {
            cdp,
            child: tokio::sync::Mutex::new(Some(child)),
            state: Mutex::new(State::default()),
            changed: watch::channel(0).0,
            shutdown: CancellationToken::new(),
            config,
            approver,
            dns,
            policy,
            stderr_tail,
            epoch,
            capture_checks: std::sync::atomic::AtomicBool::new(false),
        });
        tokio::spawn(Self::run_events(Arc::downgrade(&browser), events));
        let setup = async {
            browser
                .cdp
                .call("Browser.getVersion", json!({}), None, STARTUP)
                .await?;
            browser
                .cdp
                .call(
                    "Browser.setDownloadBehavior",
                    json!({"behavior": "deny", "eventsEnabled": true}),
                    None,
                    CMD,
                )
                .await?;
            browser
                .cdp
                .call(
                    "Target.setAutoAttach",
                    json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true}),
                    None,
                    CMD,
                )
                .await?;
            Ok::<(), CdpError>(())
        };
        if let Err(e) = setup.await {
            let tail = browser
                .stderr_tail
                .lock()
                .map(|t| t.clone())
                .unwrap_or_default();
            browser.close().await;
            let last: String = tail.lines().rev().take(5).collect::<Vec<_>>().join(" | ");
            return Err(format!(
                "the browser did not answer on its debugging channel ({e}). {last}"
            ));
        }
        if browser.ensure_tab().await.is_err() {
            browser.close().await;
            return Err("the browser started but opened no tab".into());
        }
        Ok(browser)
    }

    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn bump(&self) {
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
    }

    /// The active tab, opening one if none is left.
    pub async fn ensure_tab(&self) -> Result<String, String> {
        if let Some(id) = self.lock().active.clone() {
            return Ok(id);
        }
        let ready = self
            .wait_for(
                |s| s.active.is_some(),
                Instant::now() + Duration::from_secs(5),
                &CancellationToken::new(),
            )
            .await;
        if ready == Wait::Done
            && let Some(id) = self.lock().active.clone()
        {
            return Ok(id);
        }
        self.cdp
            .call(
                "Target.createTarget",
                json!({"url": "about:blank"}),
                None,
                CMD,
            )
            .await
            .map_err(|e| e.to_string())?;
        self.wait_for(
            |s| s.active.is_some(),
            Instant::now() + Duration::from_secs(10),
            &CancellationToken::new(),
        )
        .await;
        self.lock()
            .active
            .clone()
            .ok_or_else(|| "no browser tab is open".into())
    }

    /// Waits until `tab` is set up. A tab is usable for a call only then: a navigation
    /// sent before its requests are checked would not be paused (neither its redirects
    /// nor the site it lands on would be checked), and one sent before it reports its
    /// page events could load unseen. A tab that is gone passes; the call then fails on
    /// the missing tab.
    pub async fn wait_ready(&self, tab: &str) -> Result<(), String> {
        let ready = self
            .wait_for(
                |s| {
                    s.tab(tab)
                        .is_none_or(|t| s.sessions.get(&t.session).is_some_and(|i| i.ready))
                },
                // Set-up waits at most CMD for each of its answers.
                Instant::now() + CMD + CMD + Duration::from_secs(5),
                &CancellationToken::new(),
            )
            .await;
        if ready == Wait::Done {
            Ok(())
        } else {
            Err("The browser tab is not ready: the app could not put its requests under its checks. Try again; if it keeps happening, restart the browser from Settings → Browser.".into())
        }
    }

    /// Waits until `cond` holds, the deadline passes, `cancel` fires or the browser goes.
    pub async fn wait_for(
        &self,
        cond: impl Fn(&State) -> bool,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> Wait {
        let mut rx = self.changed.subscribe();
        let closed = self.cdp.closed_token();
        loop {
            if cond(&self.lock()) {
                return Wait::Done;
            }
            if self.cdp.is_closed() {
                return Wait::Closed;
            }
            let now = Instant::now();
            if now >= deadline {
                return Wait::Timeout;
            }
            tokio::select! {
                r = rx.changed() => if r.is_err() { return Wait::Closed; },
                _ = tokio::time::sleep(deadline - now) => {}
                _ = cancel.cancelled() => return Wait::Cancelled,
                _ = closed.cancelled() => return Wait::Closed,
            }
        }
    }

    // ----- calls -------------------------------------------------------------------------

    /// Marks a call as in progress (one at a time; the server serialises calls).
    pub fn begin_call(&self, chat_id: i64, call: ActiveCall) {
        let mut s = self.lock();
        s.chat_id = chat_id;
        s.call = Some(call);
    }

    /// Ends the call in progress.
    pub fn end_call(&self) {
        self.lock().call = None;
    }

    /// Records which chat drives the browser, for holds raised outside a call.
    pub fn set_chat(&self, chat_id: i64) {
        self.lock().chat_id = chat_id;
    }

    /// Fails every request held for `tab` or for the chat `chat_id` (Stop, 4.2: "fails
    /// any held request").
    pub fn release_held_on_stop(&self, tab: &str, chat_id: i64) {
        self.lock().cancel_holds_on_stop(tab, chat_id);
    }

    /// Takes the notes collected since the last call.
    pub fn take_notes(&self) -> Vec<String> {
        std::mem::take(&mut self.lock().notes)
    }

    // ----- shutdown ----------------------------------------------------------------------

    /// Fails every held request, closes the browser and kills its process tree.
    pub async fn close(&self) {
        self.shutdown.cancel();
        let held: Vec<(String, String)> = {
            let mut s = self.lock();
            s.held
                .drain()
                .map(|(_, h)| {
                    h.token.cancel();
                    (h.session, h.request_id)
                })
                .collect()
        };
        for (session, request_id) in held {
            let _ = self
                .cdp
                .call(
                    "Fetch.failRequest",
                    json!({"requestId": request_id, "errorReason": "Aborted"}),
                    Some(&session),
                    Duration::from_millis(500),
                )
                .await;
        }
        let _ = self
            .cdp
            .call("Browser.close", json!({}), None, Duration::from_secs(2))
            .await;
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        }
    }

    /// Synchronous last resort (drop): cancels everything and kills the process tree.
    pub fn kill_now(&self) {
        self.shutdown.cancel();
        if let Ok(mut child) = self.child.try_lock()
            && let Some(child) = child.as_mut()
        {
            let _ = child.start_kill();
        }
    }

    /// Whether the browser process is still there.
    pub fn alive(&self) -> bool {
        !self.cdp.is_closed() && !self.shutdown.is_cancelled()
    }

    // ----- events ------------------------------------------------------------------------

    async fn run_events(this: std::sync::Weak<Self>, mut events: mpsc::UnboundedReceiver<Event>) {
        while let Some(event) = events.recv().await {
            let Some(browser) = this.upgrade() else {
                return;
            };
            browser.handle_event(event);
        }
        // The connection ended: the browser exited or crashed. Nothing held can be
        // released any more; forget it all so no wait hangs.
        if let Some(browser) = this.upgrade() {
            {
                let mut s = browser.lock();
                for (_, h) in s.held.drain() {
                    h.token.cancel();
                }
                s.note(
                    "The browser closed or crashed; it will be restarted on the next browser call."
                        .into(),
                );
            }
            browser.bump();
        }
    }

    fn handle_event(self: &Arc<Self>, event: Event) {
        let Event {
            method,
            params,
            session_id,
        } = event;
        let session = session_id.unwrap_or_default();
        match method.as_str() {
            "Target.attachedToTarget" => self.on_attached(&session, &params),
            "Target.detachedFromTarget" => self.on_detached(&params),
            "Inspector.targetCrashed" | "Target.targetCrashed" => self.on_crashed(&session),
            "Page.frameAttached" => {
                let (Some(id), parent) = (
                    params.get("frameId").and_then(Value::as_str),
                    params.get("parentFrameId").and_then(Value::as_str),
                ) else {
                    return;
                };
                let mut s = self.lock();
                let Some(tab) = s.sessions.get(&session).map(|i| i.tab.clone()) else {
                    return;
                };
                s.frame_order += 1;
                let order = s.frame_order;
                s.frames.entry(id.to_string()).or_insert(Frame {
                    session: session.clone(),
                    parent: parent.map(str::to_string),
                    tab,
                    url: String::new(),
                    origin: "null".into(),
                    loader_id: String::new(),
                    blocked: None,
                    order,
                });
            }
            "Page.frameNavigated" => self.on_frame_navigated(&session, &params),
            "Page.navigatedWithinDocument" => {
                if let (Some(id), Some(url)) = (
                    params.get("frameId").and_then(Value::as_str),
                    params.get("url").and_then(Value::as_str),
                ) && let Some(f) = self.lock().frames.get_mut(id)
                {
                    f.url = url.to_string();
                }
                self.bump();
            }
            "Page.frameDetached" => {
                let id = params.get("frameId").and_then(Value::as_str).unwrap_or("");
                let swap = params.get("reason").and_then(Value::as_str) == Some("swap");
                if !swap {
                    self.lock().frames.remove(id);
                }
            }
            "Page.frameStartedLoading"
            | "Page.frameRequestedNavigation"
            | "Page.frameStartedNavigating" => {
                let id = params.get("frameId").and_then(Value::as_str).unwrap_or("");
                // Fragment and history-API navigations never stop loading; only document
                // navigations count as the page changing.
                if params
                    .get("navigationType")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t.starts_with("same"))
                {
                    return;
                }
                let mut s = self.lock();
                if method == "Page.frameRequestedNavigation"
                    && let Some(reason) = params.get("reason").and_then(Value::as_str)
                    && reason.starts_with("formSubmission")
                    && let Some(url) = params.get("url").and_then(Value::as_str)
                {
                    let now = Instant::now();
                    s.form_submissions
                        .retain(|(_, _, at)| now.duration_since(*at) < FORM_SUBMISSION_WINDOW);
                    s.form_submissions
                        .push((id.to_string(), strip_fragment(url), now));
                }
                if let Some(tab) = s.tab_mut(id) {
                    tab.nav_seq += 1;
                    tab.last_nav_event = Some(Instant::now());
                    if method == "Page.frameStartedLoading" {
                        tab.loading = true;
                    }
                }
                drop(s);
                self.bump();
            }
            "Page.frameStoppedLoading" => {
                let id = params.get("frameId").and_then(Value::as_str).unwrap_or("");
                if let Some(tab) = self.lock().tab_mut(id) {
                    tab.stop_seq += 1;
                    tab.loading = false;
                    tab.last_nav_event = Some(Instant::now());
                }
                self.bump();
            }
            "Page.lifecycleEvent" => {
                let id = params.get("frameId").and_then(Value::as_str).unwrap_or("");
                if params.get("name").and_then(Value::as_str) == Some("load") {
                    let loader = params
                        .get("loaderId")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if let Some(tab) = self.lock().tab_mut(id) {
                        tab.loaded_loader = loader;
                    }
                    self.bump();
                }
            }
            "Page.javascriptDialogOpening" => self.on_dialog(&session, &params),
            "Page.fileChooserOpened" => self.on_file_chooser(&session, &params),
            "Browser.downloadWillBegin" => {
                let name = params
                    .get("suggestedFilename")
                    .and_then(Value::as_str)
                    .unwrap_or("a file");
                self.lock().note(format!(
                    "The page tried to download {}; downloads are not supported in this version, so it was blocked.",
                    policy::sanitize::one_line(name, 120)
                ));
            }
            "Fetch.requestPaused" => {
                let this = self.clone();
                tokio::spawn(async move { this.on_request_paused(session, params).await });
            }
            // [0.0.7 P7] What each request of a capture's tab became, and the connections
            // of the tab, its frames and workers that the request checks cannot see.
            m if m.starts_with("Network.") => {
                let mut s = self.lock();
                if let Some(log) = s.capture.as_mut() {
                    if log.session == session {
                        log.on_event(m, &params);
                    }
                    if log.watches(&session) {
                        log.note_socket(m, &params);
                    }
                }
            }
            _ => {}
        }
    }

    fn on_attached(self: &Arc<Self>, parent_session: &str, params: &Value) {
        let Some(session) = params.get("sessionId").and_then(Value::as_str) else {
            tracing::warn!("attachedToTarget without a session id");
            return;
        };
        let info = params.get("targetInfo").cloned().unwrap_or(Value::Null);
        let target_id = info
            .get("targetId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let kind = info.get("type").and_then(Value::as_str).unwrap_or("");
        let session = session.to_string();
        match kind {
            "page"
                if info
                    .get("subtype")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty()) =>
            {
                // A prerendered page (speculative loading) never becomes a tab here: it is
                // closed before it runs, so nothing loads behind the agent's back.
                let cdp = self.cdp.clone();
                tokio::spawn(async move {
                    let _ = cdp
                        .call(
                            "Target.closeTarget",
                            json!({"targetId": target_id}),
                            None,
                            CMD,
                        )
                        .await;
                });
            }
            "page" => {
                let opener = info
                    .get("openerId")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let url = info.get("url").and_then(Value::as_str).unwrap_or("");
                let mut s = self.lock();
                let granted = opener
                    .as_deref()
                    .and_then(|o| s.tab(o))
                    .and_then(|t| t.granted_site.clone());
                let known_opener = opener.as_deref().is_some_and(|o| s.tab(o).is_some());
                s.sessions.insert(
                    session.clone(),
                    SessionInfo {
                        kind: TargetKind::Page,
                        tab: target_id.clone(),
                        ready: false,
                    },
                );
                s.tabs.push(Tab {
                    target_id: target_id.clone(),
                    session: session.clone(),
                    opener: opener.clone(),
                    granted_site: granted,
                    typed_loader: None,
                    crashed: false,
                    refs: RefTable::default(),
                    screenshot_dpr: None,
                    nav_seq: 0,
                    stop_seq: 0,
                    loaded_loader: None,
                    loading: false,
                    last_nav_event: None,
                });
                s.frame_order += 1;
                let order = s.frame_order;
                s.frames.insert(
                    target_id.clone(),
                    Frame {
                        session: session.clone(),
                        parent: None,
                        tab: target_id.clone(),
                        url: url.to_string(),
                        origin: netpolicy::origin_of_str(url),
                        loader_id: String::new(),
                        blocked: None,
                        order,
                    },
                );
                if known_opener {
                    s.active = Some(target_id.clone());
                    s.note("A new tab (pop-up) opened and is now the active tab; browser_back closes it and returns to the previous tab.".into());
                } else if s.active.is_none() {
                    s.active = Some(target_id.clone());
                }
                drop(s);
                let this = self.clone();
                tokio::spawn(async move { this.setup_session(session, TargetKind::Page).await });
            }
            "iframe" => {
                let mut s = self.lock();
                let Some(tab) = s.sessions.get(parent_session).map(|i| i.tab.clone()) else {
                    drop(s);
                    self.release_other(parent_session, &session);
                    return;
                };
                s.sessions.insert(
                    session.clone(),
                    SessionInfo {
                        kind: TargetKind::Frame,
                        tab,
                        ready: false,
                    },
                );
                if let Some(f) = s.frames.get_mut(&target_id) {
                    f.session = session.clone();
                }
                drop(s);
                let this = self.clone();
                tokio::spawn(async move { this.setup_session(session, TargetKind::Frame).await });
            }
            "worker" | "shared_worker" | "service_worker"
                if self.watch_worker(parent_session, &info, &session) => {}
            _ => self.release_other(parent_session, &session),
        }
        self.bump();
    }

    /// [0.0.7 P7] While a capture runs, a worker of its tab (and any shared or service
    /// worker, which may talk to the tab) stays attached with its network events on, so a
    /// connection it opens is seen, and its requests are checked like the page's or it does
    /// not run:
    ///
    /// - a dedicated worker's requests are paused on its page's session (the browser has no
    ///   Fetch domain on a dedicated worker; it is asked anyway, in case a later one has);
    /// - a shared worker's are paused on its own session (`Fetch.enable` with the capture's
    ///   patterns, answered before it may run; when that fails it is never resumed and the
    ///   capture stops);
    /// - a service worker is not resumed while the capture runs: the browser answers
    ///   `Fetch.enable` on one, but does not pause the requests of a service worker that
    ///   starts again (one installed before), so its requests cannot be checked. It waits
    ///   until the capture ends and is then stopped ([`Self::unwatch_capture`]).
    ///
    /// The steps after `Fetch.enable` go together with the resume last, as in
    /// `setup_session`: a worker that waits answers some of them only once it runs.
    /// `false` when no capture wants it.
    fn watch_worker(self: &Arc<Self>, parent_session: &str, info: &Value, session: &str) -> bool {
        let text = |k: &str| info.get(k).and_then(Value::as_str).unwrap_or("");
        let (kind, target_id) = (text("type"), text("targetId"));
        {
            let mut s = self.lock();
            let Some(log) = s.capture.as_mut() else {
                return false;
            };
            // The browser's own workers (a built-in extension's) are not the page's.
            let wanted = if kind == "worker" {
                log.watches(parent_session)
            } else {
                web_worker(text("url"))
            };
            if !wanted {
                return false;
            }
            log.watch.insert(session.to_string());
            if log.workers.len() < 400 {
                log.workers.push(session.to_string());
            }
            if kind == "service_worker" {
                log.service_worker_held = true;
                if log.held_workers.len() < 400 {
                    log.held_workers.push(target_id.to_string());
                }
            }
        }
        let this = self.clone();
        let session = session.to_string();
        let kind = kind.to_string();
        tokio::spawn(async move {
            let s = Some(session.as_str());
            let checked = this
                .cdp
                .call(
                    "Fetch.enable",
                    json!({"patterns": Self::capture_patterns()}),
                    s,
                    CMD,
                )
                .await;
            if kind == "service_worker" {
                if let Err(e) = this.cdp.call("Network.enable", json!({}), s, CMD).await {
                    tracing::debug!(error = %e, "a held service worker's network events are off");
                }
                return;
            }
            if let Err(e) = checked
                && kind == "shared_worker"
            {
                tracing::warn!(error = %e, "a shared worker's requests cannot be checked; it is not let run");
                if let Some(log) = this.lock().capture.as_mut() {
                    log.note_unchecked_worker(&kind);
                }
                return;
            }
            let steps = [
                ("Network.enable", json!({})),
                (
                    "Target.setAutoAttach",
                    json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true}),
                ),
                ("Runtime.runIfWaitingForDebugger", json!({})),
            ];
            let calls = steps
                .iter()
                .map(|(method, params)| this.cdp.call(method, params.clone(), s, CMD));
            let results = futures_util::future::join_all(calls).await;
            for ((method, _), result) in steps.iter().zip(results) {
                if let Err(e) = result {
                    tracing::debug!(%method, error = %e, "worker set-up step failed");
                }
            }
        });
        true
    }

    /// [0.0.7 P7] Starts watching the capture's tab: network events on its frames. Shared
    /// and service workers that already run are stopped: they started before the capture,
    /// their requests were not checked one by one, and they may hold what those brought
    /// back. One the page asks for again starts anew, waiting, and [`Self::watch_worker`]
    /// checks its requests (a shared worker) or keeps it waiting (a service worker). One
    /// that cannot be stopped makes the capture stop.
    pub(crate) async fn watch_capture(&self, tab: &str) {
        let frames: Vec<String> = {
            let mut s = self.lock();
            let frames: Vec<String> = s
                .sessions
                .iter()
                .filter(|(_, i)| i.tab == tab && i.kind == TargetKind::Frame)
                .map(|(id, _)| id.clone())
                .collect();
            if let Some(log) = s.capture.as_mut() {
                log.watch.extend(frames.iter().cloned());
            }
            frames
        };
        for session in frames {
            let _ = self
                .cdp
                .call("Network.enable", json!({}), Some(&session), CMD)
                .await;
        }
        let Ok(targets) = self
            .cdp
            .call("Target.getTargets", json!({}), None, CMD)
            .await
        else {
            return;
        };
        for t in targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let kind = t.get("type").and_then(Value::as_str).unwrap_or("");
            let url = t.get("url").and_then(Value::as_str).unwrap_or("");
            if matches!(kind, "shared_worker" | "service_worker")
                && web_worker(url)
                && let Some(id) = t.get("targetId").and_then(Value::as_str)
                && let Err(e) = self
                    .cdp
                    .call("Target.closeTarget", json!({"targetId": id}), None, CMD)
                    .await
                // One that ended meanwhile holds nothing any more.
                && !e.to_string().contains("No target with given id")
            {
                tracing::warn!(error = %e, %kind, "a worker that ran before the capture could not be stopped");
                if let Some(log) = self.lock().capture.as_mut() {
                    log.note_unchecked_worker(kind);
                }
            }
        }
    }

    /// [0.0.7 P7] Lets go of what [`Self::watch_capture`] and [`Self::watch_worker`] held.
    /// A service worker kept waiting is stopped first, so nothing the page asked of it
    /// during the capture runs afterwards without the checks; it starts anew, as outside a
    /// capture, when it is next needed.
    pub(crate) async fn unwatch_capture(
        &self,
        watched: Vec<String>,
        workers: Vec<String>,
        held: Vec<String>,
    ) {
        for target in held {
            if let Err(e) = self
                .cdp
                .call("Target.closeTarget", json!({"targetId": target}), None, CMD)
                .await
            {
                tracing::debug!(error = %e, "a held service worker could not be stopped");
            }
        }
        for session in workers {
            let _ = self
                .cdp
                .call(
                    "Target.detachFromTarget",
                    json!({"sessionId": session}),
                    None,
                    CMD,
                )
                .await;
        }
        for session in watched {
            let _ = self
                .cdp
                .call("Network.disable", json!({}), Some(&session), CMD)
                .await;
        }
    }

    /// Workers and other targets: let them run and detach; their requests are not
    /// navigations.
    // TODO(M6): a navigation that a service worker answers from its own cache makes no
    // network request, so it is not re-checked here (the worker can only have been
    // installed by a site the agent already visited).
    fn release_other(&self, parent_session: &str, session: &str) {
        let cdp = self.cdp.clone();
        let session = session.to_string();
        let parent = (!parent_session.is_empty()).then(|| parent_session.to_string());
        tokio::spawn(async move {
            let _ = cdp
                .call(
                    "Runtime.runIfWaitingForDebugger",
                    json!({}),
                    Some(&session),
                    CMD,
                )
                .await;
            let _ = cdp
                .call(
                    "Target.detachFromTarget",
                    json!({"sessionId": session}),
                    parent.as_deref(),
                    CMD,
                )
                .await;
        });
    }

    /// [0.0.7 P7] What a capture pauses: every request, and the responses of documents and
    /// scripts, so the capture's tab gets a policy that blocks WebSockets
    /// (`on_response_paused`).
    fn capture_patterns() -> Value {
        json!([
            {"urlPattern": "*", "requestStage": "Request"},
            {"urlPattern": "*", "resourceType": "Document", "requestStage": "Response"},
            {"urlPattern": "*", "resourceType": "Script", "requestStage": "Response"},
            {"urlPattern": "*", "resourceType": "Other", "requestStage": "Response"}
        ])
    }

    fn fetch_patterns(&self) -> Value {
        // [0.0.7 P7] While a capture runs, every request is checked (pictures, fonts, style
        // sheets, scripts…), not only page loads.
        if self
            .capture_checks
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Self::capture_patterns();
        }
        let mut patterns =
            vec![json!({"urlPattern": "*", "resourceType": "Document", "requestStage": "Request"})];
        if !self.config.strict_sites.is_empty() {
            for t in ["XHR", "Fetch", "Ping", "EventSource", "Other"] {
                patterns
                    .push(json!({"urlPattern": "*", "resourceType": t, "requestStage": "Request"}));
            }
        }
        Value::Array(patterns)
    }

    /// [0.0.7 P7] Switches the checks of every request on or off on every target that is
    /// set up (a target set up later reads the switch itself).
    pub(crate) async fn set_capture_checks(&self, on: bool) {
        self.capture_checks
            .store(on, std::sync::atomic::Ordering::SeqCst);
        let sessions: Vec<String> = self
            .lock()
            .sessions
            .iter()
            .filter(|(_, info)| info.ready)
            .map(|(s, _)| s.clone())
            .collect();
        let patterns = self.fetch_patterns();
        for session in sessions {
            if let Err(e) = self
                .cdp
                .call(
                    "Fetch.enable",
                    json!({"patterns": patterns}),
                    Some(&session),
                    CMD,
                )
                .await
            {
                tracing::debug!(error = %e, "could not change which requests are checked");
            }
        }
    }

    async fn setup_session(self: Arc<Self>, session: String, kind: TargetKind) {
        #[cfg(feature = "test-hooks")]
        tokio::time::sleep(self.config.test_setup_delay).await;
        let s = Some(session.as_str());
        let fetch = self
            .cdp
            .call(
                "Fetch.enable",
                json!({"patterns": self.fetch_patterns()}),
                s,
                CMD,
            )
            .await;
        if let Err(e) = fetch {
            if self.cdp.is_closed() {
                return;
            }
            // Without request enforcement the target must not run.
            tracing::warn!(error = %e, "Fetch.enable failed; closing the target");
            let target = {
                let st = self.lock();
                st.sessions.get(&session).map(|i| (i.kind, i.tab.clone()))
            };
            if let Some((TargetKind::Page, tab)) = target {
                let _ = self
                    .cdp
                    .call("Target.closeTarget", json!({"targetId": tab}), None, CMD)
                    .await;
            } else {
                let _ = self
                    .cdp
                    .call(
                        "Target.detachFromTarget",
                        json!({"sessionId": session}),
                        None,
                        CMD,
                    )
                    .await;
            }
            return;
        }
        #[cfg(feature = "test-hooks")]
        tokio::time::sleep(self.config.test_setup_delay).await;
        // A target waiting for the debugger answers some of these only once it runs, so
        // they are all sent in order, with the resume last, and awaited together. Fetch is
        // already on, so the target's first request is checked whatever happens here.
        let mut steps = vec![
            ("Page.enable", json!({})),
            ("Page.setLifecycleEventsEnabled", json!({"enabled": true})),
            (
                "Page.setInterceptFileChooserDialog",
                json!({"enabled": true}),
            ),
            ("Inspector.enable", json!({})),
            (
                "Target.setAutoAttach",
                json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true}),
            ),
            ("Page.getFrameTree", json!({})),
        ];
        // [0.0.7 P7] A frame of a tab being captured reports its network events, so a
        // WebSocket it opens is seen.
        if kind == TargetKind::Frame {
            let mut st = self.lock();
            let tab = st.sessions.get(&session).map(|i| i.tab.clone());
            if let (Some(log), Some(tab)) = (st.capture.as_mut(), tab)
                && log.tab == tab
            {
                log.watch.insert(session.clone());
                steps.push(("Network.enable", json!({})));
            }
        }
        steps.push(("Runtime.runIfWaitingForDebugger", json!({})));
        let calls = steps
            .iter()
            .map(|(method, params)| self.cdp.call(method, params.clone(), s, CMD));
        let results = futures_util::future::join_all(calls).await;
        #[cfg(feature = "test-hooks")]
        tokio::time::sleep(self.config.test_setup_delay).await;
        for ((method, _), result) in steps.iter().zip(&results) {
            match result {
                Ok(tree) if *method == "Page.getFrameTree" => {
                    if let Some(root) = tree.get("frameTree") {
                        self.seed_frames(&session, root, None);
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::debug!(%method, error = %e, ?kind, "session setup step failed"),
            }
        }
        // Only now may a tool call use the tab. The checks are on (or the target was
        // closed above); a step that failed here only costs detail in what is reported.
        if let Some(info) = self.lock().sessions.get_mut(&session) {
            info.ready = true;
        }
        self.bump();
    }

    fn seed_frames(&self, session: &str, node: &Value, parent: Option<&str>) {
        let Some(frame) = node.get("frame") else {
            return;
        };
        let id = frame.get("id").and_then(Value::as_str).unwrap_or("");
        let url = frame.get("url").and_then(Value::as_str).unwrap_or("");
        let security = frame
            .get("securityOrigin")
            .and_then(Value::as_str)
            .unwrap_or("");
        let loader = frame.get("loaderId").and_then(Value::as_str).unwrap_or("");
        {
            let mut s = self.lock();
            let Some(tab) = s.sessions.get(session).map(|i| i.tab.clone()) else {
                return;
            };
            s.frame_order += 1;
            let order = s.frame_order;
            let entry = s.frames.entry(id.to_string()).or_insert(Frame {
                session: session.to_string(),
                parent: parent.map(str::to_string),
                tab,
                url: String::new(),
                origin: "null".into(),
                loader_id: String::new(),
                blocked: None,
                order,
            });
            entry.session = session.to_string();
            if entry.loader_id.is_empty() || entry.url.is_empty() {
                entry.url = url.to_string();
                entry.origin = frame_origin(url, security);
                entry.loader_id = loader.to_string();
            }
        }
        if let Some(children) = node.get("childFrames").and_then(Value::as_array) {
            for child in children {
                self.seed_frames(session, child, Some(id));
            }
        }
    }

    fn on_detached(&self, params: &Value) {
        let Some(session) = params.get("sessionId").and_then(Value::as_str) else {
            return;
        };
        self.cdp.fail_session(session, "the tab or frame went away");
        let mut s = self.lock();
        let Some(info) = s.sessions.remove(session) else {
            return;
        };
        s.held.retain(|_, h| {
            let keep = h.session != session;
            if !keep {
                h.token.cancel();
            }
            keep
        });
        if info.kind == TargetKind::Page {
            let tab = info.tab;
            let opener = s.tab(&tab).and_then(|t| t.opener.clone());
            s.tabs.retain(|t| t.target_id != tab);
            s.frames.retain(|_, f| f.tab != tab);
            if s.active.as_deref() == Some(tab.as_str()) {
                s.active = opener
                    .filter(|o| s.tab(o).is_some())
                    .or_else(|| s.tabs.last().map(|t| t.target_id.clone()));
            }
            s.sessions.retain(|_, i| i.tab != tab);
        }
        drop(s);
        self.bump();
    }

    fn on_crashed(&self, session: &str) {
        // Commands sent to the dead renderer are never answered.
        self.cdp.fail_session(session, "the page crashed");
        let mut s = self.lock();
        let Some(tab) = s.sessions.get(session).map(|i| i.tab.clone()) else {
            return;
        };
        for h in s.held.values().filter(|h| h.tab == tab) {
            h.token.cancel();
        }
        if let Some(t) = s.tab_mut(&tab) {
            t.crashed = true;
        }
        s.note("The page crashed. Navigate again to reload it.".into());
        drop(s);
        self.bump();
    }

    fn on_frame_navigated(self: &Arc<Self>, session: &str, params: &Value) {
        let Some(frame) = params.get("frame") else {
            return;
        };
        let id = frame
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let parent = frame
            .get("parentId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let url = frame
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let security = frame
            .get("securityOrigin")
            .and_then(Value::as_str)
            .unwrap_or("");
        let loader = frame
            .get("loaderId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let blocked = (!netpolicy::committed_scheme_allowed(&url)).then(|| {
            let scheme = url.split(':').next().unwrap_or("");
            format!("{scheme}: pages are blocked")
        });
        let mut s = self.lock();
        let Some(tab) = s.sessions.get(session).map(|i| i.tab.clone()) else {
            return;
        };
        let parent_origin = parent
            .as_deref()
            .and_then(|p| s.frames.get(p))
            .map(|f| f.origin.clone());
        let mut origin = frame_origin(&url, security);
        if origin == "null"
            && url.starts_with("about:")
            && let Some(p) = parent_origin
        {
            origin = p;
        }
        s.frame_order += 1;
        let order = s.frame_order;
        let entry = s.frames.entry(id.clone()).or_insert(Frame {
            session: session.to_string(),
            parent: parent.clone(),
            tab: tab.clone(),
            url: String::new(),
            origin: "null".into(),
            loader_id: String::new(),
            blocked: None,
            order,
        });
        entry.session = session.to_string();
        if parent.is_some() {
            entry.parent = parent.clone();
        }
        entry.url = url.clone();
        entry.origin = origin;
        entry.loader_id = loader;
        entry.blocked = blocked.clone();
        let is_main = id == tab && parent.is_none();
        if is_main && let Some(t) = s.tab_mut(&tab) {
            t.refs.clear();
            t.crashed = false;
        }
        if let Some(why) = &blocked {
            s.note(format!(
                "A frame showed {} and was blocked: {why}.",
                policy::sanitize::one_line(&url, 120)
            ));
        }
        drop(s);
        if is_main && blocked.is_some() {
            let cdp = self.cdp.clone();
            let session = session.to_string();
            tokio::spawn(async move {
                let _ = cdp
                    .call(
                        "Page.navigate",
                        json!({"url": "about:blank"}),
                        Some(&session),
                        CMD,
                    )
                    .await;
            });
        }
        self.bump();
    }

    fn on_dialog(&self, session: &str, params: &Value) {
        let kind = params
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("alert")
            .to_string();
        let message = params.get("message").and_then(Value::as_str).unwrap_or("");
        // Leaving a page and plain alerts are accepted; confirm and prompt are dismissed,
        // so a page cannot get a "yes" out of the agent through a dialog.
        let accept = matches!(kind.as_str(), "alert" | "beforeunload");
        self.lock().note(format!(
            "The page showed a {kind} dialog (\"{}\"); it was {}.",
            crate::snapshot::clean_text(message, 200),
            if accept { "accepted" } else { "dismissed" }
        ));
        let cdp = self.cdp.clone();
        let session = session.to_string();
        tokio::spawn(async move {
            let _ = cdp
                .call(
                    "Page.handleJavaScriptDialog",
                    json!({"accept": accept}),
                    Some(&session),
                    CMD,
                )
                .await;
        });
    }

    fn on_file_chooser(&self, session: &str, params: &Value) {
        self.lock().note(
            "The page opened a file chooser; uploads are not supported in this version, so it was cancelled."
                .into(),
        );
        let Some(backend) = params.get("backendNodeId").and_then(Value::as_i64) else {
            return;
        };
        let frame = params
            .get("frameId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let cdp = self.cdp.clone();
        let session = session.to_string();
        tokio::spawn(async move {
            // The chooser never shows while interception is on; tell the input it was
            // cancelled, as a real dialog would.
            if let Ok(ctx) = isolated_world(&cdp, &session, &frame).await
                && let Ok(object) = resolve_node(&cdp, &session, backend, ctx).await
            {
                let _ = cdp
                    .call(
                        "Runtime.callFunctionOn",
                        json!({
                            "objectId": object,
                            "functionDeclaration": "function(){this.dispatchEvent(new Event('cancel',{bubbles:true}));}",
                        }),
                        Some(&session),
                        CMD,
                    )
                    .await;
            }
        });
    }

    // ----- request enforcement -----------------------------------------------------------

    async fn on_request_paused(self: Arc<Self>, session: String, params: Value) {
        if params.get("responseStatusCode").is_some() || params.get("responseErrorReason").is_some()
        {
            self.on_response_paused(session, params).await;
            return;
        }
        let Some(request_id) = params.get("requestId").and_then(Value::as_str) else {
            tracing::warn!("requestPaused without a request id");
            return;
        };
        let request_id = request_id.to_string();
        let url_str = params
            .pointer("/request/url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let method = params
            .pointer("/request/method")
            .and_then(Value::as_str)
            .unwrap_or("GET")
            .to_ascii_uppercase();
        let resource_type = params
            .get("resourceType")
            .and_then(Value::as_str)
            .unwrap_or("Other");
        let frame_id = params
            .get("frameId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let redirected = params
            .get("redirectedRequestId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let is_document = resource_type == "Document";

        let url = match Url::parse(&url_str) {
            Ok(u) => u,
            Err(_) => {
                self.block(
                    &session,
                    &request_id,
                    &url_str,
                    "the address could not be read",
                )
                .await;
                return;
            }
        };
        if let Err(why) = netpolicy::check_scheme(&url) {
            self.block(&session, &request_id, &url_str, &why).await;
            return;
        }
        if let Err(why) = netpolicy::check_url(&url, &self.policy, &self.dns).await {
            self.block(&session, &request_id, &url_str, &why).await;
            return;
        }
        // [0.0.7 P7] A capture saves only responses to requests that got this far.
        if let Some(network_id) = params.get("networkId").and_then(Value::as_str)
            && let Some(log) = self.lock().capture.as_mut()
        {
            log.note_checked(network_id);
        }
        let origin = netpolicy::origin_of(&url).unwrap_or_else(|| "null".into());
        let tab = self.lock().tab_of(&session);
        let Some(tab) = tab else {
            // A session that is gone (or never set up) gets nothing through.
            self.block(
                &session,
                &request_id,
                &url_str,
                "the tab it belongs to is closed",
            )
            .await;
            return;
        };
        let decision = {
            let mut s = self.lock();
            let frame = s.frames.get(&frame_id).cloned();
            let top_level = frame_id == tab && frame.as_ref().is_none_or(|f| f.parent.is_none());
            let page_origin = frame
                .as_ref()
                .map(|f| f.origin.clone())
                .unwrap_or_else(|| "null".into());
            let now = Instant::now();
            let key = strip_fragment(&url_str);
            let form_submission = match s.form_submissions.iter().position(|(f, u, at)| {
                *f == frame_id && *u == key && now.duration_since(*at) < FORM_SUBMISSION_WINDOW
            }) {
                Some(i) => {
                    s.form_submissions.remove(i);
                    is_document
                }
                None => false,
            };
            let redirect_of_released = redirected.as_ref().is_some_and(|r| s.released.contains(r));
            let opener_tab = s.tab(&tab).and_then(|t| t.opener.clone());
            let granted = s.tab(&tab).and_then(|t| t.granted_site.clone());
            let call_view = s
                .call
                .as_ref()
                .map(|c| CallView {
                    intent: c.intent.clone(),
                    expected_navigation: c.expected_navigation.clone(),
                })
                .unwrap_or_default();
            let req = PausedRequest {
                tab: tab.clone(),
                opener_tab,
                method: method.clone(),
                origin: origin.clone(),
                top_level,
                is_document,
                form_submission,
                strict: self.config.is_strict(&page_origin),
                redirect_of_released,
            };
            let verdict = intent::decide(&req, granted.as_deref(), &call_view);
            match &verdict {
                Verdict::ContinueUsingIntent => {
                    if let Some(c) = s.call.as_mut() {
                        c.intent = None;
                    }
                    s.mark_released(&request_id);
                }
                Verdict::ContinueApprovedNavigation => {
                    if let Some(t) = s.tab_mut(&tab) {
                        t.granted_site = Some(origin.clone());
                    }
                }
                Verdict::Continue if redirect_of_released => s.mark_released(&request_id),
                _ => {}
            }
            (verdict, tab, top_level, page_origin)
        };
        let (verdict, tab, top_level, page_origin) = decision;
        match verdict {
            Verdict::Continue
            | Verdict::ContinueUsingIntent
            | Verdict::ContinueApprovedNavigation => {
                self.continue_request(&session, &request_id).await;
            }
            Verdict::Hold(kind) => {
                self.hold(HoldArgs {
                    session,
                    request_id,
                    tab,
                    kind,
                    method,
                    url,
                    origin,
                    page_origin,
                    top_level,
                })
                .await;
            }
        }
    }

    /// [0.0.7 P7] A response paused while a capture runs (only documents and scripts are).
    /// On the capture's tab it gets [`CAPTURE_POLICY`], which blocks WebSocket connections
    /// (the request checks cannot see them): a document's response is fulfilled with the
    /// policy added (a header added on the way does not reach a document's policy), a
    /// script's (a worker's main script) gets it as a header. Everything else, and anything
    /// that cannot be read, continues as it came.
    async fn on_response_paused(self: Arc<Self>, session: String, params: Value) {
        let Some(request_id) = params.get("requestId").and_then(Value::as_str) else {
            return;
        };
        let request_id = request_id.to_string();
        let status = params
            .get("responseStatusCode")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let ours = {
            let s = self.lock();
            match (s.capture.as_ref(), s.tab_of(&session)) {
                (Some(c), Some(tab)) => tab == c.tab,
                _ => false,
            }
        };
        let failed = params.get("responseErrorReason").is_some();
        #[cfg(feature = "test-hooks")]
        let ours = ours && !self.config.test_no_capture_policy;
        // A redirect has no page of its own (a 3xx without a `Location` is shown as a page,
        // so it gets the policy); a response without a body has nothing to run.
        let location = params
            .get("responseHeaders")
            .and_then(Value::as_array)
            .is_some_and(|headers| {
                headers.iter().any(|h| {
                    h.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|n| n.eq_ignore_ascii_case("location"))
                })
            });
        let bodiless = ((300..400).contains(&status) && location)
            || matches!(status, 204 | 205)
            || status < 100;
        if failed || !ours || bodiless {
            self.continue_response(&session, &request_id, None).await;
            return;
        }
        let mut headers: Vec<Value> = params
            .get("responseHeaders")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        headers.push(json!({"name": "Content-Security-Policy", "value": CAPTURE_POLICY}));
        let document = params.get("resourceType").and_then(Value::as_str) == Some("Document");
        if !document {
            self.continue_response(&session, &request_id, Some((status, headers)))
                .await;
            return;
        }
        let body = self
            .cdp
            .call(
                "Fetch.getResponseBody",
                json!({"requestId": request_id}),
                Some(&session),
                CMD,
            )
            .await
            .ok()
            .and_then(|r| {
                let text = r.get("body").and_then(Value::as_str)?;
                if r.get("base64Encoded").and_then(Value::as_bool) == Some(true) {
                    Some(text.to_string())
                } else {
                    Some(base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        text.as_bytes(),
                    ))
                }
            })
            .filter(|b| b.len() <= MAX_FULFILLED_BASE64);
        let Some(body) = body else {
            self.continue_response(&session, &request_id, None).await;
            return;
        };
        // The body comes decoded; the headers must not say otherwise.
        headers.retain(|h| {
            let name = h
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            !matches!(
                name.as_str(),
                "content-encoding" | "content-length" | "transfer-encoding"
            )
        });
        let fulfilled = self
            .cdp
            .call(
                "Fetch.fulfillRequest",
                json!({"requestId": request_id, "responseCode": status, "responseHeaders": headers, "body": body}),
                Some(&session),
                CMD,
            )
            .await;
        if let Err(e) = fulfilled {
            tracing::debug!(error = %e, "could not add the capture's policy to a page");
            self.continue_response(&session, &request_id, None).await;
        }
    }

    async fn continue_response(
        &self,
        session: &str,
        request_id: &str,
        changed: Option<(i64, Vec<Value>)>,
    ) {
        let params = match changed {
            Some((status, headers)) => {
                json!({"requestId": request_id, "responseCode": status, "responseHeaders": headers})
            }
            None => json!({"requestId": request_id}),
        };
        if let Err(e) = self
            .cdp
            .call("Fetch.continueResponse", params, Some(session), CMD)
            .await
        {
            tracing::debug!(error = %e, "continueResponse failed");
        }
    }

    async fn continue_request(&self, session: &str, request_id: &str) {
        if let Err(e) = self
            .cdp
            .call(
                "Fetch.continueRequest",
                json!({"requestId": request_id}),
                Some(session),
                CMD,
            )
            .await
        {
            tracing::debug!(error = %e, "continueRequest failed");
        }
    }

    async fn block(&self, session: &str, request_id: &str, url: &str, why: &str) {
        {
            let mut s = self.lock();
            s.record_block(url, why);
            if let Some(log) = s.capture.as_mut() {
                log.note_blocked(url, why);
            }
            s.note(format!(
                "Blocked a request to {}: {why}",
                policy::sanitize::one_line(url, 200)
            ));
        }
        if let Err(e) = self
            .cdp
            .call(
                "Fetch.failRequest",
                json!({"requestId": request_id, "errorReason": "Aborted"}),
                Some(session),
                CMD,
            )
            .await
        {
            tracing::debug!(error = %e, "failRequest failed");
        }
        self.bump();
    }

    async fn hold(self: &Arc<Self>, a: HoldArgs) {
        let token = self.shutdown.child_token();
        let key = format!("{}|{}", a.session, a.request_id);
        let HoldOwner {
            chat_id,
            during_tool,
            call_cancel,
        } = {
            let mut s = self.lock();
            let owner = s.hold_owner(&a.tab);
            s.held.insert(
                key.clone(),
                Held {
                    session: a.session.clone(),
                    request_id: a.request_id.clone(),
                    tab: a.tab.clone(),
                    chat_id: owner.chat_id,
                    token: token.clone(),
                },
            );
            owner
        };
        self.bump();
        // Stop of the call in progress releases what it caused.
        if let Some(call_cancel) = call_cancel {
            let token = token.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = call_cancel.cancelled() => token.cancel(),
                    _ = token.cancelled() => {}
                }
            });
        }
        let card = HeldNavigation::new(
            a.kind,
            &a.method,
            &a.url,
            Some(&a.page_origin),
            a.top_level,
            during_tool.as_deref(),
        );
        let this = self.clone();
        tokio::spawn(async move {
            let timeout = this.config.approval_timeout;
            enum Outcome {
                Allowed,
                Denied,
                TimedOut,
                Cancelled,
            }
            // The approver runs on its own task: on Stop or timeout the request is failed
            // at once, and the approver still sees the cancellation and can resolve its
            // card, without the release waiting for it.
            let mut approval = tokio::spawn({
                let approver = this.approver.clone();
                let token = token.clone();
                async move { approver.approve(chat_id, card, token).await }
            });
            let outcome = tokio::select! {
                answer = &mut approval => match answer {
                    _ if token.is_cancelled() => Outcome::Cancelled,
                    Ok(true) => Outcome::Allowed,
                    Ok(false) | Err(_) => Outcome::Denied,
                },
                _ = tokio::time::sleep(timeout) => Outcome::TimedOut,
                _ = token.cancelled() => Outcome::Cancelled,
            };
            token.cancel();
            let shown = policy::sanitize::one_line(a.url.as_str(), 200);
            let allowed = matches!(outcome, Outcome::Allowed);
            {
                let mut s = this.lock();
                match outcome {
                    Outcome::Allowed => {
                        s.mark_released(&a.request_id);
                        if a.kind == HoldKind::LeavesSite
                            && let Some(t) = s.tab_mut(&a.tab)
                        {
                            t.granted_site = Some(a.origin.clone());
                        }
                    }
                    Outcome::Denied => {
                        s.record_block(a.url.as_str(), "the user did not allow it");
                        s.note(format!(
                            "The user did not allow the request to {shown}; it was blocked."
                        ));
                    }
                    Outcome::TimedOut => {
                        s.record_block(a.url.as_str(), "nobody answered the approval in time");
                        s.note(format!(
                            "Nobody answered the approval for {shown} within {} s; it was blocked.",
                            timeout.as_secs()
                        ));
                    }
                    Outcome::Cancelled => {
                        s.record_block(a.url.as_str(), "stopped");
                        s.note(format!("The held request to {shown} was cancelled."));
                    }
                }
            }
            this.release(&key, allowed).await;
        });
    }

    /// Continues or fails a held request exactly once.
    async fn release(&self, key: &str, allow: bool) {
        let held = self.lock().held.remove(key);
        let Some(held) = held else {
            return;
        };
        let (method, params) = if allow {
            (
                "Fetch.continueRequest",
                json!({"requestId": held.request_id}),
            )
        } else {
            (
                "Fetch.failRequest",
                json!({"requestId": held.request_id, "errorReason": "Aborted"}),
            )
        };
        if let Err(e) = self
            .cdp
            .call(method, params, Some(&held.session), Duration::from_secs(5))
            .await
        {
            tracing::debug!(error = %e, %method, "releasing a held request failed");
        }
        self.bump();
    }
}

struct HoldArgs {
    session: String,
    request_id: String,
    tab: String,
    kind: HoldKind,
    method: String,
    url: Url,
    origin: String,
    page_origin: String,
    top_level: bool,
}

/// How a wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wait {
    Done,
    Timeout,
    Cancelled,
    Closed,
}

fn strip_fragment(url: &str) -> String {
    url.split('#').next().unwrap_or(url).to_string()
}

/// A frame's origin from its URL, falling back to the security origin CDP reports.
fn frame_origin(url: &str, security_origin: &str) -> String {
    let from_url = netpolicy::origin_of_str(url);
    if from_url != "null" {
        return from_url;
    }
    netpolicy::origin_of_str(security_origin)
}

/// Creates an isolated world in `frame_id` (the page's scripts cannot see or change what
/// runs there) and returns its execution context id.
pub(crate) async fn isolated_world(
    cdp: &Cdp,
    session: &str,
    frame_id: &str,
) -> Result<i64, CdpError> {
    let r = cdp
        .call(
            "Page.createIsolatedWorld",
            json!({"frameId": frame_id, "worldName": "harness-agent", "grantUniveralAccess": false}),
            Some(session),
            CMD,
        )
        .await?;
    r.get("executionContextId")
        .and_then(Value::as_i64)
        .ok_or(CdpError::Protocol {
            method: "Page.createIsolatedWorld".into(),
            message: "no execution context".into(),
        })
}

/// Resolves a backend node into an object id in the isolated world `context`.
pub(crate) async fn resolve_node(
    cdp: &Cdp,
    session: &str,
    backend: i64,
    context: i64,
) -> Result<String, CdpError> {
    let r = cdp
        .call(
            "DOM.resolveNode",
            json!({"backendNodeId": backend, "executionContextId": context}),
            Some(session),
            CMD,
        )
        .await?;
    r.pointer("/object/objectId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(CdpError::Protocol {
            method: "DOM.resolveNode".into(),
            message: "the element is gone".into(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_table_is_stable_and_clears() {
        let mut t = RefTable::default();
        let a = t.alloc("S", "F", 10);
        let b = t.alloc("S", "F", 11);
        assert_eq!(a, "e1");
        assert_eq!(b, "e2");
        assert_eq!(t.alloc("S", "F", 10), "e1");
        assert_eq!(
            t.alloc("S2", "F2", 10),
            "e3",
            "same backend id in another process"
        );
        assert_eq!(t.get(" e2 ").unwrap().backend, 11);
        t.clear();
        assert!(t.get("e1").is_none());
        assert_eq!(t.alloc("S", "F", 10), "e4", "numbers are never reused");
    }

    #[test]
    fn frame_origins_fall_back_to_the_security_origin() {
        assert_eq!(frame_origin("https://a.example/x", ""), "https://a.example");
        assert_eq!(
            frame_origin("about:blank", "https://b.example"),
            "https://b.example"
        );
        assert_eq!(frame_origin("about:blank", "://"), "null");
        assert_eq!(
            strip_fragment("https://a.example/x#y"),
            "https://a.example/x"
        );
    }

    #[test]
    fn state_typed_tracks_the_document() {
        let mut s = State::default();
        s.tabs.push(Tab {
            target_id: "T".into(),
            session: "S".into(),
            opener: None,
            granted_site: None,
            typed_loader: Some("L1".into()),
            crashed: false,
            refs: RefTable::default(),
            screenshot_dpr: None,
            nav_seq: 0,
            stop_seq: 0,
            loaded_loader: None,
            loading: false,
            last_nav_event: None,
        });
        s.frames.insert(
            "T".into(),
            Frame {
                session: "S".into(),
                parent: None,
                tab: "T".into(),
                url: "https://a.example/".into(),
                origin: "https://a.example".into(),
                loader_id: "L1".into(),
                blocked: None,
                order: 0,
            },
        );
        assert!(s.typed_on("T"));
        s.frames.get_mut("T").unwrap().loader_id = "L2".into();
        assert!(!s.typed_on("T"), "a new document clears it");
        assert!(!s.typed_on("missing"));
    }

    fn tab(id: &str, opener: Option<&str>) -> Tab {
        Tab {
            target_id: id.into(),
            session: format!("S-{id}"),
            opener: opener.map(str::to_string),
            granted_site: None,
            typed_loader: None,
            crashed: false,
            refs: RefTable::default(),
            screenshot_dpr: None,
            nav_seq: 0,
            stop_seq: 0,
            loaded_loader: None,
            loading: false,
            last_nav_event: None,
        }
    }

    #[test]
    fn holds_belong_to_the_call_in_progress_and_its_chat() {
        let mut s = State {
            chat_id: 3,
            ..Default::default()
        };
        s.tabs.push(tab("T", None));
        s.tabs.push(tab("P", Some("T")));
        s.tabs.push(tab("U", None));
        // No call: the chat that ran the last call, nothing to cancel it.
        let owner = s.hold_owner("U");
        assert_eq!(owner.chat_id, 3);
        assert!(owner.during_tool.is_none() && owner.call_cancel.is_none());
        let cancel = CancellationToken::new();
        s.call = Some(ActiveCall {
            chat_id: 1,
            tool: "browser_click".into(),
            tab: "T".into(),
            intent: None,
            expected_navigation: None,
            cancel: cancel.clone(),
        });
        for t in ["T", "P"] {
            let owner = s.hold_owner(t);
            assert_eq!(owner.chat_id, 1, "{t}");
            assert_eq!(owner.during_tool.as_deref(), Some("browser_click"), "{t}");
            assert!(owner.call_cancel.is_some(), "{t}");
        }
        // Another tab during the call: the call's chat, and its Stop fails it, but it is
        // not attributed to the tool.
        let owner = s.hold_owner("U");
        assert_eq!(owner.chat_id, 1);
        assert!(owner.during_tool.is_none());
        cancel.cancel();
        assert!(owner.call_cancel.unwrap().is_cancelled());
    }

    #[test]
    fn stop_fails_the_holds_of_its_tab_and_its_chat_only() {
        let mut s = State::default();
        let mut tokens = Vec::new();
        for (key, tab, chat) in [("a", "T", 1), ("b", "U", 1), ("c", "T", 2), ("d", "V", 2)] {
            let token = CancellationToken::new();
            tokens.push((key, token.clone()));
            s.held.insert(
                key.into(),
                Held {
                    session: "S".into(),
                    request_id: key.into(),
                    tab: tab.into(),
                    chat_id: chat,
                    token,
                },
            );
        }
        assert_eq!(s.cancel_holds_on_stop("T", 1), 3);
        let cancelled: Vec<&str> = tokens
            .iter()
            .filter(|(_, t)| t.is_cancelled())
            .map(|(k, _)| *k)
            .collect();
        assert_eq!(cancelled, ["a", "b", "c"]);
    }

    #[test]
    fn notes_are_deduplicated_and_bounded() {
        let mut s = State::default();
        s.note("a".into());
        s.note("a".into());
        for i in 0..50 {
            s.note(format!("n{i}"));
        }
        assert_eq!(s.notes.len(), MAX_NOTES);
        assert_eq!(s.notes.iter().filter(|n| *n == "a").count(), 1);
    }

    #[test]
    fn block_reasons_are_found_by_time() {
        let mut s = State::default();
        let before = Instant::now();
        s.record_block("http://10.0.0.1/", "private");
        let found = s.block_since(before).unwrap();
        assert!(
            found.contains("10.0.0.1") && found.contains("private"),
            "{found}"
        );
        assert!(
            s.block_since(Instant::now() + Duration::from_secs(1))
                .is_none()
        );
    }
}
