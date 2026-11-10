//! A small CDP client of our own (ARCHITECTURE.md 9.3): request/response correlation,
//! events, flattened sessions, timeouts and cancellation. Events it cannot parse are
//! logged and skipped, never dropped silently.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::transport::Transport;

/// Why a CDP command failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CdpError {
    /// The connection to the browser is gone (it exited or crashed).
    #[error("the browser connection closed")]
    Closed,
    /// No answer in time.
    #[error("the browser did not answer {0} in time")]
    Timeout(String),
    /// The call was cancelled (Stop).
    #[error("stopped")]
    Cancelled,
    /// The browser answered with an error.
    #[error("{method}: {message}")]
    Protocol {
        /// The command.
        method: String,
        /// The browser's message.
        message: String,
    },
}

/// One event from the browser.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// `Domain.event`.
    pub method: String,
    /// The event's parameters.
    pub params: Value,
    /// The flattened session it belongs to; `None` for the browser target.
    pub session_id: Option<String>,
}

type Pending = Arc<Mutex<HashMap<u64, (String, oneshot::Sender<Result<Value, CdpError>>)>>>;

/// A connection to one browser.
#[derive(Clone)]
pub struct Cdp {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
    closed: CancellationToken,
}

impl std::fmt::Debug for Cdp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cdp")
            .field("closed", &self.closed.is_cancelled())
            .finish()
    }
}

/// What one incoming message is.
#[derive(Debug, PartialEq)]
enum Incoming {
    Response {
        id: u64,
        result: Result<Value, String>,
    },
    Event(Event),
}

fn parse_incoming(text: &str) -> Result<Incoming, String> {
    let mut v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    if let Some(id) = v.get("id").and_then(Value::as_u64) {
        if let Some(error) = v.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string();
            let data = error.get("data").and_then(Value::as_str).unwrap_or("");
            let message = if data.is_empty() {
                message
            } else {
                format!("{message} ({data})")
            };
            return Ok(Incoming::Response {
                id,
                result: Err(message),
            });
        }
        let result = v.get_mut("result").map(Value::take).unwrap_or(Value::Null);
        return Ok(Incoming::Response {
            id,
            result: Ok(result),
        });
    }
    let method = v
        .get("method")
        .and_then(Value::as_str)
        .ok_or("neither a response nor an event")?
        .to_string();
    let session_id = v
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let params = v.get_mut("params").map(Value::take).unwrap_or(Value::Null);
    Ok(Incoming::Event(Event {
        method,
        params,
        session_id,
    }))
}

impl Cdp {
    /// Starts reading `transport`. Events arrive on the returned channel, which closes
    /// when the connection ends.
    pub fn start(transport: Transport) -> (Self, mpsc::UnboundedReceiver<Event>) {
        let Transport {
            outgoing,
            mut incoming,
        } = transport;
        let pending: Pending = Arc::default();
        let closed = CancellationToken::new();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let reader_pending = pending.clone();
        let reader_closed = closed.clone();
        tokio::spawn(async move {
            while let Some(text) = incoming.recv().await {
                match parse_incoming(&text) {
                    Ok(Incoming::Response { id, result }) => {
                        let waiter = reader_pending.lock().ok().and_then(|mut p| p.remove(&id));
                        if let Some((method, tx)) = waiter {
                            let _ = tx.send(
                                result.map_err(|message| CdpError::Protocol { method, message }),
                            );
                        }
                    }
                    Ok(Incoming::Event(event)) => {
                        if event_tx.send(event).is_err() {
                            // Nobody listens any more; keep answering calls.
                        }
                    }
                    Err(why) => {
                        let head: String = text.chars().take(200).collect();
                        tracing::warn!(%why, message = %head, "unreadable CDP message skipped");
                    }
                }
            }
            reader_closed.cancel();
            if let Ok(mut p) = reader_pending.lock() {
                for (_, (_, tx)) in p.drain() {
                    let _ = tx.send(Err(CdpError::Closed));
                }
            }
        });
        (
            Self {
                out: outgoing,
                pending,
                next_id: Arc::new(AtomicU64::new(1)),
                closed,
            },
            event_rx,
        )
    }

    /// Cancelled when the connection ends.
    pub fn closed_token(&self) -> CancellationToken {
        self.closed.clone()
    }

