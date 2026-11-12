//! Whole turns with the real agent loop, the real tools and the real policy engine, against
//! a scripted model: what a `call_tool` call looks like to the user (the approval card),
//! to the audit log, and to the tool.
//!
//! [`TurnTools`] wires [`Routing`] into the loop's tool host the way `turns::AppTools`
//! does: validate and describe through it, and run what [`Routing::route`] says.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use agent::turn::{
    Approval, ApprovalRequest, Approver, ModelCaps, PolicyGate, RequestTemplate, ToolHost,
    TurnDeps, TurnEvent, TurnInput, TurnSink, TurnStatus,
};
use agent::{ClientConfig, HistoryMessage, OpenAiClient, ProviderKind, Sanitizer, ToolInfo};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use super::*;

// ---- a scripted endpoint -----------------------------------------------------------------

enum Say {
    Text(&'static str),
    Call(&'static str, Value),
}

struct Endpoint {
    client: OpenAiClient,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Endpoint {
    async fn start(script: Vec<Say>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("http://{}/v1", listener.local_addr().expect("address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(VecDeque::from(script)));
        let recorded = requests.clone();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let script = script.clone();
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    let _ = serve(socket, script, recorded).await;
                });
            }
        });
        let client =
            OpenAiClient::new(ClientConfig::new(url, ProviderKind::Generic)).expect("client");
        Self { client, requests }
    }

    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

async fn serve(
    mut socket: TcpStream,
    script: Arc<Mutex<VecDeque<Say>>>,
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
    let request: Value = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
    let (say, n) = {
        let mut recorded = recorded.lock().unwrap();
        recorded.push(request);
        (script.lock().unwrap().pop_front(), recorded.len())
    };
    socket
        .write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
        )
        .await?;
    let event = |value: Value| format!("data: {value}\n\n").into_bytes();
    let finish = |reason: &str| {
        event(json!({
            "choices": [{"index": 0, "delta": {}, "finish_reason": reason}],
            "usage": {"prompt_tokens": 600, "completion_tokens": 10}
        }))
    };
    match say.unwrap_or(Say::Text("(script exhausted)")) {
        Say::Text(answer) => {
            socket
                .write_all(&event(
                    json!({"choices": [{"index": 0, "delta": {"content": answer}}]}),
                ))
                .await?;
            socket.write_all(&finish("stop")).await?;
        }
        Say::Call(name, arguments) => {
            socket
                .write_all(&event(
                    json!({"choices": [{"index": 0, "delta": {"tool_calls": [{
                        "index": 0, "id": format!("call_{n}"), "type": "function",
                        "function": {"name": name, "arguments": arguments.to_string()}
                    }]}}]}),
                ))
                .await?;
            socket.write_all(&finish("tool_calls")).await?;
        }
    }
    socket.write_all(b"data: [DONE]\n\n").await?;
    socket.shutdown().await
}

// ---- the turn's pieces -------------------------------------------------------------------

