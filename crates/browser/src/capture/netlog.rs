//! What a capture records of its tab's network traffic (ARCHITECTURE.md 9.3, 13.6).
//!
//! While `browser_capture_site` runs, every HTTP request of every tab and of every shared
//! worker is paused and checked by the session's request enforcement (not only page
//! loads; a dedicated worker's requests are paused on its page's session), and the
//! requests that passed are noted here by their network id. Service workers do not run
//! during a capture: the browser does not pause the requests of one installed before.
//! Only responses noted here can be saved, so every byte the copy holds came through the
//! browser's own network rules. `Network.*` events of the capture's tab fill in what each
//! request became.
//!
//! Connections those checks never see (WebSocket, WebTransport, direct sockets) are
//! watched on the tab, its frames and its workers: one that is blocked before it starts is
//! only listed; one that starts makes the capture stop without saving anything.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// Requests recorded at most; the rest of a huge page is not saved.
const MAX_ENTRIES: usize = 6000;
/// Blocked requests remembered for the report.
const MAX_BLOCKED: usize = 200;
/// Connections remembered, of each kind.
const MAX_SOCKETS: usize = 50;

/// One request of the capture's tab.
#[derive(Debug, Clone, Default)]
pub struct NetEntry {
    pub request_id: String,
    /// Every address of the request: as asked for first, then each redirect.
    pub urls: Vec<String>,
    pub resource_type: String,
    pub frame_id: String,
    pub mime: String,
    pub status: i64,
    /// Decoded bytes received so far (`Network.dataReceived`).
    pub decoded_len: u64,
    /// Bytes on the wire when finished.
    pub encoded_len: u64,
    pub finished: bool,
    pub failed: Option<String>,
    /// The address the browser connected to, and its port, as the browser reports them.
    pub remote_ip: Option<String>,
    pub remote_port: Option<u16>,
    /// Order of first appearance.
    pub order: usize,
}

impl NetEntry {
    /// The address the response came from.
    pub fn final_url(&self) -> &str {
        self.urls.last().map(String::as_str).unwrap_or("")
    }
}

/// The capture's record. Lives in the browser state while a capture runs.
#[derive(Debug, Default)]
pub struct CaptureLog {
    /// The session of the capture's tab.
    pub session: String,
    /// The capture's tab (its page target id).
    pub tab: String,
    /// Other sessions whose connections count: the tab's frames and workers, and shared
    /// and service workers while the capture runs.
    pub watch: HashSet<String>,
    /// Worker sessions kept attached for the capture, to let go of afterwards.
    pub workers: Vec<String>,
    /// Service workers (target ids) kept waiting during the capture, to stop afterwards.
    pub held_workers: Vec<String>,
    /// Service workers (target ids) that ran when the capture started and were stopped
    /// then. The browser may still list one (one that was kept waiting in an earlier
    /// capture does not finish stopping), so the report does not count it as held unless
    /// it starts again during this capture (its attach event is then handled as any other).
    pub ran_before: HashSet<String>,
    /// Connections that started although the request checks cannot see them, as
    /// "a WebSocket to <address>".
    pub sockets: Vec<String>,
    /// Connections the page asked for that were blocked before they started.
    pub tried: Vec<String>,
    /// Shared or service workers ("shared_worker", "service_worker", or "worker" when the
    /// running ones could not be listed) whose requests could not be held to the checks:
    /// one that ran before the capture and could not be stopped, or one whose requests
    /// could not be paused. Any makes the capture stop.
    pub unchecked_workers: Vec<String>,
    /// A service worker was kept from running during the capture (its requests cannot be
    /// checked), so what the page would have had from it is missing.
    pub service_worker_held: bool,
    /// WebSocket and WebTransport ids and their addresses.
    socket_urls: HashMap<String, String>,
    pub entries: HashMap<String, NetEntry>,
    /// Network ids of requests that passed the request checks during the capture.
    pub checked: HashSet<String>,
    /// Requests the checks refused: address and why.
    pub blocked: Vec<(String, String)>,
    /// More requests than [`MAX_ENTRIES`] were made.
    pub full: bool,
    /// Bumped on every event, to tell when the page has gone quiet.
    pub changes: u64,
}

impl CaptureLog {
    pub fn new(session: &str, tab: &str) -> Self {
        Self {
            session: session.to_string(),
            tab: tab.to_string(),
            ..Default::default()
        }
    }

    /// Whether the connections of `session` count for this capture.
    pub fn watches(&self, session: &str) -> bool {
        session == self.session || self.watch.contains(session)
    }