    /// Whether the connection has ended.
    pub fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }

    /// Sends `method` to `session` (the browser target when `None`) and waits for the
    /// answer, at most `timeout`.
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, CdpError> {
        self.call_cancellable(method, params, session, timeout, &CancellationToken::new())
            .await
    }

    /// Like [`call`](Self::call), and gives up as soon as `cancel` fires.
    pub async fn call_cancellable(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Value, CdpError> {
        if self.is_closed() {
            return Err(CdpError::Closed);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        let (tx, rx) = oneshot::channel();
        if let Ok(mut p) = self.pending.lock() {
            p.insert(id, (method.to_string(), tx));
        }
        if self.out.send(message.to_string()).is_err() {
            self.forget(id);
            return Err(CdpError::Closed);
        }
        let result = tokio::select! {
            r = rx => r.unwrap_or(Err(CdpError::Closed)),
            _ = tokio::time::sleep(timeout) => Err(CdpError::Timeout(method.to_string())),
            _ = cancel.cancelled() => Err(CdpError::Cancelled),
            _ = self.closed.cancelled() => Err(CdpError::Closed),
        };
        if result.is_err() {
            self.forget(id);
        }
        result
    }

    /// Sends a command without waiting for its answer (used while shutting down).
    pub fn send_nowait(&self, method: &str, params: Value, session: Option<&str>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        let _ = self.out.send(message.to_string());
    }

    fn forget(&self, id: u64) {
        if let Ok(mut p) = self.pending.lock() {
            p.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_responses_errors_and_events() {
        assert_eq!(
            parse_incoming(r#"{"id":3,"result":{"a":1}}"#).unwrap(),
            Incoming::Response {
                id: 3,
                result: Ok(json!({"a": 1}))
            }
        );
        assert_eq!(
            parse_incoming(r#"{"id":4,"error":{"code":-32000,"message":"No node","data":"x"}}"#)
                .unwrap(),
            Incoming::Response {
                id: 4,
                result: Err("No node (x)".into())
            }
        );
        assert_eq!(
            parse_incoming(r#"{"method":"Page.loadEventFired","params":{"t":1},"sessionId":"S"}"#)
                .unwrap(),
            Incoming::Event(Event {
                method: "Page.loadEventFired".into(),
                params: json!({"t": 1}),
                session_id: Some("S".into())
            })
        );
        assert!(parse_incoming("nope").is_err());
        assert!(parse_incoming(r#"{"x":1}"#).is_err());
    }

    fn fake() -> (
        Cdp,
        mpsc::UnboundedReceiver<Event>,
        mpsc::UnboundedReceiver<String>,
        mpsc::UnboundedSender<String>,
    ) {
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (cdp, events) = Cdp::start(Transport {
            outgoing: out_tx,
            incoming: in_rx,
        });
        (cdp, events, out_rx, in_tx)
    }

    #[tokio::test]
    async fn correlates_answers_by_id_and_routes_events() {
        let (cdp, mut events, mut sent, browser) = fake();
        let call = tokio::spawn({
            let cdp = cdp.clone();
            async move {
                cdp.call(
                    "Browser.getVersion",
                    json!({}),
                    Some("S1"),
                    Duration::from_secs(5),
                )
                .await
            }
        });
        let request: Value = serde_json::from_str(&sent.recv().await.unwrap()).unwrap();
        assert_eq!(request["method"], "Browser.getVersion");
        assert_eq!(request["sessionId"], "S1");
        let id = request["id"].as_u64().unwrap();
        browser
            .send(r#"{"method":"Target.attachedToTarget","params":{}}"#.into())
            .unwrap();
        browser.send("garbage".into()).unwrap();
        browser
            .send(format!(r#"{{"id":{id},"result":{{"product":"x"}}}}"#))
            .unwrap();
        assert_eq!(call.await.unwrap().unwrap(), json!({"product": "x"}));
        assert_eq!(
            events.recv().await.unwrap().method,
            "Target.attachedToTarget"
        );
    }

    #[tokio::test]
    async fn protocol_errors_timeouts_cancellation_and_close() {
        let (cdp, _events, mut sent, browser) = fake();
        let c = cdp.clone();
        let call = tokio::spawn(async move {
            c.call("DOM.focus", json!({}), None, Duration::from_secs(5))
                .await
        });
        let request: Value = serde_json::from_str(&sent.recv().await.unwrap()).unwrap();
        browser
            .send(format!(
                r#"{{"id":{},"error":{{"message":"Element is not focusable"}}}}"#,
                request["id"]
            ))
            .unwrap();
        assert_eq!(
            call.await.unwrap(),
            Err(CdpError::Protocol {
                method: "DOM.focus".into(),
                message: "Element is not focusable".into()
            })
        );
        assert_eq!(
            cdp.call("Page.enable", json!({}), None, Duration::from_millis(20))
                .await,
            Err(CdpError::Timeout("Page.enable".into()))
        );
        let token = CancellationToken::new();
        token.cancel();
        assert_eq!(
            cdp.call_cancellable(
                "Page.enable",
                json!({}),
                None,
                Duration::from_secs(5),
                &token
            )
            .await,
            Err(CdpError::Cancelled)
        );
        let c = cdp.clone();
        let waiting = tokio::spawn(async move {
            c.call("Page.navigate", json!({}), None, Duration::from_secs(30))
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(browser);
        assert_eq!(waiting.await.unwrap(), Err(CdpError::Closed));
        assert!(cdp.is_closed());
        assert_eq!(
            cdp.call("Page.enable", json!({}), None, Duration::from_secs(1))
                .await,
            Err(CdpError::Closed)
        );
    }
}
