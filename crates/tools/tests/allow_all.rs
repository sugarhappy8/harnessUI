//! Allow all (0.0.4): what the file, terminal and git describers tell the policy engine
//! about a call, and what the engine then decides in each mode.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use mcp_host::{BuiltinServer, ChatContext, Described};
use policy::{Action, AskReason, CardKind, Decision, KeepAsking, Risk, Scope, TurnContext};
use serde_json::{Value, json};
use tools::{FilesServer, GitConfig, GitServer, TerminalConfig, TerminalServer, TerminalShell};

const STILL_ASKS: Decision = Decision::Ask {
    card: CardKind::OnceOnly,
    reason: AskReason::StillAsksInAllowAll,
};
const ASKS_AFTER_UNTRUSTED: Decision = Decision::Ask {
    card: CardKind::Grantable,
    reason: AskReason::UntrustedContentThenOutbound,
};

fn chat(dir: &Path) -> ChatContext {
    ChatContext {
        chat_id: 1,
        approved_folders: vec![dir.display().to_string()],
        result_cap_tokens: 2000,
        full_access: false,
        ..Default::default()
    }
}

async fn describe(
    server: &dyn BuiltinServer,
    tool: &str,
    args: Value,
    chat: &ChatContext,
) -> Action {
    match server
        .describe(tool, args.as_object().expect("an object"), chat)
        .await
    {
        Described::Action(action) => action,
        Described::Invalid(why) => panic!("{tool} {args}: {why}"),
    }
}

fn decide(action: &Action, allow_all: bool, untrusted: bool) -> Decision {
    let turn = TurnContext {
        tool_enabled: true,
        server_ready: true,
        untrusted_in_turn: untrusted,
        allow_all,
        ..Default::default()
    };
    policy::evaluate(action, &turn, &BTreeSet::<Scope>::new())
}

#[tokio::test]
async fn file_tools_ask_only_for_deletes_and_sensitive_places() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("keep.txt"), "data\n").unwrap();
    let c = chat(&root);
    let files = FilesServer::default();

    // Reads, writes and edits in an approved folder: no card in Allow all.
    for (tool, args) in [
        ("read_file", json!({"path": "keep.txt"})),
        ("list_dir", json!({"path": "."})),
        ("search_files", json!({"path": ".", "query": "data"})),
        ("write_file", json!({"path": "new.txt", "content": "hello"})),
        (
            "write_file",
            json!({"path": "keep.txt", "content": "other data\n"}),
        ),
        (
            "edit_file",
            json!({"path": "keep.txt", "old_text": "data", "new_text": "words"}),
        ),
        ("write_file", json!({"path": ".gitignore", "content": "x"})),
    ] {
        let action = describe(&files, tool, args.clone(), &c).await;
        assert_eq!(action.keep_asking, None, "{tool} {args}");
        assert!(!action.network, "{tool} {args}");
        for untrusted in [false, true] {
            assert_eq!(
                decide(&action, true, untrusted),
                Decision::AllowedByMode,
                "{tool} {args}"
            );
        }
        // Ask mode still asks, as before.
        assert!(
            matches!(
                decide(&action, false, false),
                Decision::Ask {
                    card: CardKind::Grantable,
                    reason: AskReason::Normal
                }
            ),
            "{tool} {args}"
        );
    }

    // Deleting, and leaving a file empty by writing or by editing.
    for (tool, args) in [
        ("delete_file", json!({"path": "keep.txt"})),
        ("write_file", json!({"path": "keep.txt", "content": ""})),
        (
            "edit_file",
            json!({"path": "keep.txt", "old_text": "data\n", "new_text": ""}),
        ),
    ] {
        let action = describe(&files, tool, args.clone(), &c).await;
        assert_eq!(action.risk, Risk::AlwaysAsk, "{tool} {args}");
        assert_eq!(
            action.keep_asking,
            Some(KeepAsking::Delete),
            "{tool} {args}"
        );
        for untrusted in [false, true] {
            assert_eq!(
                decide(&action, true, untrusted),
                STILL_ASKS,
                "{tool} {args}"
            );
        }
    }

    // A sensitive place.
    let action = describe(
        &files,
        "write_file",
        json!({"path": ".git/config", "content": "[core]"}),
        &c,
    )
    .await;
    assert_eq!(action.keep_asking, Some(KeepAsking::Sensitive));
    assert_eq!(decide(&action, true, false), STILL_ASKS);

    // Outside the approved folders: a denial in both modes, never a card.
    let outside = tempfile::tempdir().unwrap();
    let action = describe(
        &files,
        "delete_file",
        json!({"path": outside.path().join("x.txt").display().to_string()}),
        &c,
    )
    .await;
    assert!(action.hard_limit.is_some());
    for allow_all in [false, true] {
        assert!(matches!(
            decide(&action, allow_all, false),
            Decision::Denied { .. }
        ));
    }
}

fn terminal(shell: TerminalShell) -> TerminalServer {
    let base_env: BTreeMap<String, String> = std::env::vars()
        .filter(|(k, _)| k == "PATH" || k == "HOME")
        .collect();
    TerminalServer::new(TerminalConfig {
        shell,
        base_env,
        pass_through: vec![],
    })
}