/// The loop's tool host, over one turn's lists.
struct TurnTools {
    turn: Turn,
    /// What reached a tool's own code: (tool, arguments).
    ran: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl ToolHost for TurnTools {
    fn tools(&self) -> Vec<ToolInfo> {
        self.turn
            .shown
            .iter()
            .map(|t| ToolInfo {
                name: t.exposed.clone(),
                description: t.def.description.clone(),
                input_schema: t.def.input_schema.clone(),
                read_only_hint: t.def.read_only,
                destructive_hint: t.def.destructive,
            })
            .collect()
    }
    fn validate(&self, name: &str, args: &Value) -> Result<(), Vec<String>> {
        self.turn.routing().validate(name, args)
    }
    fn llama_schema(&self, name: &str) -> Option<Value> {
        self.turn.host.llama_schema(name)
    }
    async fn describe(&self, name: &str, args: &Map<String, Value>) -> Described {
        self.turn
            .routing()
            .describe(name, args, &self.turn.chat, &HashMap::new())
            .await
    }
    async fn execute(
        &self,
        name: &str,
        args: &Map<String, Value>,
        action: &policy::Action,
        cancel: CancellationToken,
    ) -> ToolOutput {
        let routing = self.turn.routing();
        let target = match routing.route(name, args, action) {
            Route::Direct => None,
            Route::Find => {
                let builtin = tool_catalog::builtin_servers(&self.turn.host);
                let mut all = super::super::listed(&self.turn.shown, true, &builtin);
                all.extend(super::super::listed(&self.turn.hidden, false, &builtin));
                let query = args.get("query").and_then(Value::as_str).unwrap_or("");
                let found = search(query, &all);
                return ToolOutput::text(render(query, &found, &all, &[], &[], &|n, v| {
                    self.turn.host.validate(n, v)
                }));
            }
            Route::Refused(why) => return ToolOutput::error(why),
            Route::Call(target) => Some(target),
        };
        let (name, args) = match &target {
            Some(target) => (target.name.as_str(), &target.args),
            None => (name, args),
        };
        self.ran
            .lock()
            .unwrap()
            .push((name.to_string(), Value::Object(args.clone())));
        self.turn
            .host
            .call(
                name,
                args,
                action,
                CallContext {
                    chat: self.turn.chat.clone(),
                    cancel,
                    progress: None,
                },
            )
            .await
    }
}

/// The real policy engine with one chat's grants, as `turns::AppPolicy` asks it.
struct Gate {
    grants: BTreeSet<Scope>,
    host: Host,
}

impl PolicyGate for Gate {
    fn decide(
        &self,
        tool: &str,
        action: &policy::Action,
        recovered: bool,
        untrusted_in_turn: bool,
    ) -> Decision {
        let context = TurnContext {
            tool_enabled: true,
            server_ready: self.host.tool_ready(tool),
            recovered_from_text: recovered,
            untrusted_in_turn,
            ..Default::default()
        };
        policy::evaluate(action, &context, &self.grants)
    }
}

/// Answers every card the same way and keeps what it was shown: the tool name as the
/// card prints it (`turns::AppApprover`), and the action.
struct Cards {
    answer: Approval,
    shown: Mutex<Vec<(String, policy::Action, CardKind)>>,
}

#[async_trait]
impl Approver for Cards {
    async fn ask(&self, request: ApprovalRequest) -> Approval {
        self.shown.lock().unwrap().push((
            real_tool(&request.tool, Some(&request.action)),
            request.action.clone(),
            request.card,
        ));
        self.answer.clone()
    }
}

/// Builds the audit rows the way the turn's sink does (`turns::audit_entry`).
#[derive(Default)]
struct Audit {
    started: Mutex<HashMap<String, Value>>,
    rows: Mutex<Vec<(&'static str, store::AuditEntry)>>,
    results: Mutex<Vec<agent::ResultRecord>>,
}

impl TurnSink for Audit {
    fn event(&self, event: TurnEvent) {
        match event {
            TurnEvent::ToolCallStarted { call } => {
                self.started
                    .lock()
                    .unwrap()
                    .insert(call.id.clone(), Value::Object(call.arguments));
            }
            TurnEvent::ToolCallFinished {
                result,
                decision,
                action,
                duration_ms,
                ..
            } => {
                let arguments = self
                    .started
                    .lock()
                    .unwrap()
                    .remove(&result.call_id)
                    .unwrap_or(Value::Null);
                self.rows.lock().unwrap().push(crate::turns::audit_entry(
                    7,
                    arguments,
                    &result,
                    &decision,
                    action.as_ref(),
                    duration_ms,
                ));
                self.results.lock().unwrap().push(result);
            }
            _ => {}
        }
    }
}

struct Ran {
    status: TurnStatus,
    requests: Vec<Value>,
    cards: Vec<(String, policy::Action, CardKind)>,
    rows: Vec<(&'static str, store::AuditEntry)>,
    results: Vec<agent::ResultRecord>,
    ran: Vec<(String, Value)>,
    dir: PathBuf,
    /// Keeps the folder until the test is done looking at it.
    _tools: TurnTools,
}

/// One turn: `shown` in the model's list, `hidden` on but kept back, everything else off.
/// `grants` are the chat's grants, given the chat's approved folder.
async fn run_turn(
    shown: &[&str],
    hidden: &[&str],
    grants: fn(&str) -> Vec<Scope>,
    answer: Approval,
    script: Vec<Say>,
) -> Ran {
    let endpoint = Endpoint::start(script).await;
    let turn = Turn::new(shown, hidden);
    let dir = turn.dir.clone();
    let gate = Gate {
        grants: grants(&dir.display().to_string()).into_iter().collect(),
        host: turn.host.clone(),
    };
    let tools = TurnTools {
        turn,
        ran: Mutex::default(),
    };
    let cards = Cards {
        answer,
        shown: Mutex::default(),
    };
    let audit = Audit::default();
    let sanitizer = Sanitizer::generic();
    let mut input = TurnInput::new(
        vec![HistoryMessage::user("Do the thing.")],
        ModelCaps {
            n_ctx: 32_768,
            ..Default::default()
        },
        RequestTemplate {
            model: "fake-model".into(),
            ..Default::default()
        },
    );
    input.system_prompt = "You are a test.".into();
    let outcome = agent::turn::run_turn(
        TurnDeps {
            client: &endpoint.client,
            tools: &tools,
            policy: &gate,
            approver: &cards,
            sink: &audit,
            sanitizer: &sanitizer,
        },
        input,
        CancellationToken::new(),
    )
    .await;
    let ran = tools.ran.lock().unwrap().clone();
    Ran {
        status: outcome.status,
        requests: endpoint.requests(),
        cards: cards.shown.into_inner().unwrap(),
        rows: audit.rows.into_inner().unwrap(),
        results: audit.results.into_inner().unwrap(),
        ran,
        dir,
        _tools: tools,
    }
}

fn offered(request: &Value) -> Vec<String> {
    request["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn no_grants(_: &str) -> Vec<Scope> {
    Vec::new()
}

const SHOWN: &[&str] = &["read_file", "list_dir", "find_tools", "call_tool"];
const HIDDEN: &[&str] = &["write_file", "delete_file", "git_status"];

// ---- the tests -----------------------------------------------------------------------------

/// The user sees the card of the tool that runs, the audit log gets that tool's row, and
/// the tool's own code runs with its own arguments.
#[tokio::test]
async fn a_hidden_tool_run_through_call_tool_asks_and_is_logged_as_itself() {
    let write = json!({"path": "new.txt", "content": "hello"});
    let ran = run_turn(
        SHOWN,
        HIDDEN,
        no_grants,
        Approval::AllowOnce,
        vec![
            Say::Call(CALL_TOOL, json!({"name": "write_file", "arguments": write})),
            Say::Text("Done."),
        ],
    )
    .await;
    assert_eq!(ran.status, TurnStatus::Completed);

    // One card: the write's own.
    assert_eq!(ran.cards.len(), 1, "{:?}", ran.cards);
    let (tool, action, card) = &ran.cards[0];
    assert_eq!(tool, "write_file");
    assert_eq!(
        (action.server.as_str(), action.tool.as_str()),
        ("files", "write_file")
    );
    assert_eq!(*card, CardKind::Grantable);
    assert!(
        action.preview.contains("hello"),
        "the card shows the write: {}",
        action.preview
    );
    assert_eq!(
        action.scope,
        Scope::FileWrite {
            folder: ran.dir.display().to_string()
        }
    );

    // The file tool ran, with the tool's arguments.
    assert_eq!(ran.ran, vec![("write_file".to_string(), write.clone())]);
    assert_eq!(
        std::fs::read_to_string(ran.dir.join("new.txt")).expect("written"),
        "hello"
    );

    // The audit row is the row of a direct call to write_file.
    assert_eq!(ran.rows.len(), 1);
    let (label, row) = &ran.rows[0];
    assert_eq!(*label, "allowed once");
    assert_eq!(
        (row.server.as_str(), row.tool.as_str()),
        ("files", "write_file")
    );
    assert_eq!(row.arguments, write);
    assert_eq!(row.decision, store::AuditDecision::AllowOnce);
    assert_eq!(row.decided_by, "user");
    assert_eq!(row.result.as_ref().map(|r| r.status.as_str()), Some("ok"));

    // The tool list of the turn is what the model was shown, in every request.
    assert_eq!(ran.requests.len(), 2);
    for request in &ran.requests {
        assert_eq!(offered(request), SHOWN, "the list changed within the turn");
    }
}

/// Deny means the tool does not run, and the log says who refused what.
#[tokio::test]
async fn a_denied_card_runs_nothing() {
    let ran = run_turn(
        SHOWN,
        HIDDEN,
        no_grants,
        Approval::Deny {
            note: Some("not now".into()),
        },
        vec![
            Say::Call(
                CALL_TOOL,
                json!({"name": "write_file", "arguments": {"path": "new.txt", "content": "hello"}}),
            ),
            Say::Text("Understood."),
        ],
    )
    .await;
    assert_eq!(ran.cards.len(), 1);
    assert_eq!(ran.cards[0].0, "write_file");
    assert!(ran.ran.is_empty(), "{:?}", ran.ran);
    assert!(!ran.dir.join("new.txt").exists());
    let (label, row) = &ran.rows[0];
    assert_eq!(*label, "denied");
    assert_eq!(row.tool, "write_file");
    assert_eq!(row.decision, store::AuditDecision::Denied);
    assert_eq!(row.note.as_deref(), Some("not now"));
}

/// A grant covers exactly what it covers for a direct call: the write runs without a card,
/// a delete in the same folder still asks.
#[tokio::test]
async fn grants_cover_the_same_as_for_a_direct_call() {
    let script = || {
        vec![
            Say::Call(
                CALL_TOOL,
                json!({"name": "write_file", "arguments": {"path": "new.txt", "content": "x"}}),
            ),
            Say::Call(
                CALL_TOOL,
                json!({"name": "delete_file", "arguments": {"path": "notes.txt"}}),
            ),
            Say::Text("Done."),
        ]
    };
    // No grant: both ask, and the delete offers no chat-wide grant.
    let ran = run_turn(SHOWN, HIDDEN, no_grants, Approval::AllowOnce, script()).await;
    let asked: Vec<(&str, CardKind)> = ran
        .cards
        .iter()
        .map(|(tool, _, card)| (tool.as_str(), *card))
        .collect();
    assert_eq!(
        asked,
        [
            ("write_file", CardKind::Grantable),
            ("delete_file", CardKind::OnceOnly)
        ]
    );
    assert!(
        !ran.dir.join("notes.txt").exists(),
        "the allowed delete did not run"
    );

    // "Allow for this chat" on a write (a grant for writes in the folder): the write
    // runs without a card, the delete still asks.
    fn writes_granted(folder: &str) -> Vec<Scope> {
        vec![Scope::FileWrite {
            folder: folder.to_string(),
        }]
    }
    let ran = run_turn(SHOWN, HIDDEN, writes_granted, Approval::AllowOnce, script()).await;
    let asked: Vec<&str> = ran.cards.iter().map(|(tool, _, _)| tool.as_str()).collect();
    assert_eq!(
        asked,
        ["delete_file"],
        "the grant covered more, or less, than a write"
    );
    assert_eq!(
        std::fs::read_to_string(ran.dir.join("new.txt")).expect("written"),
        "x"
    );
    let logged: Vec<(&str, &str)> = ran
        .rows
        .iter()
        .map(|(label, row)| (row.tool.as_str(), *label))
        .collect();
    assert_eq!(
        logged,
        [
            ("write_file", "allowed by an earlier grant"),
            ("delete_file", "allowed once")
        ]
    );

    // A grant for another folder covers nothing here.
    fn elsewhere(_: &str) -> Vec<Scope> {
        vec![Scope::FileWrite {
            folder: "/somewhere/else".to_string(),
        }]
    }
    let ran = run_turn(SHOWN, HIDDEN, elsewhere, Approval::AllowOnce, script()).await;
    assert_eq!(ran.cards.len(), 2);
}

/// A tool that is switched off is never run and never asked about, whatever the model
/// writes: the call is an invalid one, like a call to a tool that does not exist.
#[tokio::test]
async fn a_tool_that_is_off_is_not_run_and_not_asked_about() {
    let ran = run_turn(
        SHOWN,
        HIDDEN,
        no_grants,
        Approval::AllowOnce,
        vec![
            Say::Call(
                CALL_TOOL,
                json!({"name": "run_command", "arguments": {"command": "echo hi"}}),
            ),
            Say::Call(CALL_TOOL, json!({"name": "git_push", "arguments": {}})),
            // A direct call to it is no better.
            Say::Call("run_command", json!({"command": "echo hi"})),
            Say::Text("I cannot do that."),
        ],
    )
    .await;
    assert_eq!(ran.status, TurnStatus::Completed);
    assert!(ran.cards.is_empty(), "a card was shown: {:?}", ran.cards);
    assert!(ran.ran.is_empty(), "something ran: {:?}", ran.ran);
    assert_eq!(ran.results.len(), 3);
    let said: Vec<String> = ran.results.iter().map(|r| r.notes.join(" ")).collect();
    assert!(
        said[0].contains("run_command is switched off in this chat"),
        "{}",
        said[0]
    );
    assert!(said[0].contains("Tools panel"), "{}", said[0]);
    assert_eq!(
        said[0],
        "Invalid arguments for call_tool: run_command is switched off in this chat, so it was \
         not run. The user can turn it on in the Tools panel. The tool was not run; fix the \
         arguments and call it again."
    );
    assert!(said[1].contains("git_push is switched off"), "{}", said[1]);
    assert!(said[2].contains("No such tool: run_command"), "{}", said[2]);
    // Logged as what was tried, and as refused.
    let logged: Vec<(&str, &str, store::AuditDecision)> = ran
        .rows
        .iter()
        .map(|(_, row)| (row.tool.as_str(), row.server.as_str(), row.decision))
        .collect();
    assert_eq!(
        logged,
        [
            ("call_tool", "", store::AuditDecision::RefusedByPolicy),
            ("call_tool", "", store::AuditDecision::RefusedByPolicy),
            ("run_command", "", store::AuditDecision::RefusedByPolicy),
        ]
    );
    assert_eq!(
        ran.rows[0].1.arguments,
        json!({"name": "run_command", "arguments": {"command": "echo hi"}})
    );
}

/// `call_tool` cannot be used to reach itself, `find_tools` or a helper, even when they
/// are on in the chat.
#[tokio::test]
async fn call_tool_does_not_reach_itself_or_a_helper() {
    let ran = run_turn(
        &["read_file", "find_tools", "call_tool", "run_subagent"],
        &["write_file"],
        no_grants,
        Approval::AllowOnce,
        vec![
            Say::Call(
                CALL_TOOL,
                json!({"name": "call_tool", "arguments": {"name": "write_file", "arguments": {"path": "a.txt", "content": "x"}}}),
            ),
            Say::Call(
                CALL_TOOL,
                json!({"name": "run_subagent", "arguments": {"task": "Write a.txt."}}),
            ),
            Say::Text("No."),
        ],
    )
    .await;
    assert!(ran.cards.is_empty() && ran.ran.is_empty());
    assert!(!ran.dir.join("a.txt").exists());
    for result in &ran.results {
        assert!(result.notes.join(" ").contains("cannot run"), "{result:?}");
    }
    assert!(ran.rows.iter().all(|(_, row)| row.tool == "call_tool"));
}

/// `find_tools` never asks, finds what is on, and is logged as needing no approval. What
/// it found is then run through `call_tool`, here the tool that always asks: the card is
/// the delete's own, with no chat-wide grant on offer.
#[tokio::test]
async fn find_then_call_shows_the_found_tools_own_card() {
    let ran = run_turn(
        SHOWN,
        HIDDEN,
        no_grants,
        Approval::AllowOnce,
        vec![
            Say::Call(FIND_TOOLS, json!({"query": "delete a file"})),
            Say::Call(
                CALL_TOOL,
                json!({"name": "delete_file", "arguments": {"path": "notes.txt"}}),
            ),
            Say::Text("Done."),
        ],
    )
    .await;
    assert_eq!(ran.status, TurnStatus::Completed);
    // The search result names the tool and how to run it; no card for the search.
    let found = &ran.results[0];
    assert!(
        found.text.contains("delete_file [Files]:"),
        "{}",
        found.text
    );
    assert!(
        found
            .text
            .contains("Run it with call_tool: name \"delete_file\""),
        "{}",
        found.text
    );
    assert!(
        !found.text.contains("git_status"),
        "a tool the search is not about was listed: {}",
        found.text
    );
    assert_eq!(ran.cards.len(), 1, "{:?}", ran.cards);
    let (tool, action, card) = &ran.cards[0];
    assert_eq!(tool, "delete_file");
    assert_eq!(action.server, "files");
    assert_eq!(*card, CardKind::OnceOnly);
    let rows: Vec<(&str, &str, &str)> = ran
        .rows
        .iter()
        .map(|(label, row)| (row.tool.as_str(), row.server.as_str(), *label))
        .collect();
    assert_eq!(
        rows,
        [
            ("find_tools", "toolfinder", "no approval needed"),
            ("delete_file", "files", "allowed once"),
        ]
    );
    assert_eq!(
        ran.ran,
        vec![("delete_file".to_string(), json!({"path": "notes.txt"}))]
    );
    assert!(!ran.dir.join("notes.txt").exists());
    // The third request (after both calls) still carries the same list.
    assert_eq!(ran.requests.len(), 3);
    assert!(ran.requests.iter().all(|r| offered(r) == SHOWN));
}
