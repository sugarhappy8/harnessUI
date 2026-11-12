//! Pinned instructions in the app: who may change them, what a turn and a helper get, and
//! what happens when they cannot be read.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use agent::turn::{Approval, ApprovalRequest, Approver, ModelCaps, PolicyGate, RequestTemplate};
use agent::{ClientConfig, HistoryMessage, OpenAiClient, ProviderKind, Sanitizer, ToolInfo};
use async_trait::async_trait;
use mcp_host::{Described, ToolOutput};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use super::*;

const COMMANDS: [&str; 6] = [
    "list_pins",
    "add_pin",
    "edit_pin",
    "remove_pin",
    "move_pin",
    "set_pin_scope",
];

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every `.rs` file under `dir`.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

// ---- who may change a pin ----------------------------------------------------------------

/// The six commands are granted to the main window and to no other window, and each one
/// checks the calling window again.
#[test]
fn the_commands_are_for_the_main_window_only() {
    let caps = manifest().join("capabilities");
    let main = read(&caps.join("main.json"));
    for command in COMMANDS {
        let grant = format!("\"allow-{}\"", command.replace('_', "-"));
        assert!(main.contains(&grant), "main.json lacks {grant}");
    }
    for other in ["overlay.json", "indicator.json"] {
        let text = read(&caps.join(other));
        assert!(!text.contains("pin"), "{other} grants a pin command");
    }
    let list = read(&manifest().join("commands.list"));
    for command in COMMANDS {
        assert!(list.lines().any(|l| l.trim() == command), "{command}");
    }
    assert!(only_main_window(crate::MAIN_WINDOW).is_ok());
    for label in ["overlay", "indicator", "artifact", ""] {
        let err = only_main_window(label).expect_err(label);
        assert_eq!(err.code, "bad_request");
    }
}

/// No tool reaches pins: there is no built-in tool or group for them, and nothing but
/// this module calls the store's pin writers, so no tool result, model output, file or web
/// page can become a pin. Only the pin module of the store names the table.
#[test]
fn only_this_module_writes_a_pin_and_no_tool_does() {
    for entry in crate::tool_catalog::ENTRIES {
        assert!(!entry.name.contains("pin"), "tool {}", entry.name);
        assert!(
            !entry.purpose.to_lowercase().contains("pinned"),
            "{}",
            entry.name
        );
    }
    for group in crate::tool_catalog::GROUPS {
        assert!(!group.server.contains("pin"), "group {}", group.server);
    }
    let root = manifest().parent().expect("workspace").to_path_buf();
    let mut files = Vec::new();
    rust_files(&manifest().join("src"), &mut files);
    rust_files(&root.join("crates"), &mut files);
    assert!(
        files.len() > 50,
        "the scan found too few files: {}",
        files.len()
    );
    let mine = [
        manifest().join("src").join("pinned.rs"),
        manifest().join("src").join("pinned").join("tests.rs"),
        root.join("crates")
            .join("store")
            .join("src")
            .join("pinned.rs"),
    ];
    let writers = [
        ".add_pin(",
        ".edit_pin(",
        ".remove_pin(",
        ".move_pin(",
        ".set_pin_scope(",
    ];
    for file in files.iter().filter(|f| !mine.contains(f)) {
        let text = read(file);
        for writer in writers {
            assert!(
                !text.contains(writer),
                "{} calls {writer}: only src-tauri/src/pinned.rs may write a pin",
                file.display()
            );
        }
        let in_store = file.starts_with(root.join("crates").join("store"));
        let schema = file.ends_with("schema.rs");
        if in_store && !schema {
            assert!(
                !text.contains("pinned_instructions"),
                "{} reaches the pins table",
                file.display()
            );
        }
    }
}

// ---- what the Context tab gets -----------------------------------------------------------

#[tokio::test]
async fn the_list_carries_the_pins_in_order_and_the_limits() {
    let store = store::Store::open_in_memory().expect("store");
    let a = store
        .create_chat(store::NewChat {
            title: "a".into(),
            ..store::NewChat::default()
        })
        .await
        .expect("chat");
    let own = store
        .add_pin(a, false, "Use metric units.".into())
        .await
        .expect("pin");
    let every = store
        .add_pin(a, true, "Answer in German.".into())
        .await
        .expect("pin");
    let listed = listing(&store, a.0).await.expect("list");
    assert_eq!(
        serde_json::to_value(&listed).unwrap(),
        json!({
            "pins": [
                {"id": every.id, "text": "Answer in German.", "everyChat": true},
                {"id": own.id, "text": "Use metric units.", "everyChat": false},
            ],
            "max": 20,
            "maxChars": 1000,
        })
    );
    // A turn gets the same texts in the same order.
    assert_eq!(
        texts(store.pins_for_chat(a).await).expect("texts"),
        ["Answer in German.", "Use metric units."]
    );
}

/// A refusal is the store's own sentence; a missing chat or pin says so plainly.
#[test]
fn refusals_reach_the_tab_as_the_stores_sentence() {
    let refused = shown(store::StoreError::InvalidInput(
        "A pinned instruction cannot be empty.".into(),
    ));
    assert_eq!(refused.code, "pinned_refused");
    assert_eq!(refused.message, "A pinned instruction cannot be empty.");
    let gone = shown(store::StoreError::NotFound {
        what: "chat",
        id: 4,
    });
    assert_eq!(gone.message, "That chat no longer exists.");
    let gone = shown(store::StoreError::NotFound {
        what: "pinned instruction",
        id: 4,
    });
    assert_eq!(gone.code, "not_found");
    assert_eq!(gone.message, "That pinned instruction no longer exists.");
}

// ---- a turn ------------------------------------------------------------------------------