    /// A connection event (`Network.webSocket*`, `Network.webTransport*`,
    /// `Network.direct*Socket*`) of a watched session.
    pub fn note_socket(&mut self, method: &str, p: &Value) {
        let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let id = if method.contains("webTransport") {
            s("transportId")
        } else {
            s("requestId")
        };
        match method {
            "Network.webSocketCreated" | "Network.webTransportCreated" => {
                let url = s("url");
                if self.socket_urls.len() < MAX_SOCKETS * 4 {
                    self.socket_urls.insert(id, url.clone());
                }
                let kind = if method.contains("webTransport") {
                    "a WebTransport connection"
                } else {
                    "a WebSocket"
                };
                push_once(&mut self.tried, format!("{kind} to {url}"));
            }
            "Network.webSocketWillSendHandshakeRequest"
            | "Network.webSocketHandshakeResponseReceived"
            | "Network.webSocketFrameReceived"
            | "Network.webSocketFrameSent" => {
                let url = self.socket_urls.get(&id).cloned().unwrap_or_default();
                let what = format!("a WebSocket to {url}");
                self.tried.retain(|t| *t != what);
                push_once(&mut self.sockets, what);
            }
            "Network.webTransportConnectionEstablished" => {
                let url = self.socket_urls.get(&id).cloned().unwrap_or_default();
                let what = format!("a WebTransport connection to {url}");
                self.tried.retain(|t| *t != what);
                push_once(&mut self.sockets, what);
            }
            "Network.directTCPSocketOpened" | "Network.directUDPSocketOpened" => {
                let addr = format!(
                    "{}:{}",
                    s("remoteAddr"),
                    p.get("remotePort").and_then(Value::as_u64).unwrap_or(0)
                );
                push_once(&mut self.sockets, format!("a direct socket to {addr}"));
            }
            _ => {}
        }
    }

    /// A shared or service worker whose requests the checks could not hold (see
    /// [`Self::unchecked_workers`]).
    pub fn note_unchecked_worker(&mut self, kind: &str) {
        push_once(&mut self.unchecked_workers, kind.to_string());
    }

    /// A request the checks let through.
    pub fn note_checked(&mut self, network_id: &str) {
        if self.checked.len() < MAX_ENTRIES * 2 {
            self.checked.insert(network_id.to_string());
        }
    }

    /// A request the checks refused.
    pub fn note_blocked(&mut self, url: &str, why: &str) {
        if self.blocked.len() < MAX_BLOCKED && !self.blocked.iter().any(|(u, _)| u == url) {
            self.blocked.push((url.to_string(), why.to_string()));
        }
    }

    /// A `Network.*` event of the capture's tab.
    pub fn on_event(&mut self, method: &str, p: &Value) {
        let Some(id) = p.get("requestId").and_then(Value::as_str) else {
            return;
        };
        self.changes = self.changes.wrapping_add(1);
        let s = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or("").to_string();
        match method {
            "Network.requestWillBeSent" => {
                let url = s(p.pointer("/request/url"));
                if let Some(e) = self.entries.get_mut(id) {
                    // A redirect: the same request, a new address.
                    if e.urls.len() < 20 {
                        e.urls.push(url);
                    }
                    return;
                }
                if self.entries.len() >= MAX_ENTRIES {
                    self.full = true;
                    return;
                }
                let order = self.entries.len();
                self.entries.insert(
                    id.to_string(),
                    NetEntry {
                        request_id: id.to_string(),
                        urls: vec![url],
                        resource_type: s(p.get("type")),
                        frame_id: s(p.get("frameId")),
                        order,
                        ..Default::default()
                    },
                );
            }
            "Network.responseReceived" => {
                if let Some(e) = self.entries.get_mut(id) {
                    let r = p.get("response").cloned().unwrap_or(Value::Null);
                    e.status = r.get("status").and_then(Value::as_i64).unwrap_or(0);
                    e.mime = s(r.get("mimeType"));
                    let t = s(p.get("type"));
                    if !t.is_empty() {
                        e.resource_type = t;
                    }
                    let url = s(r.get("url"));
                    if !url.is_empty() && e.urls.last() != Some(&url) && e.urls.len() < 20 {
                        e.urls.push(url);
                    }
                    e.remote_ip = r
                        .get("remoteIPAddress")
                        .and_then(Value::as_str)
                        .filter(|ip| !ip.is_empty())
                        .map(str::to_string);
                    e.remote_port = r
                        .get("remotePort")
                        .and_then(Value::as_u64)
                        .and_then(|p| u16::try_from(p).ok());
                }
            }
            "Network.dataReceived" => {
                if let Some(e) = self.entries.get_mut(id) {
                    e.decoded_len = e
                        .decoded_len
                        .saturating_add(p.get("dataLength").and_then(Value::as_u64).unwrap_or(0));
                }
            }
            "Network.loadingFinished" => {
                if let Some(e) = self.entries.get_mut(id) {
                    e.finished = true;
                    e.encoded_len = p
                        .get("encodedDataLength")
                        .and_then(Value::as_f64)
                        .map(|f| f.max(0.0) as u64)
                        .unwrap_or(0);
                }
            }
            "Network.loadingFailed" => {
                if let Some(e) = self.entries.get_mut(id) {
                    let why = s(p.get("blockedReason"));
                    let text = s(p.get("errorText"));
                    e.failed = Some(if why.is_empty() { text } else { why });
                }
            }
            _ => {}
        }
    }

