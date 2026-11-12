//! WP3a end-to-end check (temporary, not part of the repository): the agent loop against the
//! pinned llama-server with the real small model. Gated on LLAMA_SERVER_BIN and TEST_GGUF.
//!
//! 1. "How long it may think": the same question with Short (1,024 tokens) and with an
//!    explicit tiny budget in the template; the reasoning the model writes stays within it.
//! 2. "A chat summarised at 32k continues at 4k": a summary of about 4,000 characters, on a
//!    model loaded with 4,096 tokens, is folded by the real model under the 2,048-character
//!    cap before the turn, and the turn answers.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use agent::{
    Approval, ApprovalRequest, Approver, ClientConfig, CompactionSummary, HistoryMessage,
    ModelCaps, OpenAiClient, PolicyGate, RequestTemplate, Sanitizer, ThinkingLimit, ToolHost,
    ToolInfo, TurnDeps, TurnEvent, TurnInput, TurnSink, TurnStatus, run_turn,
};
use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

struct Guard(Child, PathBuf, tempfile::TempDir);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[allow(clippy::disallowed_methods)]
fn start(bin: &Path, model: &Path, port: u16, key: &str) -> Guard {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("llama-server.log");
    let file = std::fs::File::create(&log).unwrap();
    let mut command = std::process::Command::new(bin); // spawn-ok: test
    command
        .args(["--host", "127.0.0.1", "--port", &port.to_string()])
        .args(["--no-webui", "--offline", "--jinja"])
        .arg("-m")
        .arg(model)
        .args(["-c", "4096", "-np", "1", "--device", "none", "--no-context-shift"])
        .env_clear()
        .env("LLAMA_API_KEY", key)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(file));
    if let Some(dir) = bin.parent() {
        command.current_dir(dir);
    }
    Guard(command.spawn().unwrap(), log, dir)
}

fn tail(path: &Path) -> String {
    let mut text = String::new();
    let _ = std::fs::File::open(path).map(|mut f| f.read_to_string(&mut text));
    text.lines().rev().take(30).collect::<Vec<_>>().join("\n")
}

struct NoTools;
#[async_trait]
impl ToolHost for NoTools {
    fn tools(&self) -> Vec<ToolInfo> {
        Vec::new()
    }
    fn validate(&self, _: &str, _: &Value) -> Result<(), Vec<String>> {
        Ok(())
    }
    fn llama_schema(&self, _: &str) -> Option<Value> {
        None
    }
    async fn describe(&self, _: &str, _: &Map<String, Value>) -> mcp_host::Described {
        mcp_host::Described::Invalid("none".into())
    }
    async fn execute(
        &self,
        _: &str,
        _: &Map<String, Value>,
        _: &policy::Action,
        _: CancellationToken,
    ) -> mcp_host::ToolOutput {
        mcp_host::ToolOutput::error("none")
    }
}
impl PolicyGate for NoTools {
    fn decide(&self, _: &str, _: &policy::Action, _: bool, _: bool) -> policy::Decision {
        policy::Decision::Refused { reason: "none".into() }
    }
}
#[async_trait]
impl Approver for NoTools {
    async fn ask(&self, _: ApprovalRequest) -> Approval {
        Approval::Cancelled
    }
}
#[derive(Default)]
struct Log(Mutex<Vec<TurnEvent>>);
impl TurnSink for Log {
    fn event(&self, e: TurnEvent) {
        self.0.lock().unwrap().push(e);
    }
}

