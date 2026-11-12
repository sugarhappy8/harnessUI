//! Conversation memory (0.0.6, item 1) against the real llama-server and a small thinking
//! model: the parts of the agent loop that only a real model can show.
//!
//! Gated on `LLAMA_SERVER_BIN` (the pinned llama-server) and `TEST_GGUF` (a small GGUF model
//! that can think, such as Qwen3 0.6B); skipped with a note when either is unset, so a
//! machine without them still passes. CI sets both on Linux, Windows and macOS.
//!
//! The server is started through the runtime crate as the app starts it ([`runtime::start`]
//! with the processor backend found next to `LLAMA_SERVER_BIN`, an explicit context of
//! 4,096 tokens and the app's own arguments, environment and process group), never with a
//! process built here (`scripts/check-spawn.sh`).
//!
//! Checks, in one server:
//! 1. A chat summarised while the model remembered 32,768 tokens continues at 4,096: its
//!    summary (about 4,000 characters, twice the 2,048 a summary may keep at 4,096 tokens)
//!    is rewritten by the model under the smaller cap before the turn measures anything;
//!    the stage-2 event carries the new summary for the same messages and the turn answers.
//! 2. How long it may think: with Short, each request carries a thinking budget of 1,024
//!    tokens (min(1,024, half the output limit)), and the server ends the model's reasoning
//!    there (plus the closing tag it forces).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use agent::{
    Approval, ApprovalRequest, Approver, ClientConfig, CompactionSummary, HistoryMessage,
    ModelCaps, OpenAiClient, PolicyGate, RequestTemplate, Sanitizer, ThinkingLimit, ToolHost,
    ToolInfo, TurnDeps, TurnEvent, TurnInput, TurnOutcome, TurnSink, TurnStatus, run_turn,
};
use async_trait::async_trait;
use runtime::llama::server::loopback_client_builder;
use runtime::{
    Backend, BackendKind, GpuUse, LaunchConfig, RunningServer, RuntimeLayout, StartSpec,
    discover, free_loopback_port, random_api_key, start,
};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

/// The context the server is started with: what a model "remembers" after the user chose
/// a smaller size than the one a chat was summarised at.
const CONTEXT: u32 = 4_096;
/// The most characters a summary may keep at [`CONTEXT`] (`ContextBudget::summary_max_chars`).
const SUMMARY_CAP_AT_4K: usize = 2_048;
/// How long one turn may take on a slow processor before the test gives up.
const TURN_TIMEOUT: Duration = Duration::from_secs(1_200);
/// Tokens the server may add after the budget ends the reasoning (the closing tag it
/// forces, and the token the budget was reached on).
const BUDGET_SLACK: usize = 64;

fn env_paths(test: &str) -> Option<(PathBuf, PathBuf)> {
    let bin = std::env::var_os("LLAMA_SERVER_BIN").filter(|v| !v.is_empty());
    let gguf = std::env::var_os("TEST_GGUF").filter(|v| !v.is_empty());
    match (bin, gguf) {
        (Some(bin), Some(gguf)) => Some((PathBuf::from(bin), PathBuf::from(gguf))),
        _ => {
            eprintln!("skipping {test}: set LLAMA_SERVER_BIN and TEST_GGUF to run it");
            None
        }
    }
}

/// The processor backend of the directory holding `LLAMA_SERVER_BIN`, found the way the app
/// finds a development runtime.
fn cpu_backend(bin: &Path) -> Backend {
    let dir = bin
        .parent()
        .expect("LLAMA_SERVER_BIN has a parent directory")
        .to_path_buf();
    let layout = RuntimeLayout {
        installed_dir: None,
        packs_dir: dir.join("no-packs-here"),
        dev_override: Some(dir),
    };
    discover(&layout)
        .into_iter()
        .find(|backend| backend.kind == BackendKind::Cpu)
        .unwrap_or_else(|| panic!("no processor backend next to {}", bin.display()))
}

