//! A tiny HTTP/1.1 server on a tokio `TcpListener` that serves canned responses, for testing
//! the client without a real model server or an HTTP-server dependency.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// One request the server received.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    /// The request target: a path, or an absolute URL when the server is used as a proxy.
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("request body is JSON")
    }

    /// The path of the target, without query (works for absolute-form targets too).
    pub fn path(&self) -> &str {
        let target = self
            .target
            .split_once("://")
            .map_or(self.target.as_str(), |(_, rest)| {
                rest.find('/').map_or("/", |slash| &rest[slash..])
            });
        target.split('?').next().unwrap_or(target)
    }
}

/// How a streamed body ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEnd {
    /// The terminating zero-length chunk, then close.
    Clean,
    /// Close the socket mid-body, without the terminating chunk.
    Abort,
    /// Keep the connection open (up to 20 s) and report when the client closes it.
    HoldOpen,
}

/// What to answer.
#[derive(Debug, Clone)]
pub enum Reply {
    /// A complete response with a Content-Length.
    Raw {
        status: u16,
        content_type: String,
        body: Vec<u8>,
    },
    /// A chunked response written piece by piece.
    Stream {
        status: u16,
        content_type: String,
        chunks: Vec<Vec<u8>>,
        delay: Duration,
        end: StreamEnd,
    },
    /// Say nothing for this long, then close.
    Silence(Duration),
    /// A redirect with this status to `location`, with an empty body.
    Redirect { status: u16, location: String },
}

impl Reply {
    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        Self::Raw {
            status,
            content_type: "application/json".into(),
            body: value.to_string().into_bytes(),
        }
    }

    pub fn raw_json(status: u16, body: &[u8]) -> Self {
        Self::Raw {
            status,
            content_type: "application/json; charset=utf-8".into(),
            body: body.to_vec(),
        }
    }

    pub fn not_found() -> Self {
        Self::json(
            404,
            &serde_json::json!({"error": {"code": 404, "message": "File Not Found", "type": "not_found_error"}}),
        )
    }

    /// An event stream of `body` cut into `piece`-byte chunks.
    pub fn sse(body: &[u8], piece: usize, delay: Duration, end: StreamEnd) -> Self {
        Self::Stream {
            status: 200,
            content_type: "text/event-stream".into(),
            chunks: body.chunks(piece.max(1)).map(<[u8]>::to_vec).collect(),
            delay,
            end,
        }
    }
}

type Handler = dyn Fn(&Recorded) -> Reply + Send + Sync;

pub struct MockServer {
    pub addr: SocketAddr,
    requests: Arc<Mutex<Vec<Recorded>>>,
    closed: tokio::sync::Mutex<mpsc::UnboundedReceiver<Instant>>,
}

impl MockServer {
    pub async fn start(handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        Self::serve_on(listener, handler)
    }

    /// Serves on a listener bound earlier (before a runtime existed).
    pub fn start_on(
        listener: std::net::TcpListener,
        handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static,
    ) -> Self {
        listener.set_nonblocking(true).expect("non-blocking");
        Self::serve_on(TcpListener::from_std(listener).expect("listener"), handler)
    }

    fn serve_on(
        listener: TcpListener,
        handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static,
    ) -> Self {
        let addr = listener.local_addr().expect("address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (closed_tx, closed_rx) = mpsc::unbounded_channel();
        let handler: Arc<Handler> = Arc::new(handler);
        let recorded = Arc::clone(&requests);
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let handler = Arc::clone(&handler);
                let recorded = Arc::clone(&recorded);
                let closed_tx = closed_tx.clone();
                tokio::spawn(async move {
                    let _ = serve(socket, handler, recorded, closed_tx).await;
                });
            }
        });
        Self {
            addr,
            requests,
            closed: tokio::sync::Mutex::new(closed_rx),
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("lock").clone()
    }

    /// Waits until a held-open connection is closed by the client; returns when that was.
    pub async fn wait_closed(&self, limit: Duration) -> Option<Instant> {
        let mut closed = self.closed.lock().await;
        tokio::time::timeout(limit, closed.recv())
            .await
            .ok()
            .flatten()
    }
}

async fn serve(
    mut socket: TcpStream,
    handler: Arc<Handler>,
    recorded: Arc<Mutex<Vec<Recorded>>>,
    closed_tx: mpsc::UnboundedSender<Instant>,
) -> std::io::Result<()> {
    let Some(request) = read_request(&mut socket).await? else {
        return Ok(());
    };
    let reply = handler(&request);
    recorded.lock().expect("lock").push(request);
    match reply {
        Reply::Raw {
            status,
            content_type,
            body,
        } => {
            let head = format!(
                "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reason(status),
                body.len()
            );
            socket.write_all(head.as_bytes()).await?;
            socket.write_all(&body).await?;
            socket.shutdown().await?;
        }
        Reply::Stream {
            status,
            content_type,
            chunks,
            delay,
            end,
        } => {
            let head = format!(
                "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                reason(status)
            );
            socket.write_all(head.as_bytes()).await?;
            socket.flush().await?;
            for chunk in chunks {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                let mut framed = format!("{:x}\r\n", chunk.len()).into_bytes();
                framed.extend_from_slice(&chunk);
                framed.extend_from_slice(b"\r\n");
                socket.write_all(&framed).await?;
                socket.flush().await?;
            }
            match end {
                StreamEnd::Clean => {
                    socket.write_all(b"0\r\n\r\n").await?;
                    socket.shutdown().await?;
                }
                StreamEnd::Abort => {}
                StreamEnd::HoldOpen => {
                    let mut buf = [0u8; 256];
                    let deadline = Instant::now() + Duration::from_secs(20);
                    while Instant::now() < deadline {
                        let read =
                            tokio::time::timeout(Duration::from_millis(50), socket.read(&mut buf))
                                .await;
                        // EOF or a reset: the client closed the connection.
                        if let Ok(Ok(0) | Err(_)) = read {
                            let _ = closed_tx.send(Instant::now());
                            break;
                        }
                    }
                }
            }
        }
        Reply::Silence(duration) => tokio::time::sleep(duration).await,
        Reply::Redirect { status, location } => {
            let head = format!(
                "HTTP/1.1 {status} {}\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                reason(status)
            );
            socket.write_all(head.as_bytes()).await?;
            socket.shutdown().await?;
        }
    }
    Ok(())
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

async fn read_request(socket: &mut TcpStream) -> std::io::Result<Option<Recorded>> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buffer[head_end + 4..].to_vec();
    while body.len() < length {
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Ok(Some(Recorded {
        method,
        target,
        headers,
        body,
    }))
}
