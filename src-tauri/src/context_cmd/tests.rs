//! The Context tab's view: built from the loop's own preview, plain text, no secrets, and
//! the shared assembly changes nothing in its preview mode.

use agent::turn::{ModelCaps, RequestTemplate, TurnInput};
use agent::{CallRecord, CallStatus, ClientConfig, ProviderKind, ResultRecord, Sanitizer};
use serde_json::json;

use super::*;

const SECRET_KEY: &str = "sk-very-secret-key-1234";
const SECRET_HOST: &str = "secret-endpoint.example";

struct Tools(Vec<ToolInfo>);

#[async_trait]
impl agent::turn::ToolHost for Tools {
    fn tools(&self) -> Vec<ToolInfo> {
        self.0.clone()
    }
    fn validate(&self, _: &str, _: &Value) -> Result<(), Vec<String>> {
        Ok(())
    }
    fn llama_schema(&self, _: &str) -> Option<Value> {
        None
    }
    async fn describe(&self, _: &str, _: &Map<String, Value>) -> Described {
        Described::Invalid("none".into())
    }
    async fn execute(
        &self,
        _: &str,
        _: &Map<String, Value>,
        _: &policy::Action,
        _: CancellationToken,
    ) -> ToolOutput {
        ToolOutput::error("none")
    }
}

fn tool(name: &str) -> ToolInfo {
    ToolInfo {
        name: name.into(),
        description: format!("The {name} tool."),
        input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        read_only_hint: None,
        destructive_hint: None,
    }
}

/// A client whose address and key must never reach the view. It is never asked anything
/// (a generic endpoint: no probe, no token count).
fn client() -> OpenAiClient {
    let mut config = ClientConfig::new(format!("https://{SECRET_HOST}/v1"), ProviderKind::Generic);
    config.api_key = Some(SECRET_KEY.into());
    OpenAiClient::new(config).expect("client")
}

fn chat_input(n_ctx: u32) -> TurnInput {
    let caps = ModelCaps {
        tools: true,
        n_ctx,
        ..ModelCaps::default()
    };
    let mut first = HistoryMessage::user("Read notes.txt");
    first.parts.push(Part::Attachment {
        name: "notes.txt".into(),
        text: "the secret word is heron".into(),
    });
    first.id = Some(1);
    let mut call = HistoryMessage::assistant_calls(
        "",
        vec![CallRecord {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: json!({"path": "notes.txt"}).as_object().cloned().unwrap(),
            recovered: false,
        }],
    );
    call.id = Some(2);
    let mut result = HistoryMessage::tool_result(ResultRecord {
        call_id: "c1".into(),
        name: "read_file".into(),
        status: CallStatus::Ok,
        text: "<b>file text</b> that is untrusted".into(),
        images: vec![],
        source: Some("notes.txt".into()),
        notes: vec![],
    });
    result.id = Some(3);
    let mut answer = HistoryMessage::assistant("The word is heron.");
    answer.id = Some(4);
    let mut input = TurnInput::new(
        vec![
            first,
            call,
            result,
            answer,
            HistoryMessage::user(NEXT_MESSAGE),
        ],
        caps,
        RequestTemplate {
            model: "fake-model".into(),
            ..RequestTemplate::default()
        },
    );
    input.system_prompt = "You are a careful assistant.".into();
    input.pinned = vec!["Answer in German.".into()];
    input.app_notes = vec!["Task: tidy the notes. Next: rename the file.".into()];
    input
}

async fn preview(input: TurnInput) -> TurnPreview {
    let tools = Tools(vec![tool("read_file"), tool("list_dir")]);
    agent::preview_turn(&client(), &tools, &Sanitizer::generic(), input).await
}

fn inspected<'a>(p: &'a TurnPreview, model: Result<String, String>) -> Inspected<'a> {
    Inspected {
        preview: p,
        hidden: 3,
        task_note: Some("Task: tidy the notes. Next: rename the file."),
        task_status: Some("The task state is sent with the next message."),
        placeholder: true,
        model,
    }
}