#[tokio::test]
async fn the_terminal_carries_the_classifiers_allow_all_facts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let c = chat(&root);
    let sh = terminal(TerminalShell::Sh);
    let ps = terminal(TerminalShell::PowerShell {
        program: "powershell.exe".into(),
    });
    let run = |command: &str| json!({ "command": command });

    // (shell, command, keeps asking, can reach the network)
    let rows: &[(&TerminalServer, &str, Option<KeepAsking>, bool)] = &[
        (&sh, "cargo test", None, false),
        (&sh, "npm run build", None, false),
        (&sh, "python script.py", None, false),
        (&sh, "ls -la", None, false),
        (&sh, "rm -rf build", Some(KeepAsking::Delete), false),
        (&sh, "git clean -fdx", Some(KeepAsking::Delete), false),
        (
            &sh,
            "python -c \"import shutil; shutil.rmtree('d')\"",
            Some(KeepAsking::Delete),
            true,
        ),
        (&sh, "git reset --hard", Some(KeepAsking::Destroy), false),
        (&sh, "sudo make install", Some(KeepAsking::Destroy), false),
        (&sh, "git push", Some(KeepAsking::Publish), true),
        (&sh, "curl https://example.com", None, true),
        // A redirection to a file and a variable as an argument hide nothing: every
        // program still stands in the line by name.
        (&sh, "npm test > log.txt", None, false),
        (&sh, "echo $HOME", None, false),
        // A substitution can hide what runs; an empty redirection leaves a file empty.
        (&sh, "echo $(date) > log.txt", None, true),
        (&sh, ": > log.txt", Some(KeepAsking::Delete), false),
        (&ps, "npm run build", None, false),
        (&ps, "git commit -m \"Fix the build\"", None, false),
        (
            &ps,
            "Remove-Item -Recurse build",
            Some(KeepAsking::Delete),
            false,
        ),
        (&ps, "del x", Some(KeepAsking::Delete), false),
        (&ps, "cmd /c del x", Some(KeepAsking::Delete), true),
        (&ps, "format D:", Some(KeepAsking::Destroy), false),
        (
            &ps,
            "powershell -EncodedCommand SQBFAFgA",
            Some(KeepAsking::Hidden),
            true,
        ),
        (&ps, "Invoke-WebRequest https://example.com", None, true),
    ];
    for (server, command, keep_asking, network) in rows {
        let action = describe(*server, "run_command", run(command), &c).await;
        assert_eq!(action.keep_asking, *keep_asking, "{command}");
        assert_eq!(action.network, *network, "{command}");
        assert!(action.outbound, "the terminal is outbound in Ask mode");
        let expected = |untrusted: bool| match (keep_asking, network, untrusted) {
            (Some(_), _, _) => STILL_ASKS,
            (None, true, true) => ASKS_AFTER_UNTRUSTED,
            _ => Decision::AllowedByMode,
        };
        for untrusted in [false, true] {
            assert_eq!(
                decide(&action, true, untrusted),
                expected(untrusted),
                "{command} (untrusted {untrusted})"
            );
        }
        // Ask mode never allows a command without a grant, whatever the new facts say.
        assert!(
            matches!(decide(&action, false, false), Decision::Ask { .. }),
            "{command}"
        );
    }

    // In Ask mode a quoted PowerShell argument still always asks (no grant covers it).
    let quoted = describe(
        &ps,
        "run_command",
        run("git commit -m \"Fix the build\""),
        &c,
    )
    .await;
    assert_eq!(quoted.risk, Risk::AlwaysAsk);
    // A command with nowhere to run is a hard limit in both modes.
    let nowhere = describe(&sh, "run_command", json!({"command": "ls", "cwd": "/"}), &c).await;
    for allow_all in [false, true] {
        assert!(matches!(
            decide(&nowhere, allow_all, false),
            Decision::Denied { .. }
        ));
    }
}

fn git_server() -> Option<GitServer> {
    let path = std::env::var("PATH").ok();
    let git = runtime::resolve_program("git", path.as_deref().map(std::ffi::OsStr::new))?;
    let env: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| k == "PATH" || k == "HOME")
        .collect();
    Some(GitServer::new(GitConfig {
        git: Some(git),
        env,
        credential_helpers: vec![],
    }))
}

#[tokio::test]
async fn git_push_keeps_asking_and_the_network_tools_ask_after_untrusted_content() {
    let Some(git) = git_server() else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let repo = git2::Repository::init(&root).unwrap();
    repo.remote("origin", "https://example.com/r.git").unwrap();
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    let c = chat(&root);

    // Local git: no card in Allow all.
    for (tool, args) in [
        ("git_status", json!({})),
        ("git_diff", json!({})),
        ("git_log", json!({})),
        ("git_commit", json!({"message": "Add a"})),
    ] {
        let action = describe(&git, tool, args, &c).await;
        assert_eq!(
            (action.keep_asking, action.network),
            (None, false),
            "{tool}"
        );
        for untrusted in [false, true] {
            assert_eq!(
                decide(&action, true, untrusted),
                Decision::AllowedByMode,
                "{tool}"
            );
        }
    }

    // Pull and clone reach the network: allowed until untrusted content is in the turn.
    let pull = describe(&git, "git_pull", json!({}), &c).await;
    let clone = describe(
        &git,
        "git_clone",
        json!({"url": "https://example.com/other.git", "destination": "other"}),
        &c,
    )
    .await;
    for action in [&pull, &clone] {
        assert_eq!((action.keep_asking, action.network), (None, true));
        assert_eq!(decide(action, true, false), Decision::AllowedByMode);
        assert_eq!(decide(action, true, true), ASKS_AFTER_UNTRUSTED);
    }

    // A push publishes in the user's name: it asks in every mode.
    let push = describe(&git, "git_push", json!({}), &c).await;
    assert_eq!(push.risk, Risk::AlwaysAsk);
    assert_eq!(push.keep_asking, Some(KeepAsking::Publish));
    assert!(push.network);
    for untrusted in [false, true] {
        assert_eq!(decide(&push, true, untrusted), STILL_ASKS);
    }
    assert_eq!(
        decide(&push, false, false),
        Decision::Ask {
            card: CardKind::OnceOnly,
            reason: AskReason::AlwaysAsk
        }
    );
}
