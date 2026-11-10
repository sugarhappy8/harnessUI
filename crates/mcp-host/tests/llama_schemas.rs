//! The schema hand-off to a real llama-server (ARCHITECTURE.md 8.3 step 1).
//!
//! llama-server turns every offered tool schema into a grammar, and one schema it cannot
//! convert fails the whole request. Each schema below is valid JSON Schema but was seen
//! to fail a request on llama-server b12110 as is (one of them, a `minLength` above its
//! `maxLength`, made it exhaust its memory). Their simplified copies must all be
//! accepted, one at a time and all together, and the copies the host offers for a
//! built-in server with such tools too.
//!
//! Needs `LLAMA_SERVER_BIN` (a llama-server binary) and `TEST_GGUF` (a small model with
//! a tool-calling chat template); skipped with a note when either is unset.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use mcp_host::{
    BuiltinServer, CallContext, ChatContext, Described, Host, HostOptions, ToolDef, ToolOutput,
    simplify_for_llama,
};
use runtime::spawn::{SpawnSpec, StdioMode};
use serde_json::{Map, Value, json};

/// Valid schemas that break a request on llama-server b12110 unless simplified. None of
/// them may be sent to the server as is: some exhaust its memory.
fn odd_schemas() -> Vec<(&'static str, Value)> {
    let field = |schema: Value| json!({"type": "object", "properties": {"f": schema}});
    vec![
        ("boolean_field", field(json!(true))),
        ("false_field", field(json!(false))),
        ("items_true", field(json!({"type": "array", "items": true}))),
        (
            "boolean_in_one_of",
            field(json!({"oneOf": [true, {"type": "string"}]})),
        ),
        ("empty_enum", field(json!({"enum": []}))),
        (
            "large_min_length",
            field(json!({"type": "string", "minLength": 5000})),
        ),
        (
            "large_min_items",
            field(json!({"type": "array", "items": {"type": "integer"}, "minItems": 3000})),
        ),
        (
            "min_length_above_max",
            field(json!({"type": "string", "minLength": 10, "maxLength": 1})),
        ),
        (
            "negative_min_items",
            field(json!({"type": "array", "minItems": -1})),
        ),
        (
            "fractional_min_length",
            field(json!({"type": "string", "minLength": 2.5})),
        ),
        (
            "huge_integer_bound",
            field(json!({"type": "integer", "maximum": 1e20})),
        ),
        (
            "missing_definition",
            field(json!({"$ref": "#/$defs/missing"})),
        ),
        ("anchor_reference", field(json!({"$ref": "#anchor"}))),
        (
            "remote_reference",
            field(json!({"$ref": "https://example.com/schema.json"})),
        ),
        (
            "contradicted_reference",
            json!({
                "type": "object",
                "$defs": {"name": {"type": "string", "minLength": 5}},
                "properties": {"f": {"$ref": "#/$defs/name", "maxLength": 2}}
            }),
        ),
        ("not_an_object_at_the_top", json!({"type": "string"})),
        (
            "recursive",
            json!({
                "type": "object",
                "$defs": {"node": {
                    "type": "object",
                    "properties": {
                        "children": {"type": "array", "items": {"$ref": "#/$defs/node"}}
                    }
                }},
                "properties": {"root": {"$ref": "#/$defs/node"}}
            }),
        ),
    ]
}

struct OddTools;

#[async_trait]
impl BuiltinServer for OddTools {
    fn name(&self) -> &str {
        "odd"
    }

    fn tools(&self) -> Vec<ToolDef> {
        odd_schemas()
            .into_iter()
            .map(|(name, schema)| ToolDef {
                name: name.to_owned(),
                description: format!("A tool whose schema is {name}"),
                input_schema: schema,
                destructive: None,
                read_only: None,
            })
            .collect()
    }

    async fn describe(&self, _: &str, _: &Map<String, Value>, _: &ChatContext) -> Described {
        Described::Invalid("not used".into())
    }