/// Starts the model on the processor with [`CONTEXT`] tokens, as the app would.
async fn start_model(bin: &Path, gguf: &Path) -> RunningServer {
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let spec = StartSpec {
        backend: cpu_backend(bin),
        launch: LaunchConfig {
            gpu: GpuUse::Off,
            cache_ram_mib: 128,
            ..LaunchConfig::new(
                gguf.to_path_buf(),
                CONTEXT,
                free_loopback_port().expect("a free port"),
                random_api_key(),
            )
        },
        load_timeout: Duration::from_secs(180),
    };
    match start(spec, events).await {
        Ok(server) => server,
        Err(error) => panic!("start failed: {error}\n{}", error.stderr_tail().join("\n")),
    }
}

/// No tools: the checks here are about memory and thinking, not tool calls.
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
        mcp_host::Described::Invalid("no tools here".into())
    }
    async fn execute(
        &self,
        _: &str,
        _: &Map<String, Value>,
        _: &policy::Action,
        _: CancellationToken,
    ) -> mcp_host::ToolOutput {
        mcp_host::ToolOutput::error("no tools here")
    }
}

impl PolicyGate for NoTools {
    fn decide(&self, _: &str, _: &policy::Action, _: bool, _: bool) -> policy::Decision {
        policy::Decision::Refused {
            reason: "no tools here".into(),
        }
    }
}

