//! Test helpers: a folder that is removed when the test ends, and a scripted model
//! endpoint, so the harness, the probes and every benchmark task are proven without a
//! model.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agent::{ClientConfig, OpenAiClient, ProviderKind};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A fresh folder under the system's temporary folder, removed on drop.
pub struct TempFolder(PathBuf);

impl TempFolder {
    pub fn new(what: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("harness-evals-{what}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("a temporary folder");
        Self(dunce::canonicalize(&path).expect("its real path"))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What the scripted model answers to one request.
#[derive(Debug, Clone)]
pub enum Say {
    /// A plain answer.
    Text(String),
    /// Native tool calls.
    Calls(Vec<(String, Value)>),
    /// A tool call written into the answer's text, the way a model without native calls
    /// writes it.
    TextCall(String, Value),
    /// A word every 40 ms for a long time (for Stop).
    Slow,
    /// An HTTP error.
    Status(u16),
}

pub fn text(t: &str) -> Say {
    Say::Text(t.to_owned())
}

pub fn call(name: &str, args: Value) -> Say {
    Say::Calls(vec![(name.to_owned(), args)])
}

/// The scripted model: it is handed each chat request and says what to answer.
pub type Model = dyn Fn(&Value) -> Say + Send + Sync;

/// An OpenAI-compatible endpoint on a loopback port whose answers come from a [`Model`].
pub struct Endpoint {
    pub client: OpenAiClient,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Endpoint {
    pub async fn start(model: impl Fn(&Value) -> Say + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("http://{}/v1", listener.local_addr().expect("address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let model: Arc<Model> = Arc::new(model);
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let model = model.clone();
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    let _ = serve(socket, model, recorded).await;
                });
            }
        });
        let client =
            OpenAiClient::new(ClientConfig::new(url, ProviderKind::Generic)).expect("client");
        Self { client, requests }
    }

    /// An endpoint that plays a fixed list of answers, then says "Done.".
    pub async fn playing(script: Vec<Say>) -> Self {
        let script = Mutex::new(std::collections::VecDeque::from(script));
        Self::start(move |_| {
            script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| text("Done."))
        })
        .await
    }

    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

/// The text of every tool result in a request, oldest first.
pub fn tool_results(request: &Value) -> Vec<String> {
    request["messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .filter(|m| m["role"] == "tool")
                .map(|m| match &m["content"] {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The text of the request's first user message (the task).
pub fn task_of(request: &Value) -> String {
    request["messages"]
        .as_array()
        .and_then(|messages| messages.iter().find(|m| m["role"] == "user"))
        .map(|m| match &m["content"] {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

fn event(value: &Value) -> Vec<u8> {
    format!("data: {value}\n\n").into_bytes()
}

async fn serve(
    mut socket: TcpStream,
    model: Arc<Model>,
    recorded: Arc<Mutex<Vec<Value>>>,
) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let (head_end, length) = loop {
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            break (end + 4, length);
        }
    };
    while buf.len() < head_end + length {
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let first = head.lines().next().unwrap_or("");
    if !(first.starts_with("POST") && first.contains("/chat/completions ")) {
        socket
            .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
            .await?;
        return Ok(());
    }
    let request: Value = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
    let say = model(&request);
    let n = {
        let mut recorded = recorded.lock().unwrap();
        recorded.push(request);
        recorded.len()
    };
    if let Say::Status(status) = say {
        let body = json!({"error": {"message": "scripted failure"}}).to_string();
        let head = format!(
            "HTTP/1.1 {status} Scripted\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(head.as_bytes()).await?;
        return Ok(());
    }
    socket
        .write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
        )
        .await?;
    let delta = |delta: Value| event(&json!({"choices": [{"index": 0, "delta": delta}]}));
    let finish = |reason: &str| {
        event(&json!({
            "choices": [{"index": 0, "delta": {}, "finish_reason": reason}],
            "usage": {"prompt_tokens": 500, "completion_tokens": 10}
        }))
    };
    match say {
        Say::Text(text) => {
            socket.write_all(&delta(json!({"content": text}))).await?;
            socket.write_all(&finish("stop")).await?;
        }
        Say::TextCall(name, arguments) => {
            let text = format!(
                "<tool_call>\n{}\n</tool_call>",
                json!({"name": name, "arguments": arguments})
            );
            socket.write_all(&delta(json!({"content": text}))).await?;
            socket.write_all(&finish("stop")).await?;
        }
        Say::Calls(calls) => {
            for (index, (name, arguments)) in calls.iter().enumerate() {
                socket
                    .write_all(&delta(json!({"tool_calls": [{
                        "index": index,
                        "id": format!("call_{n}_{index}"),
                        "type": "function",
                        "function": {"name": name, "arguments": arguments.to_string()}
                    }]})))
                    .await?;
            }
            socket.write_all(&finish("tool_calls")).await?;
        }
        Say::Slow => {
            for _ in 0..1500 {
                if socket
                    .write_all(&delta(json!({"content": "word "})))
                    .await
                    .is_err()
                {
                    return Ok(());
                }
                let _ = socket.flush().await;
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            }
            socket.write_all(&finish("stop")).await?;
        }
        Say::Status(_) => {}
    }
    socket.write_all(b"data: [DONE]\n\n").await?;
    socket.shutdown().await
}