/// If the pins cannot be read the turn does not start: it never goes on without them.
#[test]
fn a_turn_whose_pins_cannot_be_read_does_not_start() {
    let err = texts(Err(store::StoreError::Closed)).expect_err("fails closed");
    assert_eq!(err.code, "pinned_unreadable");
    assert!(err.message.contains("nothing was sent"), "{}", err.message);
    assert_eq!(
        texts(Ok(Vec::new())).expect("no pins"),
        Vec::<String>::new()
    );
}

/// `turns::run` sets the pins with `?` (a failed read stops the turn) BEFORE it copies the
/// turn's input for helpers, and a turn the pins do not fit gets this module's advice.
#[test]
fn the_turn_sets_the_pins_before_the_helpers_copy_and_explains_a_refusal() {
    let turns = read(&manifest().join("src").join("turns.rs"));
    let line = "input.pinned = crate::pinned::for_turn(app, chat_id).await?;";
    let set = turns.find(line).expect("turns.rs sets the pins");
    assert_eq!(turns.matches("input.pinned").count(), 1);
    let copy = turns
        .find("let helper_input = input.clone();")
        .expect("the helper's copy");
    assert!(set < copy, "the pins are set after the helper's copy");
    assert!(
        turns.contains("(\"pinned_do_not_fit\", _) => crate::pinned::next_step(local),"),
        "failure_next_step has no arm for pinned_do_not_fit"
    );
    for local in [true, false] {
        assert!(next_step(local).contains("in the Context tab"));
    }
    assert!(next_step(true).contains("larger context"));
    assert!(next_step(false).contains("Settings → Endpoints"));
}

// ---- a helper ----------------------------------------------------------------------------

/// Answers every chat request with "done" and records the requests.
async fn endpoint() -> (OpenAiClient, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}/v1", listener.local_addr().expect("address"));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let recorded = recorded.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let (head_end, length) = loop {
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
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
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                if !head.contains("/chat/completions ") {
                    let _ = socket
                        .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                        .await;
                    return;
                }
                let body: Value = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
                recorded.lock().unwrap().push(body);
                let event = |v: Value| format!("data: {v}\n\n");
                let reply = [
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
                        .to_owned(),
                    event(json!({"choices": [{"index": 0, "delta": {"content": "done"}}]})),
                    event(json!({
                        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 300, "completion_tokens": 2}
                    })),
                    "data: [DONE]\n\n".to_owned(),
                ]
                .concat();
                let _ = socket.write_all(reply.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    let client = OpenAiClient::new(ClientConfig::new(url, ProviderKind::Generic)).expect("client");
    (client, requests)
}

struct NoTools;

#[async_trait]
impl agent::turn::ToolHost for NoTools {
    fn tools(&self) -> Vec<ToolInfo> {
        Vec::new()
    }
    fn validate(&self, _: &str, _: &Value) -> Result<(), Vec<String>> {
        Ok(())
    }
    fn llama_schema(&self, _: &str) -> Option<Value> {
        None
    }
    async fn describe(&self, _: &str, _: &Map<String, Value>) -> Described {
        Described::Invalid("no tools here".into())
    }
    async fn execute(
        &self,
        _: &str,
        _: &Map<String, Value>,
        _: &policy::Action,
        _: CancellationToken,
    ) -> ToolOutput {
        ToolOutput::error("no tools here")
    }
}

struct Refuse;

impl PolicyGate for Refuse {
    fn decide(&self, _: &str, _: &policy::Action, _: bool, _: bool) -> policy::Decision {
        policy::Decision::Refused {
            reason: "no tools here".into(),
        }
    }
}

#[async_trait]
impl Approver for Refuse {
    async fn ask(&self, _: ApprovalRequest) -> Approval {
        Approval::Cancelled
    }
}

/// A helper gets the chat's pins like the chat's system prompt: its request carries the
/// same pinned block, right after the system prompt, and nothing of the chat's messages.
#[tokio::test]
async fn a_helpers_request_carries_the_chats_pins() {
    let (client, requests) = endpoint().await;
    let mut input = agent::turn::TurnInput::new(
        vec![HistoryMessage::user("PARENT-QUESTION")],
        ModelCaps::default(),
        RequestTemplate {
            model: "fake-model".into(),
            ..Default::default()
        },
    );
    input.system_prompt = "BASE-SYSTEM-PROMPT".into();
    input.pinned = vec![
        "Always answer in German.".into(),
        "Use metric units.".into(),
    ];
    let helper = crate::subagent::Helper {
        chat_id: 7,
        client,
        sanitizer: Sanitizer::generic(),
        tools: Arc::new(NoTools),
        policy: Arc::new(Refuse),
        approver: Arc::new(Refuse),
        input,
        inherited: crate::subagent::Inherited::default(),
        progress: Arc::new(|_| {}),
        audit: Arc::new(|_| {}),
        parent_audit: None,
        result_cap: Arc::new(AtomicUsize::new(2000)),
    };
    let args = json!({"task": "Convert 3 miles."});
    let out = crate::subagent::run(
        helper,
        args.as_object().expect("object"),
        CancellationToken::new(),
    )
    .await;
    assert!(!out.is_error, "{out:?}");
    let requests = requests.lock().unwrap().clone();
    assert!(!requests.is_empty());
    for request in &requests {
        let system = request["messages"][0]["content"]
            .as_str()
            .expect("a system message")
            .to_owned();
        assert!(
            system.starts_with(
                "BASE-SYSTEM-PROMPT\n\n<pinned_instructions>\nThe user pinned these \
                 instructions. Follow them in every reply:\n1. Always answer in German.\n2. \
                 Use metric units.\n</pinned_instructions>"
            ),
            "{system}"
        );
        assert!(!request.to_string().contains("PARENT-QUESTION"));
    }
}