#[async_trait]
impl Approver for NoTools {
    async fn ask(&self, _: ApprovalRequest) -> Approval {
        Approval::Cancelled
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<TurnEvent>>);

impl TurnSink for Events {
    fn event(&self, event: TurnEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Events {
    fn all(&self) -> Vec<TurnEvent> {
        self.0.lock().unwrap().clone()
    }
}

/// One turn against the real server, within [`TURN_TIMEOUT`].
async fn turn(
    client: &OpenAiClient,
    sanitizer: &Sanitizer,
    events: &Events,
    input: TurnInput,
) -> TurnOutcome {
    let deps = TurnDeps {
        client,
        tools: &NoTools,
        policy: &NoTools,
        approver: &NoTools,
        sink: events,
        sanitizer,
    };
    tokio::time::timeout(
        TURN_TIMEOUT,
        run_turn(deps, input, CancellationToken::new()),
    )
    .await
    .expect("the turn ends in time")
}

/// The number of tokens in `text`, as the server counts them.
async fn count_tokens(server: &RunningServer, text: &str) -> usize {
    let body: Value = loopback_client_builder()
        .build()
        .expect("client")
        .post(format!("{}/tokenize", server.base_url()))
        .bearer_auth(server.api_key())
        .json(&json!({"content": text}))
        .send()
        .await
        .expect("tokenize")
        .json()
        .await
        .expect("tokenize JSON");
    body["tokens"].as_array().map_or(0, Vec::len)
}

/// A summary as a context of 32,768 tokens leaves it: near its cap there (4,096
/// characters), about twice what [`CONTEXT`] allows.
fn summary_written_at_32k() -> String {
    let mut text = String::from(
        "Goal: plan a vegetable garden for a small backyard.\nDecisions: raised beds; \
         tomatoes, lettuce and basil; drip watering.\nWork done: ",
    );
    while text.len() < 3_900 {
        text.push_str("measured the beds at 1.2 by 2.4 metres and listed the seeds to buy; ");
    }
    text.push_str(
        "\nWhat is left: choose a watering timer.\nExact paths and identifiers: garden-plan.txt",
    );
    assert!(text.chars().count() > SUMMARY_CAP_AT_4K);
    text
}

#[tokio::test(flavor = "multi_thread")]
async fn conversation_memory_with_the_real_model() {
    let Some((bin, gguf)) = env_paths("conversation_memory_with_the_real_model") else {
        return;
    };
    let server = start_model(&bin, &gguf).await;
    assert_eq!(server.props().n_ctx, CONTEXT);
    let client = OpenAiClient::new(ClientConfig::llama_server(server.port(), server.api_key()))
        .expect("client");
    let sanitizer = Sanitizer::for_endpoint(&client).await;
    let caps = ModelCaps {
        tools: server.props().chat_template_caps.supports_tool_calls,
        vision: false,
        n_ctx: CONTEXT,
        local: true,
        thinking: true,
        preserve_reasoning: false,
        images_in_tool_results: true,
    };
    let template = RequestTemplate {
        model: runtime::MODEL_ALIAS.into(),
        ..RequestTemplate::default()
    };

    // 1. A chat summarised at 32k continues at 4k.
    let old_summary = summary_written_at_32k();
    let mut first = HistoryMessage::user("Help me plan my garden.");
    first.id = Some(1);
    let mut reply = HistoryMessage::assistant("Sure, let us start with the beds.");
    reply.id = Some(2);
    let mut input = TurnInput::new(
        vec![
            first,
            reply,
            HistoryMessage::user("What is left to do? Answer in one short sentence."),
        ],
        caps,
        template.clone(),
    );
    input.thinking = false;
    input.summary = Some(CompactionSummary {
        text: old_summary.clone(),
        covered_ids: vec![1, 2],
    });
    let events = Events::default();
    let outcome = turn(&client, &sanitizer, &events, input).await;
    let events = events.all();
    assert_eq!(outcome.status, TurnStatus::Completed, "{events:?}");
    let folded: Vec<(u8, Option<String>, Vec<i64>)> = events
        .iter()
        .filter_map(|event| match event {
            TurnEvent::Compacted {
                stage,
                summary,
                covered_ids,
            } => Some((*stage, summary.clone(), covered_ids.clone())),
            _ => None,
        })
        .collect();
    eprintln!(
        "fold: {} characters became {:?}",
        old_summary.chars().count(),
        folded
    );
    // The model wrote the shorter summary (not the cut the turn falls back to): one stage-2
    // event, for the same messages, within the cap.
    assert_eq!(folded.len(), 1, "{folded:?}");
    let (stage, text, covered) = &folded[0];
    assert_eq!(*stage, 2);
    assert_eq!(covered, &[1, 2]);
    let text = text.as_deref().expect("the folded summary");
    assert!(!text.trim().is_empty());
    assert!(
        text.chars().count() <= SUMMARY_CAP_AT_4K,
        "{}",
        text.chars().count()
    );
    let used = outcome.summary.expect("the summary the turn used");
    assert_eq!(used.text, text);
    assert_eq!(used.covered_ids, [1, 2]);
    let answer = outcome.messages.last().expect("an answer");
    assert!(!answer.text().trim().is_empty(), "{answer:?}");

    // 2. How long it may think: Short is 1,024 tokens here (half the output limit is more).
    let question = "A farmer has 17 sheep, 9 goats and 4 cows. He sells a third of the animals \
                    that have four legs and buys twice as many chickens as he has cows left. \
                    Think it through step by step, checking each step twice, then give the \
                    number of legs on the farm.";
    let mut input = TurnInput::new(
        vec![HistoryMessage::user(question)],
        caps,
        template.clone(),
    );
    input.thinking = true;
    input.thinking_limit = ThinkingLimit::Short;
    let events = Events::default();
    let outcome = turn(&client, &sanitizer, &events, input).await;
    assert!(
        matches!(outcome.status, TurnStatus::Completed),
        "{:?}",
        outcome.status
    );
    let prompt = outcome.usage.last_prompt_tokens.unwrap_or(0);
    let budget = ThinkingLimit::Short.budget(CONTEXT.saturating_sub(prompt));
    assert_eq!(budget, 1_024, "a short prompt leaves more than 2,048 tokens");
    let reasoning = outcome
        .messages
        .iter()
        .filter_map(|m| m.reasoning.clone())
        .collect::<Vec<_>>()
        .join("\n");
    let reasoning_tokens = count_tokens(&server, &reasoning).await;
    eprintln!(
        "thinking: {reasoning_tokens} reasoning tokens for a budget of {budget}; {} completion \
         tokens",
        outcome.usage.completion_tokens
    );
    assert!(
        reasoning_tokens <= usize::try_from(budget).unwrap() + BUDGET_SLACK,
        "{reasoning_tokens} reasoning tokens for a budget of {budget}"
    );

    server.stop().await;
}