    async fn call(
        &self,
        _: &str,
        _: &Map<String, Value>,
        _: &policy::Action,
        _: CallContext,
    ) -> ToolOutput {
        ToolOutput::error("not used")
    }
}

struct Server {
    base: String,
    client: reqwest::Client,
    // Dropping it kills llama-server.
    _child: runtime::spawn::ManagedChild,
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .expect("a free port")
}

async fn start_server(bin: &str, model: &str) -> Server {
    let port = free_port();
    let bin = std::path::PathBuf::from(bin);
    let cwd = bin
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let spec = SpawnSpec::new(bin, cwd)
        .args([
            "-m",
            model,
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "-c",
            "2048",
            "-np",
            "1",
            "-t",
            "4",
            "--jinja",
        ])
        .stdout(StdioMode::Null)
        .stderr(StdioMode::Null);
    let child = runtime::spawn::spawn(spec).expect("llama-server starts");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(120))
        .build()
        .expect("client");
    let base = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Ok(response) = client.get(format!("{base}/health")).send().await
            && response.status().is_success()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "llama-server did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Server {
        base,
        client,
        _child: child,
    }
}

impl Server {
    /// Offers `tools` (name, parameters) and returns the HTTP status and the body.
    async fn offer(&self, tools: &[(String, Value)]) -> (u16, String) {
        let tools: Vec<Value> = tools
            .iter()
            .map(|(name, parameters)| {
                json!({"type": "function", "function": {
                    "name": name,
                    "description": "test tool",
                    "parameters": parameters
                }})
            })
            .collect();
        let body = json!({
            "messages": [{"role": "user", "content": "Call one of the tools."}],
            "tools": tools,
            "tool_choice": "required",
            "max_tokens": 4,
            "chat_template_kwargs": {"enable_thinking": false}
        });
        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.base))
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .expect("llama-server answers");
        let status = response.status().as_u16();
        (status, response.text().await.unwrap_or_default())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn simplified_schemas_are_accepted_by_llama_server() {
    let (Ok(bin), Ok(model)) = (
        std::env::var("LLAMA_SERVER_BIN"),
        std::env::var("TEST_GGUF"),
    ) else {
        eprintln!("note: LLAMA_SERVER_BIN or TEST_GGUF is not set; skipping the llama-server test");
        return;
    };
    let server = start_server(&bin, &model).await;

    // The server does check schemas: a safe one of the odd cases, as is, is refused.
    let raw = odd_schemas()
        .into_iter()
        .find(|(name, _)| *name == "items_true")
        .map(|(_, schema)| schema)
        .expect("case");
    let (status, _) = server.offer(&[("raw".into(), raw)]).await;
    assert_eq!(
        status, 400,
        "llama-server accepted a schema it used to refuse"
    );

    // Each simplified copy on its own.
    let mut all = Vec::new();
    for (name, schema) in odd_schemas() {
        let simple = simplify_for_llama(&schema);
        let (status, body) = server.offer(&[(name.to_owned(), simple.clone())]).await;
        assert_eq!(status, 200, "{name}: {simple} was refused: {body}");
        all.push((name.to_owned(), simple));
    }
    // All of them in one request, as a chat offers every enabled tool.
    let (status, body) = server.offer(&all).await;
    assert_eq!(status, 200, "all together were refused: {body}");

    // What the host offers for a server with such tools (those whose schema compiled).
    let host = Host::new(HostOptions::default());
    host.add_builtin(Arc::new(OddTools));
    let offered: Vec<(String, Value)> = host
        .tools()
        .into_iter()
        .filter_map(|t| host.llama_schema(&t.exposed).map(|s| (t.exposed, s)))
        .collect();
    assert!(!offered.is_empty());
    let (status, body) = server.offer(&offered).await;
    assert_eq!(status, 200, "the host's copies were refused: {body}");
}
