//! A tiny HTTP/1.1 server on 127.0.0.1 for the integration tests: it answers each
//! request with what a closure returns, counts connections and keeps the requests.

#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// One request as the server saw it.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    /// Path and query, as sent.
    pub target: String,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The path without the query.
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }

    /// One query parameter, decoded.
    pub fn query(&self, key: &str) -> Option<String> {
        let query = self.target.split_once('?')?.1;
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    }
}

/// What the server answers.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Wait this long before answering.
    pub delay: Duration,
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), content_type.into())],
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    pub fn html(body: impl Into<Vec<u8>>) -> Self {
        Self::new(200, "text/html; charset=utf-8", body)
    }

    pub fn json(body: impl Into<Vec<u8>>) -> Self {
        Self::new(200, "application/json", body)
    }

    pub fn status(status: u16) -> Self {
        Self::new(status, "text/plain", format!("status {status}"))
    }

    pub fn redirect(status: u16, location: &str) -> Self {
        Self {
            status,
            headers: vec![("Location".into(), location.into())],
            body: Vec::new(),
            delay: Duration::ZERO,
        }
    }

    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

pub struct TestServer {
    pub port: u16,
    connections: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Request>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TestServer {
    /// Starts a server on a free port of 127.0.0.1.
    pub async fn start(handler: impl Fn(&Request) -> Response + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("address").port();
        let connections = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler = Arc::new(handler);
        let (count, seen) = (connections.clone(), requests.clone());
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                count.fetch_add(1, Ordering::SeqCst);
                let handler = handler.clone();
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    let mut chunk = [0_u8; 4096];
                    while !buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                        }
                        if buffer.len() > 64 * 1024 {
                            return;
                        }
                    }
                    let head = String::from_utf8_lossy(&buffer).into_owned();
                    let mut lines = head.split("\r\n");
                    let mut first = lines.next().unwrap_or("").split(' ');
                    let request = Request {
                        method: first.next().unwrap_or("").to_string(),
                        target: first.next().unwrap_or("").to_string(),
                        headers: lines
                            .take_while(|l| !l.is_empty())
                            .filter_map(|l| l.split_once(':'))
                            .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
                            .collect(),
                    };
                    let response = handler(&request);
                    if let Ok(mut all) = seen.lock() {
                        all.push(request);
                    }
                    if !response.delay.is_zero() {
                        tokio::time::sleep(response.delay).await;
                    }
                    let mut head = format!("HTTP/1.1 {} Test\r\n", response.status);
                    for (name, value) in &response.headers {
                        head.push_str(&format!("{name}: {value}\r\n"));
                    }
                    head.push_str(&format!(
                        "Content-Length: {}\r\nConnection: close\r\n\r\n",
                        response.body.len()
                    ));
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&response.body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self {
            port,
            connections,
            requests,
            task,
        }
    }

    /// How many connections were accepted.
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().map(|r| r.clone()).unwrap_or_default()
    }

    /// `http://<host>:<port><path>`.
    pub fn url(&self, host: &str, path: &str) -> url::Url {
        url::Url::parse(&format!("http://{host}:{}{path}", self.port)).expect("url")
    }
}
