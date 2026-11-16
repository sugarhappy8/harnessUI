//! What a capture records of its tab's network traffic (ARCHITECTURE.md 9.3, 13.6).
//!
//! While `browser_capture_site` runs, every request of every tab is paused and checked by
//! the session's request enforcement (not only page loads), and the requests that passed
//! are noted here by their network id. Only responses noted here can be saved, so every
//! byte the copy holds came through the browser's own network rules. `Network.*` events
//! of the capture's tab fill in what each request became.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// Requests recorded at most; the rest of a huge page is not saved.
const MAX_ENTRIES: usize = 6000;
/// Blocked requests remembered for the report.
const MAX_BLOCKED: usize = 200;

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
    pub fn new(session: &str) -> Self {
        Self {
            session: session.to_string(),
            ..Default::default()
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_request_is_followed_through_redirects_to_its_body() {
        let mut log = CaptureLog::new("S");
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
}