    /// Entries in the order they appeared.
    pub fn ordered(&self) -> Vec<&NetEntry> {
        let mut v: Vec<&NetEntry> = self.entries.values().collect();
        v.sort_by_key(|e| e.order);
        v
    }

    /// Requests still waiting for their response or body.
    pub fn pending(&self) -> usize {
        self.entries
            .values()
            .filter(|e| !e.finished && e.failed.is_none())
            .count()
    }
}

fn push_once(list: &mut Vec<String>, item: String) {
    if list.len() < MAX_SOCKETS && !list.contains(&item) {
        list.push(item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_request_is_followed_through_redirects_to_its_body() {
        let mut log = CaptureLog::new("S", "T");
        log.on_event("Network.requestWillBeSent", &json!({"requestId": "1", "type": "Image", "frameId": "F", "request": {"url": "https://a.example/logo"}}));
        log.on_event("Network.requestWillBeSent", &json!({"requestId": "1", "request": {"url": "https://cdn.example/logo.png"}, "redirectResponse": {}}));
        log.on_event("Network.responseReceived", &json!({"requestId": "1", "type": "Image", "response": {"url": "https://cdn.example/logo.png", "status": 200, "mimeType": "image/png", "remoteIPAddress": "93.184.215.14", "remotePort": 443}}));
        log.on_event(
            "Network.dataReceived",
            &json!({"requestId": "1", "dataLength": 100}),
        );
        log.on_event(
            "Network.dataReceived",
            &json!({"requestId": "1", "dataLength": 50}),
        );
        assert_eq!(log.pending(), 1);
        log.on_event(
            "Network.loadingFinished",
            &json!({"requestId": "1", "encodedDataLength": 120.0}),
        );
        let e = &log.entries["1"];
        assert_eq!(
            e.urls,
            ["https://a.example/logo", "https://cdn.example/logo.png"]
        );
        assert_eq!(e.final_url(), "https://cdn.example/logo.png");
        assert_eq!(
            (e.status, e.decoded_len, e.encoded_len, e.finished),
            (200, 150, 120, true)
        );
        assert_eq!(e.remote_ip.as_deref(), Some("93.184.215.14"));
        assert_eq!(e.remote_port, Some(443));
        assert_eq!(log.pending(), 0);
        log.on_event("Network.requestWillBeSent", &json!({"requestId": "2", "type": "Font", "request": {"url": "https://a.example/f.woff2"}}));
        log.on_event("Network.loadingFailed", &json!({"requestId": "2", "errorText": "net::ERR_FAILED", "blockedReason": "inspector"}));
        assert_eq!(log.entries["2"].failed.as_deref(), Some("inspector"));
        assert_eq!(
            log.ordered()
                .iter()
                .map(|e| e.request_id.as_str())
                .collect::<Vec<_>>(),
            ["1", "2"]
        );
        log.note_blocked("http://10.0.0.1/x.png", "private");
        log.note_blocked("http://10.0.0.1/x.png", "private");
        assert_eq!(log.blocked.len(), 1);
    }

    #[test]
    fn a_socket_that_starts_is_told_apart_from_one_that_was_blocked() {
        let mut log = CaptureLog::new("S", "T");
        log.watch.insert("W".into());
        assert!(log.watches("S") && log.watches("W") && !log.watches("X"));
        log.note_socket(
            "Network.webSocketCreated",
            &json!({"requestId": "1", "url": "ws://10.0.0.1/a"}),
        );
        assert_eq!(log.tried, ["a WebSocket to ws://10.0.0.1/a"]);
        assert!(log.sockets.is_empty(), "blocked before it started");
        log.note_socket(
            "Network.webSocketCreated",
            &json!({"requestId": "2", "url": "ws://127.0.0.1:9/b"}),
        );
        log.note_socket(
            "Network.webSocketWillSendHandshakeRequest",
            &json!({"requestId": "2"}),
        );
        log.note_socket("Network.webSocketFrameReceived", &json!({"requestId": "2"}));
        assert_eq!(log.sockets, ["a WebSocket to ws://127.0.0.1:9/b"]);
        assert_eq!(log.tried, ["a WebSocket to ws://10.0.0.1/a"]);
        log.note_socket(
            "Network.webTransportCreated",
            &json!({"transportId": "t", "url": "https://192.168.1.1:4433/"}),
        );
        log.note_socket(
            "Network.webTransportConnectionEstablished",
            &json!({"transportId": "t"}),
        );
        log.note_socket(
            "Network.directTCPSocketOpened",
            &json!({"identifier": "d", "remoteAddr": "10.1.1.1", "remotePort": 22}),
        );
        assert_eq!(log.sockets.len(), 3, "{:?}", log.sockets);
        assert!(log.sockets[2].ends_with("10.1.1.1:22"));
    }
}