async fn tokens(port: u16, key: &str, text: &str) -> usize {
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let body: Value = http
        .post(format!("http://127.0.0.1:{port}/tokenize"))
        .bearer_auth(key)
        .json(&json!({"content": text}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    body["tokens"].as_array().map_or(0, Vec::len)
}

#[tokio::test(flavor = "multi_thread")]
async fn wp3a_real_model() {
    let (Some(bin), Some(model)) = (std::env::var_os("LLAMA_SERVER_BIN"), std::env::var_os("TEST_GGUF")) else {
        eprintln!("skipped: LLAMA_SERVER_BIN and TEST_GGUF not set");
        return;
    };
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let key = "wp3a-key";
    let guard = start(Path::new(&bin), Path::new(&model), port, key);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Ok(r) = http.get(format!("http://127.0.0.1:{port}/health")).send().await
            && r.status().is_success()
        {
            break;
        }
        assert!(Instant::now() < deadline, "not healthy:\n{}", tail(&guard.1));
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let client = OpenAiClient::new(ClientConfig::llama_server(port, key)).unwrap();
    let sanitizer = Sanitizer::for_endpoint(&client).await;
    let caps = ModelCaps {
        tools: true,
        vision: false,
        n_ctx: 4_096,
        local: true,
        thinking: true,
        preserve_reasoning: false,
        images_in_tool_results: true,
    };
    let template = || {
        let mut t = RequestTemplate { model: "local".into(), ..RequestTemplate::default() };
        t.extra.insert("cache_prompt".into(), json!(true));
        t
    };

    // 1. How long it may think.
    let question = "A farmer has 17 sheep, 9 goats and 4 cows. He sells a third of the animals \
                    that have four legs and buys twice as many chickens as he has cows left. \
                    Think it through step by step, checking each step twice, then give the \
                    number of legs on the farm.";
    for (label, limit, explicit) in [
        ("Short (1,024)", ThinkingLimit::Short, None),
        ("template budget 48", ThinkingLimit::NoLimit, Some(48)),
    ] {
        let log = Log::default();
        let deps = TurnDeps { client: &client, tools: &NoTools, policy: &NoTools, approver: &NoTools, sink: &log, sanitizer: &sanitizer };
        let mut tpl = template();
        if let Some(n) = explicit {
            tpl.extra.insert("reasoning_budget_tokens".into(), json!(n));
        }
        let mut input = TurnInput::new(vec![HistoryMessage::user(question)], caps, tpl);
        input.thinking = true;
        input.thinking_limit = limit;
        let started = Instant::now();
        let outcome = tokio::time::timeout(Duration::from_secs(900), run_turn(deps, input, CancellationToken::new())).await.expect("too slow");
        let answer = outcome.messages.last().cloned().unwrap();
        let reasoning = answer.reasoning.clone().unwrap_or_default();
        let reasoning_tokens = tokens(port, key, &reasoning).await;
        let completion: u64 = outcome.usage.completion_tokens;
        println!(
            "THINKING {label}: status {:?}, {} s, reasoning {} tokens ({} chars), completion tokens {}, answer {:?}",
            outcome.status,
            started.elapsed().as_secs(),
            reasoning_tokens,
            reasoning.len(),
            completion,
            answer.text().chars().take(200).collect::<String>()
        );
        let budget = explicit.unwrap_or(1_024);
        // The server's budget sampler ends the thinking at the budget (plus its closing tag and message).
        assert!(reasoning_tokens <= budget + 64, "{label}: {reasoning_tokens} reasoning tokens");
    }

    // 2. A summary written at 32k on a model loaded with 4,096 tokens.
    let mut summary = String::from(
        "Goal: plan a vegetable garden for a small backyard in Lisbon.\nDecisions: raised beds; tomatoes, lettuce and basil; drip watering.\nWork done: ",
    );
    while summary.len() < 3_900 {
        summary.push_str("measured the beds at 1.2 by 2.4 metres and listed the seeds to buy; ");
    }
    summary.push_str("\nWhat is left: choose a watering timer.\nExact paths and identifiers: garden-plan.txt");
    let mut first = HistoryMessage::user("Help me plan my garden.");
    first.id = Some(1);
    let mut reply = HistoryMessage::assistant("Sure, let us start with the beds.");
    reply.id = Some(2);
    let log = Log::default();
    let deps = TurnDeps { client: &client, tools: &NoTools, policy: &NoTools, approver: &NoTools, sink: &log, sanitizer: &sanitizer };
    let mut input = TurnInput::new(
        vec![first, reply, HistoryMessage::user("What is left to do? Answer in one short sentence.")],
        caps,
        template(),
    );
    input.thinking = false;
    input.summary = Some(CompactionSummary { text: summary.clone(), covered_ids: vec![1, 2] });
    let started = Instant::now();
    let outcome = tokio::time::timeout(Duration::from_secs(900), run_turn(deps, input, CancellationToken::new())).await.expect("too slow");
    let events = log.0.lock().unwrap().clone();
    let folded: Vec<(u8, Option<String>, Vec<i64>)> = events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::Compacted { stage, summary, covered_ids } => Some((*stage, summary.clone(), covered_ids.clone())),
            _ => None,
        })
        .collect();
    let notes: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::AppNote { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    println!("FOLD: old summary {} chars; status {:?}; {} s; notes {:?}", summary.chars().count(), outcome.status, started.elapsed().as_secs(), notes);
    for (stage, text, covered) in &folded {
        println!("FOLD event: stage {stage}, covered {covered:?}, {} chars:\n{}\n----", text.as_deref().map_or(0, |t| t.chars().count()), text.as_deref().unwrap_or(""));
    }
    println!("FOLD answer: {:?}", outcome.messages.last().map(HistoryMessage::text));
    assert_eq!(outcome.status, TurnStatus::Completed);
    let used = outcome.summary.unwrap();
    assert!(used.text.chars().count() <= 2_048, "{}", used.text.chars().count());
    assert_eq!(used.covered_ids, vec![1, 2]);
    drop(guard);
}
