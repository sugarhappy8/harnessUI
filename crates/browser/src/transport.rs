//! The two ways CDP messages travel (ARCHITECTURE.md 9.3): NUL-delimited JSON over a pair
//! of pipes (`--remote-debugging-pipe`), or WebSocket frames over a loopback port.
//!
//! Both end up as a pair of channels of whole JSON messages, so the CDP client does not
//! care which one is in use. The incoming channel closes when the browser goes away.

use std::io::{Read, Write};

use tokio::sync::mpsc;

/// The largest message accepted. Screenshots and DOM snapshots of big pages are tens of
/// megabytes; anything above this is treated as a broken connection.
pub const MAX_MESSAGE_BYTES: usize = 256 * 1024 * 1024;

/// Whole messages in both directions.
pub struct Transport {
    /// Messages to the browser.
    pub outgoing: mpsc::UnboundedSender<String>,
    /// Messages from the browser; closes when the connection ends.
    pub incoming: mpsc::UnboundedReceiver<String>,
}

/// Splits a byte stream into NUL-terminated messages.
#[derive(Debug, Default)]
pub struct NulFramer {
    buf: Vec<u8>,
}

/// What [`NulFramer::push`] produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    /// One complete message (lossily decoded if it was not UTF-8).
    Message(String),
    /// The pending message grew past [`MAX_MESSAGE_BYTES`].
    TooLarge,
}

impl NulFramer {
    /// Adds bytes and returns every message they completed.
    pub fn push(&mut self, data: &[u8]) -> Vec<Frame> {
        self.push_with_limit(data, MAX_MESSAGE_BYTES)
    }

    fn push_with_limit(&mut self, data: &[u8], limit: usize) -> Vec<Frame> {
        let mut out = Vec::new();
        let mut rest = data;
        while let Some(pos) = rest.iter().position(|&b| b == 0) {
            self.buf.extend_from_slice(&rest[..pos]);
            let message = std::mem::take(&mut self.buf);
            if message.len() > limit {
                out.push(Frame::TooLarge);
            } else if !message.is_empty() {
                out.push(Frame::Message(match String::from_utf8(message) {
                    Ok(s) => s,
                    Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
                }));
            }
            rest = &rest[pos + 1..];
        }
        self.buf.extend_from_slice(rest);
        if self.buf.len() > limit {
            self.buf.clear();
            out.push(Frame::TooLarge);
        }
        out
    }
}

/// A transport over a pair of pipe ends. Blocking threads do the I/O, because anonymous
/// pipes on Windows have no asynchronous mode; each thread ends when its pipe closes.
pub fn pipe_transport<R, W>(reader: R, writer: W) -> std::io::Result<Transport>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let (in_tx, in_rx) = mpsc::unbounded_channel::<String>();
    std::thread::Builder::new()
        .name("cdp-pipe-read".into())
        .spawn(move || {
            let mut reader = reader;
            let mut framer = NulFramer::default();
            let mut chunk = vec![0u8; 64 * 1024];
            loop {
                let n = match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        tracing::debug!(error = %e, "CDP pipe read ended");
                        break;
                    }
                };
                for frame in framer.push(&chunk[..n]) {
                    match frame {
                        Frame::Message(m) => {
                            if in_tx.send(m).is_err() {
                                return;
                            }
                        }
                        Frame::TooLarge => {
                            tracing::warn!(
                                "CDP message above the size limit; closing the connection"
                            );
                            return;
                        }
                    }
                }
            }
        })?;
    std::thread::Builder::new()
        .name("cdp-pipe-write".into())
        .spawn(move || {
            let mut writer = writer;
            while let Some(message) = out_rx.blocking_recv() {
                let mut bytes = message.into_bytes();
                bytes.push(0);
                let ok = writer.write_all(&bytes).and_then(|()| writer.flush());
                if let Err(e) = ok {
                    tracing::debug!(error = %e, "CDP pipe write ended");
                    break;
                }
            }
        })?;
    Ok(Transport {
        outgoing: out_tx,
        incoming: in_rx,
    })
}

