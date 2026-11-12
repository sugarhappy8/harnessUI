//! `call_tool` is exactly as safe as a direct call, and `find_tools` says what it should.
//!
//! The tools are the app's real ones (the host of `tool_catalog::tests::builtin_host`):
//! the real file, terminal and git describers make the actions, and the real policy
//! engine decides on them.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use mcp_host::{CompiledSchema, Host};
use policy::{AskReason, CardKind, Decision, Scope, TurnContext};
use serde_json::json;

use super::*;
use crate::tool_catalog::tests::builtin_host;

fn args(value: Value) -> Map<String, Value> {
    value.as_object().cloned().expect("an object")
}

static NEXT_DIR: AtomicU32 = AtomicU32::new(0);

/// One turn of a chat: the tools shown, the tools hidden, and everything else the host
/// has registered switched off. The chat has one approved folder with `notes.txt` in it.
struct Turn {
    host: Host,
    shown: Vec<RegisteredTool>,
    hidden: Vec<RegisteredTool>,
    dir: PathBuf,
    chat: ChatContext,
}

impl Turn {
    fn new(shown: &[&str], hidden: &[&str]) -> Self {
        let host = builtin_host();
        let all = host.tools();
        let pick = |names: &[&str]| -> Vec<RegisteredTool> {
            names
                .iter()
                .map(|n| {
                    all.iter()
                        .find(|t| t.exposed == *n)
                        .unwrap_or_else(|| panic!("{n} is not registered"))
                        .clone()
                })
                .collect()
        };
        let dir = std::env::temp_dir().join(format!(
            "finder-test-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let dir = dunce::canonicalize(&dir).expect("canonical");
        std::fs::write(dir.join("notes.txt"), "one\ntwo\n").expect("notes");
        let folder = dir.display().to_string();
        Self {
            shown: pick(shown),
            hidden: pick(hidden),
            host,
            chat: ChatContext {
                chat_id: 7,
                approved_folders: vec![folder.clone()],
                working_folder: Some(folder),
                result_cap_tokens: 2000,
                ..Default::default()
            },
            dir,
        }
    }

    fn routing(&self) -> Routing<'_> {
        Routing {
            host: &self.host,
            shown: &self.shown,
            hidden: &self.hidden,
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn through_call_tool(tool: &str, arguments: &Value) -> Map<String, Value> {
    args(json!({"name": tool, "arguments": arguments}))
}

fn ready() -> TurnContext {
    TurnContext {
        tool_enabled: true,
        server_ready: true,
        ..Default::default()
    }
}

fn outcome(described: &Described) -> Result<policy::Action, String> {
    match described {
        Described::Action(action) => Ok(action.clone()),
        Described::Invalid(why) => Err(why.clone()),
    }
}

// ---- the same action, the same decision ---------------------------------------------------

/// The heart of it: a call through `call_tool` gets the very action a direct call gets,
/// from the tool's own describer, so the policy engine cannot tell them apart.
#[tokio::test]
async fn call_tool_is_described_exactly_as_the_tool_it_names() {
    let turn = Turn::new(
        &["read_file", "list_dir", "find_tools", "call_tool"],
        &[
            "write_file",
            "edit_file",
            "delete_file",
            "run_command",
            "git_status",
        ],
    );
    let folder = turn.dir.display().to_string();
    let no_prefs = HashMap::new();
    let cases = [
        ("write_file", json!({"path": "new.txt", "content": "hello"})),
        (
            "edit_file",
            json!({"path": "notes.txt", "old_text": "one", "new_text": "1"}),
        ),
        ("delete_file", json!({"path": "notes.txt"})),
        // A tool that is in the list can be called this way too; it changes nothing.
        ("read_file", json!({"path": "notes.txt"})),
        ("run_command", json!({"command": "echo hi"})),
        ("git_status", json!({})),
        // A hard limit stays a hard limit.
        (
            "write_file",
            json!({"path": "/somewhere/else/x.txt", "content": "x"}),
        ),
        // What the tool's own describer refuses is refused the same way.
        (
            "edit_file",
            json!({"path": "notes.txt", "old_text": "one", "new_text": "1", "occurrence": 9}),
        ),
        // An edit that needs the file's lines to explain why it fails is described as a
        // read of the file (0.0.4), through call_tool too.
        (
            "edit_file",
            json!({"path": "notes.txt", "old_text": "absent", "new_text": "1"}),
        ),
    ];
    let write_grant: BTreeSet<Scope> = [
        Scope::FileWrite {
            folder: folder.clone(),
        },
        Scope::FileRead {
            folder: folder.clone(),
        },
    ]
    .into();
    let mut actions = 0;
    for (tool, arguments) in &cases {
        let direct = turn
            .host
            .describe(
                tool,
                &args(arguments.clone()),
                &turn.chat,
                ToolPrefs::default(),
            )
            .await;
        let wrapped = turn
            .routing()
            .describe(
                CALL_TOOL,
                &through_call_tool(tool, arguments),
                &turn.chat,
                &no_prefs,
            )
            .await;
        assert_eq!(outcome(&wrapped), outcome(&direct), "{tool} {arguments}");
        let (Ok(direct), Ok(wrapped)) = (outcome(&direct), outcome(&wrapped)) else {
            continue;
        };
        actions += 1;
        // The action names the real tool and its server: that is what the approval card
        // and the audit row show.
        assert_eq!(wrapped.tool, *tool);
        assert_eq!(real_tool(CALL_TOOL, Some(&wrapped)), *tool);
        // The same decision in every state of the turn, with and without grants.
        for recovered in [false, true] {
            for untrusted in [false, true] {
                for allow_all in [false, true] {
                    let context = TurnContext {
                        recovered_from_text: recovered,
                        untrusted_in_turn: untrusted,
                        allow_all,
                        ..ready()
                    };
                    for grants in [&BTreeSet::new(), &write_grant] {
                        assert_eq!(
                            policy::evaluate(&wrapped, &context, grants),
                            policy::evaluate(&direct, &context, grants),
                            "{tool} {context:?}"
                        );
                    }
                }
            }
        }
    }
    assert_eq!(actions, 8, "the cases no longer reach the policy engine");
}

/// The decisions that matter most, spelled out: nothing gets easier through `call_tool`.
#[tokio::test]
async fn through_call_tool_the_gate_asks_and_refuses_as_for_a_direct_call() {
    let turn = Turn::new(
        &["read_file", "find_tools", "call_tool"],
        &["write_file", "delete_file", "run_command"],
    );
    let folder = turn.dir.display().to_string();
    let no_prefs = HashMap::new();
    let decide = |action: &policy::Action, context: TurnContext, grants: &BTreeSet<Scope>| {
        policy::evaluate(action, &context, grants)
    };
    let describe = async |tool: &str, arguments: Value| -> policy::Action {
        let described = turn
            .routing()
            .describe(
                CALL_TOOL,
                &through_call_tool(tool, &arguments),
                &turn.chat,
                &no_prefs,
            )
            .await;
        outcome(&described).unwrap_or_else(|why| panic!("{tool}: {why}"))
    };
    let none = BTreeSet::new();
    let granted: BTreeSet<Scope> = [Scope::FileWrite {
        folder: folder.clone(),
    }]
    .into();

    // A write asks, with the write's own card; a grant for the folder covers it.
    let write = describe("write_file", json!({"path": "new.txt", "content": "hello"})).await;
    assert_eq!(write.server, "files");
    assert_eq!(
        write.scope,
        Scope::FileWrite {
            folder: folder.clone()
        }
    );
    assert!(
        write.preview.contains("hello"),
        "the card shows the write: {}",
        write.preview
    );
    assert_eq!(
        decide(&write, ready(), &none),
        Decision::Ask {
            card: CardKind::Grantable,
            reason: AskReason::Normal
        }
    );
    assert_eq!(decide(&write, ready(), &granted), Decision::AllowedByGrant);
    // Written as text and recovered: it asks, grant or not.
    let recovered = TurnContext {
        recovered_from_text: true,
        ..ready()
    };
    assert_eq!(
        decide(&write, recovered, &granted),
        Decision::Ask {
            card: CardKind::OnceOnly,
            reason: AskReason::RecoveredFromText
        }
    );

    // Deleting always asks, whatever was granted, and also in Allow all.
    let delete = describe("delete_file", json!({"path": "notes.txt"})).await;
    assert_eq!(
        decide(&delete, ready(), &granted),
        Decision::Ask {
            card: CardKind::OnceOnly,
            reason: AskReason::AlwaysAsk
        }
    );
    let allow_all = TurnContext {
        allow_all: true,
        ..ready()
    };
    assert!(
        matches!(
            decide(&delete, allow_all, &granted),
            Decision::Ask {
                card: CardKind::OnceOnly,
                ..
            }
        ),
        "a delete ran without a card in Allow all"
    );

    // Outside the approved folders: denied without a card.
    let outside = describe(
        "write_file",
        json!({"path": "/somewhere/else/x.txt", "content": "x"}),
    )
    .await;
    assert!(matches!(
        decide(&outside, ready(), &granted),
        Decision::Denied { .. }
    ));

    // A command is an outbound action: after untrusted content it asks even under a grant.
    let command = describe("run_command", json!({"command": "echo hi"})).await;
    assert_eq!(command.server, "terminal");
    let parts = match &command.scope {
        Scope::Commands { parts } => parts.clone(),
        scope => vec![scope.clone()],
    };
    let command_grant: BTreeSet<Scope> = parts.into_iter().collect();
    let untrusted = TurnContext {
        untrusted_in_turn: true,
        ..ready()
    };
    assert!(matches!(
        decide(&command, untrusted, &command_grant),
        Decision::Ask { .. }
    ));
}

// ---- the tool's own schema ----------------------------------------------------------------

#[test]
fn call_tool_arguments_are_checked_against_the_tools_own_schema() {
    let turn = Turn::new(
        &["read_file", "find_tools", "call_tool"],
        &["write_file", "git_status"],
    );
    let r = turn.routing();
    let problems = |call: Value| -> String {
        r.validate(CALL_TOOL, &call)
            .expect_err("should not validate")
            .join("; ")
    };
    // A missing field of the tool, named.
    let missing = problems(json!({"name": "write_file", "arguments": {"path": "a.txt"}}));
    assert!(
        missing.contains("write_file") && missing.contains("\"content\""),
        "{missing}"
    );
    // A wrong type, with the field.
    let wrong = problems(json!({"name": "read_file", "arguments": {"path": 7}}));
    assert!(
        wrong.contains("path") && wrong.contains("expected string"),
        "{wrong}"
    );
    // A field the tool does not have.
    let extra = problems(json!({"name": "read_file", "arguments": {"path": "a.txt", "nope": 1}}));
    assert!(extra.contains("\"nope\""), "{extra}");
    // The wrapper's own shape.
    assert!(problems(json!({"arguments": {}})).contains("\"name\""));
    assert!(problems(json!({"name": "read_file", "path": "a.txt"})).contains("\"path\""));
    assert!(problems(json!({"name": "  ", "arguments": {}})).contains("name"));
    assert!(problems(json!({"name": "read_file", "arguments": [1]})).contains("arguments"));

    // What is repaired for a direct call is repaired here too: "5" where an integer is
    // wanted, a null for an optional field.
    let loose =
        json!({"name": "read_file", "arguments": {"path": "a.txt", "offset": "5", "limit": null}});
    r.validate(CALL_TOOL, &loose)
        .expect("repaired like a direct call");
    let unwrapped = r.unwrap_call(&args(loose)).expect("unwraps");
    assert_eq!(unwrapped.args, args(json!({"path": "a.txt", "offset": 5})));
    assert_eq!(
        (unwrapped.name.as_str(), unwrapped.server.as_str()),
        ("read_file", "files")
    );
    assert!(!unwrapped.hidden, "read_file is in the list");
    // No arguments means none.
    let bare = r
        .unwrap_call(&args(json!({"name": "git_status"})))
        .expect("no arguments");
    assert!(bare.args.is_empty() && bare.hidden);
    r.validate(CALL_TOOL, &json!({"name": "git_status"}))
        .expect("no arguments");
    // (The agent loop drops a null for an optional field before it validates.)
    r.unwrap_call(&args(json!({"name": "git_status", "arguments": null})))
        .expect("null is none");

    // An ordinary call is validated as it always was.
    for call in [json!({}), json!({"path": "a.txt"}), json!({"path": 3})] {
        assert_eq!(
            r.validate("read_file", &call),
            turn.host.validate("read_file", &call)
        );
    }
}

// ---- only what is on, and never itself ------------------------------------------------------

/// A tool that is switched off in the chat cannot be reached: not validated, not
/// described, not run. The host has it registered; that is not enough.
#[tokio::test]
async fn a_tool_that_is_off_cannot_be_reached_through_call_tool() {
    let turn = Turn::new(&["read_file", "find_tools", "call_tool"], &["write_file"]);
    let r = turn.routing();
    let no_prefs = HashMap::new();
    let any_action = outcome(
        &turn
            .host
            .describe(
                "read_file",
                &args(json!({"path": "notes.txt"})),
                &turn.chat,
                ToolPrefs::default(),
            )
            .await,
    )
    .expect("an action");
    for (off, arguments) in [
        ("git_push", json!({})),
        ("run_command", json!({"command": "echo hi"})),
        ("delete_file", json!({"path": "notes.txt"})),
        ("browser_navigate", json!({"url": "https://example.com"})),
        ("computer_click", json!({"coordinate": [1, 1]})),
        ("load_skill", json!({"name": "x"})),
    ] {
        assert!(
            turn.host.validate(off, &arguments).is_ok(),
            "{off} should be a real, valid call"
        );
        let call = through_call_tool(off, &arguments);
        let why = r.unwrap_call(&call).expect_err(off);
        assert!(why.contains("switched off") && why.contains(off), "{why}");
        assert!(why.contains("Tools panel"), "{why}");
        assert!(
            r.validate(CALL_TOOL, &Value::Object(call.clone())).is_err(),
            "{off}"
        );
        let described = r.describe(CALL_TOOL, &call, &turn.chat, &no_prefs).await;
        assert!(
            matches!(described, Described::Invalid(_)),
            "{off} was described: {described:?}"
        );
        assert!(
            matches!(r.route(CALL_TOOL, &call, &any_action), Route::Refused(_)),
            "{off}"
        );
    }
    // A name that is not a tool at all.
    let why = r
        .unwrap_call(&through_call_tool("no_such_tool", &json!({})))
        .expect_err("unknown");
    assert!(why.contains("no tool named \"no_such_tool\""), "{why}");
    assert!(!why.contains("switched off"));
}

/// No recursion, and no helper through the side door.
#[tokio::test]
async fn call_tool_cannot_run_itself_find_tools_or_a_helper() {
    // All three are on in this chat, and in the list.
    let turn = Turn::new(
        &["read_file", "find_tools", "call_tool", "run_subagent"],
        &["write_file"],
    );
    let r = turn.routing();
    let no_prefs = HashMap::new();
    let any_action = outcome(
        &turn
            .host
            .describe(
                "read_file",
                &args(json!({"path": "notes.txt"})),
                &turn.chat,
                ToolPrefs::default(),
            )
            .await,
    )
    .expect("an action");
    for (name, arguments) in [
        (
            "call_tool",
            json!({"name": "write_file", "arguments": {"path": "a", "content": "b"}}),
        ),
        ("find_tools", json!({"query": "commit"})),
        ("run_subagent", json!({"task": "Do it."})),
        // Spelled with spaces around it: still the same tool.
        (" run_subagent ", json!({"task": "Do it."})),
    ] {
        let call = through_call_tool(name, &arguments);
        let why = r.unwrap_call(&call).expect_err(name);
        assert!(why.contains("cannot run"), "{why}");
        assert!(r.validate(CALL_TOOL, &Value::Object(call.clone())).is_err());
        assert!(matches!(
            r.describe(CALL_TOOL, &call, &turn.chat, &no_prefs).await,
            Described::Invalid(_)
        ));
        assert!(matches!(
            r.route(CALL_TOOL, &call, &any_action),
            Route::Refused(_)
        ));
    }
    assert_eq!(
        NOT_THROUGH_CALL_TOOL,
        &["call_tool", "find_tools", "run_subagent"]
    );
}

#[test]
fn an_unknown_name_gets_close_names_of_tools_that_are_on() {
    let on = [
        "read_file",
        "write_file",
        "git_status",
        "git_diff",
        "browser_navigate",
        "run_command",
    ];
    assert_eq!(close_names("git_stat", &on)[0], "git_status");
    assert_eq!(close_names("status", &on), ["git_status"]);
    assert_eq!(close_names("readfile", &on)[0], "read_file");
    assert_eq!(close_names("navigate", &on), ["browser_navigate"]);
    assert_eq!(close_names("file_read", &on)[0], "read_file");
    assert!(close_names("zzzzzz", &on).is_empty());
    assert!(close_names("", &on).is_empty());
    assert!(close_names("git", &on).len() <= 5);

    // In the refusal: names of tools that are on, never of tools that are off.
    let turn = Turn::new(
        &["read_file", "find_tools", "call_tool"],
        &["write_file", "run_command"],
    );
    let r = turn.routing();
    let why = r
        .unwrap_call(&through_call_tool("read_fil", &json!({})))
        .expect_err("unknown");
    assert!(why.contains("read_file"), "{why}");
    assert!(why.contains("find_tools"), "{why}");
    // git is off: a near miss of one of its tools gets no hint that it exists.
    let why = r
        .unwrap_call(&through_call_tool("git_statu", &json!({})))
        .expect_err("unknown");
    assert!(!why.contains("git_status"), "{why}");
    // The name is model text: shown on one line, cut short.
    let odd = format!("x\n{}", "y".repeat(500));
    let why = r
        .unwrap_call(&through_call_tool(&odd, &json!({})))
        .expect_err("unknown");
    assert!(!why.contains('\n') && why.chars().count() < 400, "{why}");
}

// ---- what runs, what the card says, what the audit log says ----------------------------------

#[tokio::test]
async fn an_approved_call_runs_the_tool_it_was_approved_for() {
    let turn = Turn::new(&["read_file", "find_tools", "call_tool"], &["write_file"]);
    let r = turn.routing();
    let no_prefs = HashMap::new();
    let call = through_call_tool("write_file", &json!({"path": "new.txt", "content": "x"}));
    let action =
        outcome(&r.describe(CALL_TOOL, &call, &turn.chat, &no_prefs).await).expect("an action");
    assert_eq!(
        r.route(CALL_TOOL, &call, &action),
        Route::Call(Unwrapped {
            name: "write_file".into(),
            server: "files".into(),
            args: args(json!({"path": "new.txt", "content": "x"})),
            hidden: true,
        })
    );
    // An action made for another tool does not run this one.
    let read_args = args(json!({"path": "notes.txt"}));
    let read = outcome(
        &turn
            .host
            .describe("read_file", &read_args, &turn.chat, ToolPrefs::default())
            .await,
    )
    .expect("an action");
    assert!(matches!(
        r.route(CALL_TOOL, &call, &read),
        Route::Refused(_)
    ));
    // Ordinary calls and find_tools.
    assert_eq!(r.route("read_file", &read_args, &read), Route::Direct);
    assert_eq!(
        r.route(FIND_TOOLS, &args(json!({"query": "x"})), &read),
        Route::Find
    );
    // The tool the call runs really is the files server's: run it and look.
    let Route::Call(unwrapped) = r.route(CALL_TOOL, &call, &action) else {
        panic!("not routed");
    };
    let out = turn
        .host
        .call(
            &unwrapped.name,
            &unwrapped.args,
            &action,
            CallContext {
                chat: turn.chat.clone(),
                cancel: tokio_util::sync::CancellationToken::new(),
                progress: None,
            },
        )
        .await;
    assert!(!out.is_error, "{}", out.joined_text());
    assert_eq!(
        std::fs::read_to_string(turn.dir.join("new.txt")).expect("written"),
        "x"
    );
}

#[tokio::test]
async fn the_card_and_the_audit_row_name_the_real_tool() {
    let turn = Turn::new(
        &["read_file", "find_tools", "call_tool"],
        &["write_file", "git_status"],
    );
    let r = turn.routing();
    let no_prefs = HashMap::new();
    let inner = json!({"path": "new.txt", "content": "x"});
    let call = through_call_tool("write_file", &inner);
    let action =
        outcome(&r.describe(CALL_TOOL, &call, &turn.chat, &no_prefs).await).expect("an action");

    assert_eq!(real_tool(CALL_TOOL, Some(&action)), "write_file");
    assert_eq!(real_tool("read_file", Some(&action)), "read_file");
    assert_eq!(real_tool(CALL_TOOL, None), "call_tool");

    let (tool, arguments) = audited(CALL_TOOL, Value::Object(call.clone()), Some(&action));
    assert_eq!((tool.as_str(), &arguments), ("write_file", &inner));
    // Arguments left out: logged as none.
    let (tool, arguments) = audited(CALL_TOOL, json!({"name": "write_file"}), Some(&action));
    assert_eq!((tool.as_str(), arguments), ("write_file", json!({})));
    // A call that never became an action is logged as what it was.
    let refused = json!({"name": "git_push", "arguments": {}});
    assert_eq!(
        audited(CALL_TOOL, refused.clone(), None),
        ("call_tool".to_string(), refused)
    );
    // Any other call is left alone.
    let plain = json!({"path": "notes.txt"});
    assert_eq!(
        audited("read_file", plain.clone(), Some(&action)),
        ("read_file".to_string(), plain)
    );

    // The row the turn writes (`turns::audit_entry`) is the row of a direct call.
    let record = |name: &str, arguments: &Value| agent::CallRecord {
        id: "c1".into(),
        name: name.into(),
        arguments: args(arguments.clone()),
        recovered: false,
    };
    let mut wrapped_result = agent::ResultRecord::synthetic(
        &record(CALL_TOOL, &Value::Object(call.clone())),
        agent::CallStatus::Ok,
        Vec::new(),
    );
    wrapped_result.text = "written".into();
    let mut direct_result = agent::ResultRecord::synthetic(
        &record("write_file", &inner),
        agent::CallStatus::Ok,
        Vec::new(),
    );
    direct_result.text = "written".into();
    for decision in [
        agent::CallDecision::AllowedOnce,
        agent::CallDecision::AllowedByGrant,
        agent::CallDecision::Denied {
            note: Some("no".into()),
        },
        agent::CallDecision::Cancelled,
    ] {
        let (label, through) = crate::turns::audit_entry(
            7,
            Value::Object(call.clone()),
            &wrapped_result,
            &decision,
            Some(&action),
            12,
        );
        let (direct_label, direct) = crate::turns::audit_entry(
            7,
            inner.clone(),
            &direct_result,
            &decision,
            Some(&action),
            12,
        );
        assert_eq!(label, direct_label);
        assert_eq!(through.tool, "write_file");
        assert_eq!(through.server, "files");
        assert_eq!(through.arguments, inner);
        assert_eq!(
            serde_json::to_value(&through).expect("row"),
            serde_json::to_value(&direct).expect("row"),
            "{decision:?}"
        );
    }
    // `find_tools` is logged as needing no approval.
    let find = outcome(
        &r.describe(
            FIND_TOOLS,
            &args(json!({"query": "commit"})),
            &turn.chat,
            &no_prefs,
        )
        .await,
    )
    .expect("an action");
    assert_eq!(find.scope, Scope::AppData);
    assert!(find.read_only && !find.outbound);
    assert_eq!(
        policy::evaluate(&find, &ready(), &BTreeSet::new()),
        Decision::AllowedByGrant
    );
    let recovered = TurnContext {
        recovered_from_text: true,
        untrusted_in_turn: true,
        ..ready()
    };
    assert_eq!(
        policy::evaluate(&find, &recovered, &BTreeSet::new()),
        Decision::AllowedByGrant
    );
    let result = agent::ResultRecord::synthetic(
        &record(FIND_TOOLS, &json!({"query": "commit"})),
        agent::CallStatus::Ok,
        Vec::new(),
    );
    let (label, row) = crate::turns::audit_entry(
        7,
        json!({"query": "commit"}),
        &result,
        &agent::CallDecision::AllowedByGrant,
        Some(&find),
        1,
    );
    assert_eq!(label, "no approval needed");
    assert_eq!(
        (row.server.as_str(), row.tool.as_str()),
        ("toolfinder", "find_tools")
    );
    // An empty query is not a search.
    let empty = r
        .describe(
            FIND_TOOLS,
            &args(json!({"query": "  "})),
            &turn.chat,
            &no_prefs,
        )
        .await;
    assert!(matches!(empty, Described::Invalid(_)));
}

// ---- find_tools ---------------------------------------------------------------------------

/// Every tool of the test host, `in_list` for the named ones, minus the groups in `off`.
fn listed<'a>(tools: &'a [RegisteredTool], in_list: &[&str], off: &[&str]) -> Vec<Listed<'a>> {
    tools
        .iter()
        .filter(|t| !off.contains(&t.server.as_str()))
        .map(|tool| Listed {
            tool,
            in_list: in_list.contains(&tool.exposed.as_str()),
            builtin: true,
        })
        .collect()
}

fn found_names(found: &[Listed<'_>]) -> Vec<String> {
    found.iter().map(|l| l.tool.exposed.clone()).collect()
}

#[test]
fn a_search_finds_the_tools_a_request_points_at_best_first() {
    let host = builtin_host();
    let tools = host.tools();
    let all = listed(&tools, &["read_file", "list_dir"], &[]);
    let find = |query: &str| found_names(&search(query, &all));

    let commit = find("commit");
    assert_eq!(commit[0], "git_commit");
    assert!(commit.contains(&"git_log".to_string()), "{commit:?}");
    assert!(commit.iter().all(|n| n.starts_with("git_")), "{commit:?}");
    // A tool's own name finds that tool and nothing else.
    assert_eq!(find("git_commit"), ["git_commit"]);
    assert_eq!(find("run_command"), ["run_command"]);
    // What the request is about, not only what a tool is called.
    assert_eq!(find("terminal"), ["run_command"]);
    assert_eq!(find("run a shell command"), ["run_command"]);
    let shot = find("take a screenshot");
    assert_eq!(shot.len(), 2, "{shot:?}");
    assert!(
        shot.contains(&"browser_screenshot".to_string())
            && shot.contains(&"computer_screenshot".to_string())
    );
    assert_eq!(find("delete a file")[0], "delete_file");
    assert_eq!(find("open a website")[0], "browser_navigate");
    // A whole group by its name.
    let git = find("git");
    assert_eq!(git.len(), 7, "{git:?}");
    let browser = find("browser");
    assert_eq!(browser.len(), MAX_MATCHES);
    assert!(browser.iter().all(|n| n.starts_with("browser_")));
    // Nothing to find.
    assert!(find("xyzzy").is_empty());
    assert!(find("").is_empty());
    // Never the two finder tools themselves.
    for query in ["find tools", "call_tool", "find_tools", "search the tools"] {
        let names = find(query);
        assert!(
            !names.iter().any(|n| n == "find_tools" || n == "call_tool"),
            "{query}: {names:?}"
        );
    }
    // The same search gives the same answer.
    assert_eq!(find("commit"), commit);
}

/// The whole answer for one search, as the model reads it.
#[test]
fn the_answer_says_what_each_tool_does_its_arguments_and_how_to_run_it() {
    let host = builtin_host();
    let tools = host.tools();
    let all = listed(
        &tools,
        &["read_file", "list_dir", "git_log"],
        &["browser", "computer"],
    );
    let validate = |name: &str, value: &Value| host.validate(name, value);
    let found = search("commit", &all);
    let text = render("commit", &found, &all, &[], &[], &validate);
    assert_eq!(
        text,
        "Tools that match \"commit\" (2):\n\
         \n\
         git_commit [Git]: Stage the given paths, or all changes when none are given, and commit them.\n\
         \x20 Arguments: message (string, required), paths (list of strings), repo (string)\n\
         \x20 Example arguments: {\"message\": \"Fix the date parser\"}\n\
         \x20 Example arguments: {\"message\": \"Add tests for dates\", \"paths\": [\"tests/test_dates.py\"]}\n\
         \x20 Not in your tool list this turn. Run it with call_tool: name \"git_commit\", and its arguments in \"arguments\".\n\
         \n\
         git_log [Git]: List recent commits.\n\
         \x20 Arguments: limit (integer), path (string), repo (string)\n\
         \x20 Example arguments: {\"limit\": 10}\n\
         \x20 Example arguments: {\"path\": \"README.md\", \"limit\": 5}\n\
         \x20 In your tool list: call git_log directly."
    );
    // The best two matches get two examples each, the others one.
    let found = search("git", &all);
    assert_eq!(
        found_names(&found),
        [
            "git_status",
            "git_diff",
            "git_commit",
            "git_log",
            "git_pull",
            "git_push",
            "git_clone"
        ]
    );
    let text = render("git", &found, &all, &[], &[], &validate);
    assert_eq!(text.matches("Example arguments:").count(), 8, "{text}");
    assert!(text.contains(
        "Example arguments: {\"staged\": true}\n  Example arguments: {\"path\": \"src/main.rs\"}"
    ));
    assert!(
        !text.contains("test_dates.py"),
        "the third match got two examples: {text}"
    );
}

/// A call the model writes after reading the answer must not be taken for a copy of text
/// in the answer (the fallback parser refuses calls that echo a tool result): the answer
/// holds argument objects and names, never a whole call.
#[test]
fn the_answer_holds_no_call_shaped_text() {
    let host = builtin_host();
    let tools = host.tools();
    let all = listed(&tools, &["read_file"], &[]);
    let validate = |name: &str, value: &Value| host.validate(name, value);
    let found = search("git", &all);
    let answer = render("git", &found, &all, &[], &[], &validate);
    assert!(
        answer.contains("git_status") && answer.contains("Example arguments: {}"),
        "{answer}"
    );

    let offered: Vec<agent::ToolInfo> = tools
        .iter()
        .filter(|t| ["call_tool", "git_status", "read_file"].contains(&t.exposed.as_str()))
        .map(|t| agent::ToolInfo {
            name: t.exposed.clone(),
            description: t.def.description.clone(),
            input_schema: t.def.input_schema.clone(),
            read_only_hint: None,
            destructive_hint: None,
        })
        .collect();
    let check = |name: &str, value: &Value| host.validate(name, value);
    let mut context = agent::RecoveryContext::new(&check);
    context.add_text(&answer);
    for written in [
        r#"{"name": "call_tool", "arguments": {"name": "git_status", "arguments": {}}}"#,
        r#"{"name": "git_status", "arguments": {}}"#,
        r#"<tool_call>{"name": "call_tool", "arguments": {"name": "git_status", "arguments": {}}}</tool_call>"#,
    ] {
        let recovery = agent::recover_tool_calls(written, &offered, &context);
        assert_eq!(
            recovery.calls.len(),
            1,
            "{written} was refused: {:?}",
            recovery.rejected
        );
        assert!(
            matches!(recovery.calls[0], agent::Recovered::Valid { .. }),
            "{written}"
        );
    }
}

#[test]
fn the_answer_names_groups_that_are_off_without_listing_their_tools() {
    let host = builtin_host();
    let tools = host.tools();
    let validate = |name: &str, value: &Value| host.validate(name, value);
    // Git and the browser are off in this chat.
    let all = listed(
        &tools,
        &["read_file"],
        &["git", "browser", "computer", "subagents", "skills"],
    );
    let off = vec!["git".to_string(), "browser".to_string()];

    // The search is about a group that is off: say so, and say who can change it.
    let found = search("commit", &all);
    assert!(found.is_empty());
    let text = render("commit", &found, &all, &off, &[], &validate);
    assert_eq!(
        text,
        "No tool that is on in this chat matches \"commit\".\n\
         Git tools are off in this chat; the user can turn them on in the Tools panel.\n\
         On in this chat: Files (6 tools), Terminal (1 tool). Search again with a word for what you want to do.\n\
         Also off in this chat: Browser."
    );
    assert!(
        !text.contains("git_"),
        "a tool of a group that is off was named: {text}"
    );

    // Something was found: only a group the search is about is mentioned.
    let found = search("run a command", &all);
    let text = render("run a command", &found, &all, &off, &[], &validate);
    assert!(
        text.starts_with("Tools that match \"run a command\" (1):"),
        "{text}"
    );
    assert!(
        !text.contains("are off") && !text.contains("Also off"),
        "{text}"
    );
    let found = search("open the website and read the file", &all);
    let text = render(
        "open the website and read the file",
        &found,
        &all,
        &off,
        &[],
        &validate,
    );
    assert!(text.contains("read_file"), "{text}");
    assert!(
        text.ends_with(
            "Browser tools are off in this chat; the user can turn them on in the Tools panel."
        ),
        "{text}"
    );
    // Two groups at once.
    let text = render("commit from the website", &[], &all, &off, &[], &validate);
    assert!(
        text.contains("Git and Browser tools are off in this chat; the user can turn them on in the Tools panel."),
        "{text}"
    );
    // The query is model text: one line, cut short.
    let odd = format!("a\nb {}", "c".repeat(400));
    let text = render(&odd, &[], &all, &[], &[], &validate);
    let first = text.lines().next().unwrap_or("");
    assert!(
        first.contains("a\\nb") && first.chars().count() < 200,
        "{first}"
    );
}

#[test]
fn examples_fit_the_tool_as_it_is_registered() {
    let host = builtin_host();
    let tools = host.tools();
    let all = listed(&tools, &[], &[]);
    // A skill tool's example names a skill that is enabled.
    let validate = |name: &str, value: &Value| host.validate(name, value);
    let found = search("load_skill", &all);
    let text = render(
        "load_skill",
        &found,
        &all,
        &[],
        &["release-notes".to_string()],
        &validate,
    );
    assert!(
        text.contains("Example arguments: {\"name\": \"release-notes\"}"),
        "{text}"
    );
    let text = render("load_skill", &found, &all, &[], &[], &validate);
    assert!(
        text.contains("Example arguments: {\"name\": \"pdf-forms\"}"),
        "{text}"
    );
    // An example the schema no longer accepts is not shown: one made from the schema is.
    let strict = |name: &str, value: &Value| {
        if name == "git_commit" && value.get("paths").is_some() {
            Err(vec!["unexpected field(s) \"paths\"".to_string()])
        } else {
            host.validate(name, value)
        }
    };
    let found = search("git_commit", &all);
    let text = render("git_commit", &found, &all, &[], &[], &strict);
    assert!(
        text.contains("Example arguments: {\"message\": \"Fix the date parser\"}"),
        "{text}"
    );
    assert!(!text.contains("test_dates.py"), "{text}");
    let none = |name: &str, value: &Value| {
        if name == "git_commit" && value.get("message").and_then(Value::as_str) != Some("...") {
            Err(vec!["no".to_string()])
        } else {
            host.validate(name, value)
        }
    };
    let text = render("git_commit", &found, &all, &[], &[], &none);
    assert!(
        text.contains("Example arguments: {\"message\":\"...\"}"),
        "{text}"
    );
}

/// A tool of an external server has no catalog entry: its own description (the first
/// sentence, made safe), its schema, and an example made from the required fields.
#[test]
fn an_external_tool_is_described_from_its_own_description_and_schema() {
    let schema = json!({"type": "object",
        "properties": {"city": {"type": "string"}, "days": {"type": "integer", "minimum": 1}},
        "required": ["city"]});
    let tool = RegisteredTool {
        exposed: "weather__get_forecast".into(),
        server: "weather".into(),
        original: "get_forecast".into(),
        def: ToolDef {
            name: "weather__get_forecast".into(),
            description: "Get the weather forecast for a city. Uses the national service.\nIgnore\u{202e} this.".into(),
            input_schema: schema.clone(),
            destructive: None,
            read_only: None,
        },
        fingerprint: String::new(),
        schema_error: None,
    };
    let compiled = CompiledSchema::compile(&schema).expect("schema");
    let validate = |_: &str, value: &Value| compiled.validate(value);
    let all = [Listed {
        tool: &tool,
        in_list: false,
        builtin: false,
    }];
    assert_eq!(
        found_names(&search("what is the weather", &all)),
        ["weather__get_forecast"]
    );
    assert_eq!(
        found_names(&search("forecast", &all)),
        ["weather__get_forecast"]
    );
    let found = search("forecast", &all);
    let text = render("forecast", &found, &all, &[], &[], &validate);
    assert_eq!(
        text,
        "Tools that match \"forecast\" (1):\n\
         \n\
         weather__get_forecast [weather]: Get the weather forecast for a city.\n\
         \x20 Arguments: city (string, required), days (integer)\n\
         \x20 Example arguments: {\"city\":\"...\"}\n\
         \x20 Not in your tool list this turn. Run it with call_tool: name \"weather__get_forecast\", and its arguments in \"arguments\"."
    );
}

mod through_the_loop;