#[tokio::test]
async fn the_view_has_every_section_in_order_with_the_exact_text() {
    let p = preview(chat_input(32_768)).await;
    let v = view(&inspected(&p, Ok(model_line(false, 32_768))));
    let titles: Vec<&str> = v.sections.iter().map(|s| s.title).collect();
    assert_eq!(
        titles,
        [
            "System prompt",
            "Pinned instructions",
            "Tool instructions",
            "Skills list",
            "Loaded skills",
            "Tool definitions",
            "Summary of earlier messages",
            "Messages kept",
            "Messages left out",
            "Task note",
            "Attachments"
        ]
    );
    let section = |id: &str| v.sections.iter().find(|s| s.id == id).unwrap().clone();
    assert_eq!(section("system").text, "You are a careful assistant.");
    assert!(section("pinned").text.contains("1. Answer in German."));
    assert_eq!(section("pinned").detail, "1 pin");
    assert_eq!(section("skills").detail, "None");
    let tools = section("tools");
    assert!(
        tools
            .detail
            .starts_with("2 shown: read_file, list_dir. 3 more are on but not shown"),
        "{}",
        tools.detail
    );
    assert!(tools.text.contains("\"read_file\"") && tools.text.contains("The list_dir tool."));
    assert_eq!(tools.tokens, Some(p.tool_tokens));
    let kept = section("kept");
    assert!(
        kept.text.contains("[tool] read_file") || kept.text.contains("[tool]"),
        "{}",
        kept.text
    );
    assert!(
        kept.text.contains("<b>file text</b>"),
        "shown as text, as sent"
    );
    assert!(kept.text.contains(NEXT_MESSAGE) && kept.text.contains("Task: tidy the notes."));
    assert_eq!(
        section("task").detail,
        "The task state is sent with the next message."
    );
    assert_eq!(section("attachments").detail, "1 file: notes.txt");
    assert!(
        section("attachments")
            .text
            .contains("the secret word is heron")
    );
    // The system blocks are the system message, in order.
    let system_message = p.request.messages[0].content.as_ref().unwrap().text();
    for id in ["system", "pinned", "preamble"] {
        assert!(system_message.contains(&section(id).text), "{id}");
    }
    let totals = v.totals.expect("numbers");
    assert_eq!(totals.n_ctx, 32_768);
    assert_eq!(totals.tokens, p.prompt_tokens);
    assert_eq!(totals.pinned.used, p.pinned_tokens);
    assert!(totals.tools.is_some());
    assert!(v.refusal.is_none());
    assert!(
        v.next_drop.starts_with("Nothing is left out now."),
        "{}",
        v.next_drop
    );
    assert!(v.next_drop.contains("1 tool call"), "{}", v.next_drop);
    assert!(v.notes[0].starts_with(NEXT_MESSAGE));
}

/// The endpoint's address and key are not part of what the tab gets, whatever the chat holds.
#[tokio::test]
async fn no_secret_reaches_the_view() {
    let p = preview(chat_input(32_768)).await;
    for model in [Ok(model_line(true, 32_768)), Err("No model.".to_owned())] {
        let text = serde_json::to_string(&view(&inspected(&p, model))).unwrap();
        assert!(!text.contains(SECRET_KEY), "the key reached the view");
        assert!(!text.contains(SECRET_HOST), "the address reached the view");
        assert!(!text.to_lowercase().contains("authorization"));
    }
}

/// Without a usable model the tab says why and shows no numbers.
#[tokio::test]
async fn without_a_model_there_are_no_numbers() {
    let p = preview(chat_input(4_096)).await;
    let v = view(&inspected(
        &p,
        Err("The model for this chat is not loaded. Token numbers appear once it is.".into()),
    ));
    assert!(v.model.contains("not loaded"));
    assert!(v.totals.is_none() && v.refusal.is_none());
    assert!(v.sections.iter().all(|s| s.tokens.is_none()));
    assert_eq!(v.next_drop, "Shown once a model is ready for this chat.");
    // The texts are still there.
    assert!(v.sections[1].text.contains("Answer in German."));
}

/// What would go next comes from the loop's stages; a turn that would not start says why.
#[tokio::test]
async fn what_goes_next_and_refusals_come_from_the_loop() {
    // Small window, a long earlier tool result: stage 1 is already needed.
    let mut input = chat_input(4_096);
    if let Part::ToolResult(r) = &mut input.history[2].parts[0] {
        r.text = "x".repeat(12_000);
    }
    let p = preview(input).await;
    let v = view(&inspected(&p, Ok(model_line(true, 4_096))));
    assert!(
        v.next_drop
            .starts_with("Results of tool calls in earlier messages are already shortened"),
        "{}",
        v.next_drop
    );
    // Pins that do not fit: the loop's own message.
    let mut input = chat_input(4_096);
    input.pinned = vec!["p".repeat(1_000); 20];
    let p = preview(input).await;
    let v = view(&inspected(&p, Ok(model_line(true, 4_096))));
    let refusal = v.refusal.expect("refused");
    assert!(
        refusal.starts_with("Your 20 pinned instructions need about "),
        "{refusal}"
    );
}

