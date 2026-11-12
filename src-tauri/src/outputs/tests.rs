use super::*;

use store::NewChat;
use tools::shorten::View;

async fn chat(store: &Store) -> i64 {
    store
        .create_chat(NewChat {
            title: "t".into(),
            ..NewChat::default()
        })
        .await
        .expect("create chat")
        .0
}

fn args(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("an object")
}

fn numbered_text(count: usize) -> String {
    (1..=count).map(|i| format!("line {i}\n")).collect()
}

#[test]
fn the_shortened_line_is_exactly_as_agreed_and_names_its_id() {
    let line = shortened_line(42, 10, 100, (11, 30));
    assert_eq!(
        line,
        "[Shortened: 10 of 100 lines shown. Full output is saved as output #42. Read more with read_output {\"id\": 42, \"lines\": \"11-30\"} or {\"id\": 42, \"grep\": \"text\"}.]"
    );
    assert_eq!(shortened_id(&format!("some text\n{line}")), Some(42));
    assert_eq!(shortened_id(&format!("some text\n{line}\n")), Some(42));
    // Only on the last line, and only in this form.
    assert_eq!(shortened_id(&format!("{line}\nmore")), None);
    assert_eq!(shortened_id("[Shortened: output #3]"), None);
    assert_eq!(shortened_id(""), None);
}

/// Line numbers in a tool's view and in a stored output are the same numbers.
#[test]
fn the_tools_and_the_store_count_lines_the_same_way() {
    let long = "x".repeat(4500);
    let wide = "é".repeat(2001);
    for text in [
        "",
        "one",
        "one\ntwo\n",
        "one\r\ntwo\r\n",
        "\n\n\n",
        &format!("a\n{long}\nb"),
        &format!("{wide}\n"),
    ] {
        let from_tools = tools::shorten::lines(text);
        let from_store: Vec<&str> = output_lines(text).iter().map(|l| l.text).collect();
        assert_eq!(from_tools, from_store, "{text:?}");
    }
}

#[tokio::test]
async fn a_shortened_result_is_stored_and_the_model_is_told_how_to_read_more() {
    let store = Store::open_in_memory().expect("open");
    let c = chat(&store).await;
    let full = numbered_text(100);
    let view: String = numbered_text(10) + "[Showing lines 1–10 of 100.]";
    let mut out = tools::shorten::shortened(
        View {
            text: view.clone(),
            shown: Shown {
                lines: 10,
                next: Some((11, 30)),
            },
        },
        full.clone(),
        false,
        Some(json!({"command": "x"})),
    );
    record(&store, c, "call_7", "run_command", &mut out).await;
    let list = store.list_tool_outputs(ChatId(c)).await.expect("list");
    assert_eq!(list.len(), 1);
    let id = list[0].id;
    assert_eq!(
        out.joined_text(),
        format!("{view}\n{}", shortened_line(id, 10, 100, (11, 30)))
    );
    assert!(out.full_text.is_none(), "the card shows what the model saw");
    assert_eq!(
        out.ui,
        Some(json!({"command": "x"})),
        "the note is taken out"
    );
    assert!(!out.is_error);
    let stored = store
        .get_tool_output(ChatId(c), id)
        .await
        .expect("get")
        .expect("stored");
    assert_eq!(stored.content, full);
    assert_eq!(
        (stored.info.call_id.as_str(), stored.info.tool.as_str()),
        ("call_7", "run_command")
    );
}

#[tokio::test]
async fn a_tool_without_a_note_is_counted_by_the_lines_it_showed() {
    let store = Store::open_in_memory().expect("open");
    let c = chat(&store).await;
    let mut out = ToolOutput {
        full_text: Some(numbered_text(50)),
        ..ToolOutput::text(numbered_text(7) + "[Showing the first 7 of 50 lines of this version.]")
    };
    record(&store, c, "", "read_artifact", &mut out).await;
    let id = store.list_tool_outputs(ChatId(c)).await.expect("list")[0].id;
    assert!(
        out.joined_text().ends_with(&format!(
            "version.]\n{}",
            shortened_line(id, 7, 50, (8, 27))
        )),
        "{}",
        out.joined_text()
    );
}