/// A transport over a WebSocket to `ws_url` on 127.0.0.1 (loopback-port mode).
pub async fn websocket_transport(ws_url: &str) -> Result<Transport, String> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

    let url = url::Url::parse(ws_url).map_err(|e| format!("bad DevTools address: {e}"))?;
    if url.host_str() != Some("127.0.0.1") || url.scheme() != "ws" {
        return Err(format!(
            "refusing a DevTools address that is not ws://127.0.0.1 ({url})"
        ));
    }
    let port = url.port().ok_or("the DevTools address has no port")?;
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|e| format!("could not connect to the browser's debugging port: {e}"))?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES));
    let (socket, _) = tokio_tungstenite::client_async_with_config(ws_url, stream, Some(config))
        .await
        .map_err(|e| format!("the browser refused the DevTools connection: {e}"))?;
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let (in_tx, in_rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            if sink.send(Message::text(message)).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    tokio::spawn(async move {
        while let Some(item) = stream.next().await {
            match item {
                Ok(Message::Text(text)) => {
                    if in_tx.send(text.to_string()).is_err() {
                        break;
                    }
                }
                Ok(Message::Binary(bytes)) => {
                    if in_tx
                        .send(String::from_utf8_lossy(&bytes).into_owned())
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!(error = %e, "CDP websocket ended");
                    break;
                }
            }
        }
    });
    Ok(Transport {
        outgoing: out_tx,
        incoming: in_rx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framer_splits_on_nul_across_chunks() {
        let mut f = NulFramer::default();
        assert_eq!(f.push(b"{\"id\":1"), vec![]);
        assert_eq!(
            f.push(b"}\0{\"id\":2}\0{\"id"),
            vec![
                Frame::Message("{\"id\":1}".into()),
                Frame::Message("{\"id\":2}".into())
            ]
        );
        assert_eq!(
            f.push(b"\":3}\0\0"),
            vec![Frame::Message("{\"id\":3}".into())]
        );
    }

    #[test]
    fn framer_reports_oversized_messages() {
        let mut f = NulFramer::default();
        assert_eq!(f.push_with_limit(b"0123456789", 4), vec![Frame::TooLarge]);
        assert_eq!(
            f.push_with_limit(b"ab\0", 4),
            vec![Frame::Message("ab".into())]
        );
        assert_eq!(
            f.push_with_limit(b"abcdef\0x\0", 4),
            vec![Frame::TooLarge, Frame::Message("x".into())]
        );
    }

    #[test]
    fn framer_survives_invalid_utf8() {
        let mut f = NulFramer::default();
        let frames = f.push(b"a\xffb\0");
        assert_eq!(frames, vec![Frame::Message("a\u{FFFD}b".into())]);
    }

    #[tokio::test]
    async fn pipe_transport_round_trips_messages() {
        let (r1, w1) = std::io::pipe().unwrap();
        let (r2, w2) = std::io::pipe().unwrap();
        // The far end echoes: reads from r1, writes to w2.
        std::thread::spawn(move || {
            let mut r1 = r1;
            let mut w2 = w2;
            let mut got = Vec::new();
            let mut buf = [0u8; 1024];
            while !got.contains(&0) {
                let n = r1.read(&mut buf).unwrap();
                assert!(n > 0, "the transport closed before a whole message arrived");
                got.extend_from_slice(&buf[..n]);
            }
            w2.write_all(&got).unwrap();
        });
        let mut t = pipe_transport(r2, w1).unwrap();
        t.outgoing.send("{\"id\":7}".into()).unwrap();
        assert_eq!(t.incoming.recv().await.unwrap(), "{\"id\":7}");
        // The echo thread exited and closed its end: the channel closes.
        assert_eq!(t.incoming.recv().await, None);
    }

    #[tokio::test]
    async fn websocket_transport_refuses_non_loopback_addresses() {
        for bad in [
            "ws://example.com:9222/x",
            "wss://127.0.0.1:9222/x",
            "ws://127.0.0.2:1/x",
        ] {
            assert!(websocket_transport(bad).await.is_err(), "{bad}");
        }
    }
}