#[test]
fn next_drop_says_each_stage_plainly() {
    let budget = agent::ContextBudget::new(8_192, 1_000);
    let base = TurnPreview {
        request: agent::ChatRequest::default(),
        system_blocks: Vec::new(),
        budget,
        prompt_tokens: 100,
        exact: false,
        refusal: None,
        tools_on: false,
        withheld_tools: Vec::new(),
        tool_tokens: 0,
        tools_allowance: 0,
        unpinned_overhead: 0,
        pinned_tokens: 0,
        pinned_allowance: 0,
        kept: Vec::new(),
        left_out: Vec::new(),
        applied: Vec::new(),
        over_trigger: false,
        over_budget: false,
        next: NextCompaction::Nothing,
    };
    assert_eq!(
        next_drop(&base),
        "Nothing would be left out: there is nothing earlier to shorten."
    );
    let summarise = TurnPreview {
        next: NextCompaction::SummariseEarlier { messages: 4 },
        over_trigger: true,
        ..base.clone()
    };
    assert_eq!(
        next_drop(&summarise),
        "Before the next request is sent, the oldest 4 messages would be replaced by a summary \
         the model writes."
    );
    let full = TurnPreview {
        over_trigger: true,
        over_budget: true,
        ..base.clone()
    };
    assert!(next_drop(&full).contains("no longer fits"));
    let turn = TurnPreview {
        next: NextCompaction::ShortenTurnResults,
        ..base
    };
    assert!(next_drop(&turn).contains("current turn"));
}

/// The commands are granted to the main window alone and check it again.
#[test]
fn the_command_is_for_the_main_window_only() {
    let caps = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("capabilities");
    let main = std::fs::read_to_string(caps.join("main.json")).unwrap();
    assert!(main.contains("\"allow-inspect-context\""));
    for other in ["overlay.json", "indicator.json"] {
        let text = std::fs::read_to_string(caps.join(other)).unwrap();
        assert!(!text.contains("inspect-context"), "{other}");
    }
    assert!(only_main_window(crate::MAIN_WINDOW).is_ok());
    assert!(only_main_window("overlay").is_err());
}

/// In the shared assembly, everything a real turn changes when it starts is done only for a
/// real turn; the preview reads the read-only twins. (The assembly needs the running app,
/// so this reads its source; the end-to-end scenario `context-inspector` checks the
/// database before and after.)
#[test]
fn the_preview_mode_of_the_assembly_changes_nothing() {
    // A checkout on Windows may end lines differently.
    let source = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("turns.rs"),
    )
    .unwrap()
    .replace("\r\n", "\n");
    let start = source
        .find("pub(crate) async fn assemble(")
        .expect("assemble");
    let end = start + source[start..].find("\n}\n").expect("end of assemble");
    let body = &source[start..end];
    // Each call that records something sits in an `if turn` branch.
    for call in [
        "crate::artifacts::note_history(",
        "crate::computer_cmd::prepare_turn(",
        "crate::tool_select::for_turn(",
        "skills_srv.set_context_tokens(",
        "skills_srv.set_terminal_enabled(",
        "crate::task::begin_turn(",
    ] {
        let at = body
            .find(call)
            .unwrap_or_else(|| panic!("{call} is not in the assembly"));
        let guard = body[..at].rfind("if turn {").expect("a guard");
        let between = &body[guard..at];
        let opened = between.matches('{').count();
        let closed = between.matches('}').count();
        assert!(opened > closed, "{call} is not inside `if turn`");
        assert!(
            !between.contains("} else {"),
            "{call} is in the else branch"
        );
    }
    for twin in [
        "crate::tool_select::preview_for_turn(",
        "crate::task::note_for_turn(",
        "state.store.current_path(",
        "crate::pinned::for_turn(app, chat_id, turn)",
    ] {
        assert!(body.contains(twin), "{twin}");
    }
    // `run` uses the same assembly.
    assert!(source.contains("assemble(app, &req, Assembly::Turn).await?"));
}