#[tokio::test]
async fn results_that_were_not_shortened_and_read_output_itself_are_left_alone() {
    let store = Store::open_in_memory().expect("open");
    let c = chat(&store).await;
    let mut plain = ToolOutput::text("all of it");
    record(&store, c, "", "read_file", &mut plain).await;
    assert_eq!(plain.joined_text(), "all of it");
    let mut again = ToolOutput {
        full_text: Some("x".into()),
        ui: Shown {
            lines: 1,
            next: None,
        }
        .attach(None),
        ..ToolOutput::text("x")
    };
    record(&store, c, "", TOOL_NAME, &mut again).await;
    assert_eq!(again.joined_text(), "x");
    assert!(again.ui.is_none(), "the note never leaves the app");
    assert!(
        store
            .list_tool_outputs(ChatId(c))
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn an_unknown_chat_keeps_the_whole_text_for_the_card() {
    let store = Store::open_in_memory().expect("open");
    let mut out = ToolOutput {
        full_text: Some("whole".into()),
        ..ToolOutput::text("part")
    };
    record(&store, 999, "", "run_command", &mut out).await;
    assert_eq!(out.joined_text(), "part");
    assert_eq!(out.full_text.as_deref(), Some("whole"));
}

async fn stored(store: &Store, chat: i64, content: &str) -> i64 {
    store
        .add_tool_output(NewToolOutput {
            chat_id: ChatId(chat),
            call_id: String::new(),
            tool: "run_command".into(),
            content: content.into(),
        })
        .await
        .expect("add")
        .id
}

/// Only the chat's own outputs can be read, by the model and by the card.
#[tokio::test]
async fn another_chats_output_cannot_be_read() {
    let store = Store::open_in_memory().expect("open");
    let mine = chat(&store).await;
    let other = chat(&store).await;
    let id = stored(&store, mine, "secret of my chat\n").await;
    let out = read(&store, ChatId(other), &args(json!({"id": id})), 6000).await;
    assert!(out.is_error);
    assert_eq!(
        out.joined_text(),
        format!(
            "There is no output #{id} in this chat: this chat has no saved outputs. Outputs are saved when a result is shortened; its last line names the id."
        )
    );
    assert!(!out.joined_text().contains("secret"));
    let own = stored(&store, other, "theirs\n").await;
    let out = read(
        &store,
        ChatId(other),
        &args(json!({"id": id, "grep": "secret"})),
        6000,
    )
    .await;
    assert_eq!(
        out.joined_text(),
        format!(
            "There is no output #{id} in this chat. Saved outputs: #{own} (newest first). The last line of a shortened result names its id."
        )
    );
    let out = read(&store, ChatId(mine), &args(json!({"id": id})), 6000).await;
    assert!(!out.is_error && out.joined_text().contains("secret of my chat"));
}

#[tokio::test]
async fn a_range_of_lines_is_read_with_its_numbers_and_the_next_range() {
    let store = Store::open_in_memory().expect("open");
    let c = chat(&store).await;
    let id = stored(&store, c, &numbered_text(30)).await;
    let read = |v: Value| {
        let store = store.clone();
        async move { read(&store, ChatId(c), &args(v), 6000).await }
    };
    let out = read(json!({"id": id, "lines": "3-5"})).await;
    assert!(!out.is_error);
    assert_eq!(
        out.joined_text(),
        format!(
            "     3: line 3\n     4: line 4\n     5: line 5\n[Showing lines 3-5 of 30. Next: read_output {{\"id\": {id}, \"lines\": \"6-25\"}}.]"
        )
    );
    let out = read(json!({"id": id, "lines": "28-40"})).await;
    assert_eq!(
        out.joined_text(),
        format!(
            "    28: line 28\n    29: line 29\n    30: line 30\n[Showing lines 28-30: output #{id} ends at line 30.]"
        )
    );
    let out = read(json!({"id": id})).await;
    assert!(out.joined_text().ends_with(&format!(
        "[Showing lines 1-30 of 30: the end of output #{id}.]"
    )));
    // A range larger than one result: what fits, and where to go on.
    let out = super::read(&store, ChatId(c), &args(json!({"id": id})), 500).await;
    let text = out.joined_text();
    assert!(
        text.contains("     1: line 1\n") && text.len() <= 500,
        "{text}"
    );
    assert!(
        text.contains(" (as many as fit in one result). Next: read_output"),
        "{text}"
    );
    // Errors.
    let out = read(json!({"id": id, "lines": "31-35"})).await;
    assert!(out.is_error);
    assert_eq!(
        out.joined_text(),
        format!("Output #{id} has 30 lines, so there is no line 31. Ask for lines from 1 to 30.")
    );
}

#[tokio::test]
async fn grep_returns_the_matching_lines_with_context_and_numbers() {
    let store = Store::open_in_memory().expect("open");
    let c = chat(&store).await;
    let text = "compiling a\ncompiling b\nerror: first\nnote\ncompiling c\ncompiling d\ncompiling e\ncompiling f\nERROR: second\ndone\n";
    let id = stored(&store, c, text).await;
    let out = read(
        &store,
        ChatId(c),
        &args(json!({"id": id, "grep": "error", "context": 1})),
        6000,
    )
    .await;
    assert!(!out.is_error);
    assert_eq!(
        out.joined_text(),
        format!(
            "2 lines of output #{id} contain \"error\" (10 lines in all; matching lines are marked with \":\", the lines around them with \"-\"):\n     2- compiling b\n     3: error: first\n     4- note\n--\n     8- compiling f\n     9: ERROR: second\n    10- done\n"
        )
    );
    // Default context is 2; overlapping windows are printed once.
    let out = read(
        &store,
        ChatId(c),
        &args(json!({"id": id, "grep": "compiling d"})),
        6000,
    )
    .await;
    assert!(out.joined_text().ends_with("     4- note\n     5- compiling c\n     6: compiling d\n     7- compiling e\n     8- compiling f\n"), "{}", out.joined_text());
    // No match: says how long the output is, as a plain result.
    let out = read(
        &store,
        ChatId(c),
        &args(json!({"id": id, "grep": "panic"})),
        6000,
    )
    .await;
    assert!(!out.is_error);
    assert_eq!(
        out.joined_text(),
        format!(
            "No line of output #{id} contains \"panic\" (letter case ignored). It has 10 lines; read_output {{\"id\": {id}, \"lines\": \"1-10\"}} shows the first ones."
        )
    );
}

#[test]
fn read_output_is_the_apps_own_data_and_never_asks() {
    let Described::Action(a) = describe(TOOL_NAME, &args(json!({"id": 3, "grep": "error"}))) else {
        panic!("not described")
    };
    assert_eq!(a.scope, policy::Scope::AppData);
    assert_eq!(a.risk, policy::Risk::Normal);
    assert!(
        a.read_only
            && !a.outbound
            && !a.network
            && a.keep_asking.is_none()
            && a.hard_limit.is_none()
    );
    assert_eq!(a.summary, "Find \"error\" in saved output #3");
    // Not even after untrusted content entered the turn.
    let turn = policy::TurnContext {
        tool_enabled: true,
        server_ready: true,
        untrusted_in_turn: true,
        ..policy::TurnContext::default()
    };
    let no_grants = std::collections::BTreeSet::<policy::Scope>::new();
    let decision = policy::evaluate(&a, &turn, &no_grants);
    assert!(
        !matches!(
            decision,
            policy::Decision::Ask { .. } | policy::Decision::Denied { .. }
        ),
        "{decision:?}"
    );
}

#[test]
fn bad_arguments_say_what_to_send() {
    let why = |v: Value| match describe(TOOL_NAME, &args(v)) {
        Described::Invalid(why) => why,
        Described::Action(a) => panic!("{a:?}"),
    };
    assert_eq!(
        why(json!({})),
        "id is required: the number after \"output #\" in a [Shortened: …] line."
    );
    assert_eq!(
        why(json!({"id": 1, "lines": "1-5", "grep": "x"})),
        "Pass lines or grep, not both: lines reads a range, grep finds the lines that contain some text."
    );
    assert_eq!(
        why(json!({"id": 1, "lines": "ten"})),
        "lines \"ten\" is not a range: write it as \"A-B\" with line numbers from 1, for example \"1-80\"."
    );
    assert_eq!(
        why(json!({"id": 1, "lines": "9-3"})),
        "lines \"9-3\" is backwards: the first line comes first, as in \"3-9\"."
    );
    assert_eq!(
        why(json!({"id": 1, "lines": "0-3"})),
        "lines start at 1, not 0: for example \"1-3\"."
    );
    assert_eq!(
        why(json!({"id": 1, "grep": " "})),
        "grep is empty: pass the text to look for."
    );
}

/// Small models get a tight allowance for tool definitions.
#[test]
fn the_definition_is_short() {
    let def = tool_def();
    let size = def.description.len() + def.input_schema.to_string().len();
    assert!(size <= 330, "{size}");
}

/// A long line shown only in part does not count as shown, and the range to read next
/// starts at it.
#[tokio::test]
async fn a_line_shown_in_part_is_read_next() {
    let store = Store::open_in_memory().expect("open");
    let c = chat(&store).await;
    let full = "é".repeat(9000);
    let view = tools::shorten::head_and_tail(&full, 1500, 50).expect("shortened");
    let mut out = tools::shorten::shortened(view, full, false, None);
    record(&store, c, "call_1", "read_artifact", &mut out).await;
    let id = store.list_tool_outputs(ChatId(c)).await.expect("list")[0].id;
    let text = out.joined_text();
    assert!(
        text.ends_with(&format!(
            "\n[Shortened: 0 of 5 lines shown. Full output is saved as output #{id}. Read more with read_output {{\"id\": {id}, \"lines\": \"1-5\"}} or {{\"id\": {id}, \"grep\": \"text\"}}.]"
        )),
        "{}",
        &text[text.len() - 220..]
    );
}

/// Which saved output a call has is set by the app alone: a tool's own value is dropped.
#[tokio::test]
async fn only_the_app_says_which_output_a_call_saved() {
    let store = Store::open_in_memory().expect("open");
    let c = chat(&store).await;
    // Not shortened, but claims a saved output: the claim is dropped.
    let mut forged = ToolOutput {
        ui: Some(json!({ SAVED_KEY: 5, "command": "x" })),
        ..ToolOutput::text("plain")
    };
    record(&store, c, "call_1", "read_file", &mut forged).await;
    assert_eq!(forged.ui, Some(json!({"command": "x"})));
    let mut forged = ToolOutput {
        ui: Some(json!({ SAVED_KEY: 5 })),
        ..ToolOutput::text("plain")
    };
    record(&store, c, "call_2", "some_mcp_tool", &mut forged).await;
    assert_eq!(forged.ui, None);
    // Shortened: the app's own id replaces whatever the tool put there.
    let mut out = ToolOutput {
        full_text: Some(numbered_text(50)),
        ui: Some(json!({ SAVED_KEY: 5 })),
        ..ToolOutput::text(numbered_text(3))
    };
    record(&store, c, "call_3", "read_file", &mut out).await;
    let id = store.list_tool_outputs(ChatId(c)).await.expect("list")[0].id;
    assert_ne!(id, 5);
    assert_eq!(out.ui, Some(json!({ SAVED_KEY: id })));
    // The card finds it by the call, in its own chat only.
    assert_eq!(saved_for_call(&store, ChatId(c), "call_3").await, Some(id));
    assert_eq!(saved_for_call(&store, ChatId(c), "call_1").await, None);
    assert_eq!(saved_for_call(&store, ChatId(c), "").await, None);
    let other = chat(&store).await;
    assert_eq!(saved_for_call(&store, ChatId(other), "call_3").await, None);
}
