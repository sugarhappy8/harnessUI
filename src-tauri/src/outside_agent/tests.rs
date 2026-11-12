//! Coding apps (0.0.6): the templates and the card, finding the programs, the policy
//! decision in every mode, and runs of fake programs that record their arguments, standard
//! input, working folder and environment. No real Claude Code or Codex is ever run: every
//! search below is given its own `PATH` of temporary folders.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mcp_host::{BuiltinServer, CallContext, ChatContext, Described, ToolOutput};
use policy::command::coding_apps::{BYPASS_TOKENS, bypass_token};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::output::{End, Lines, Parsed, result};
use super::*;

// ---- Helpers -------------------------------------------------------------------------------

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("coding-apps-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dunce::canonicalize(&dir).unwrap()
}

/// Writes an executable script.
#[cfg(unix)]
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// A fake app: records its arguments (one per line), standard input, working folder and
/// environment next to itself, then prints `output`.
#[cfg(unix)]
fn fake(dir: &Path, name: &str, output: &[&str]) -> PathBuf {
    let mut body = String::from(
        "#!/bin/sh\nd=\"$(dirname \"$0\")\"\nprintf '%s\\n' \"$@\" > \"$d/args.txt\"\ncat > \"$d/stdin.txt\"\npwd > \"$d/cwd.txt\"\nenv > \"$d/env.txt\"\n",
    );
    for line in output {
        body.push_str(&format!(
            "printf '%s\\n' '{}'\n",
            line.replace('\'', "'\\''")
        ));
    }
    script(dir, name, &body)
}

fn chat(work: &Path) -> ChatContext {
    ChatContext {
        chat_id: 1,
        approved_folders: vec![work.display().to_string()],
        working_folder: Some(work.display().to_string()),
        result_cap_tokens: 4000,
        ..Default::default()
    }
}

fn args(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap()
}

fn no_share() -> SharePort {
    Arc::new(|| Box::pin(async { None }))
}

fn base_env() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ("HOME".to_string(), "/nonexistent-home".to_string()),
    ])
}

/// A server whose apps were found in `bin` (and nowhere else).
fn server_for(bin: &Path, env: BTreeMap<String, String>) -> CodingAppsServer {
    let found = CodingApp::ALL
        .into_iter()
        .map(|a| find_app(a, Some(bin.as_os_str()), None, &[]))
        .collect();
    let mut s = CodingAppsServer::new(
        SavedSetup {
            checked_at: Some(1),
            apps: found,
        },
        env,
        None,
        no_share(),
    );
    s.windows = false;
    s
}

/// What the store holds: every approved folder, and the asking chat's own (`None`: they
/// could not be read).
fn approved_store(all: Vec<String>, chat: Option<Vec<String>>) -> Approved {
    Arc::new(move |_| {
        let now = ApprovedNow {
            all: all.clone(),
            chat: chat.clone(),
        };
        Box::pin(async move { now })
    })
}

/// The chat's tool context in Full access, as `turns::AppTools::chat_now` builds it: its
/// own folders, then the whole computer, and `home` as the working folder when it has none.
fn in_full_access(mut chat: ChatContext, home: &Path) -> ChatContext {
    crate::approvals::widen(&mut chat, &crate::approvals::whole_computer(Some(home)));
    chat
}

fn turn_in(
    allow_all: bool,
    full_access: bool,
    untrusted: bool,
    recovered: bool,
) -> policy::TurnContext {
    policy::TurnContext {
        tool_enabled: true,
        server_ready: true,
        untrusted_in_turn: untrusted,
        recovered_from_text: recovered,
        allow_all,
        full_access,
    }
}

async fn describe(s: &CodingAppsServer, tool: &str, a: Value, c: &ChatContext) -> policy::Action {
    match s.describe(tool, &args(a), c).await {
        Described::Action(action) => action,
        Described::Invalid(why) => panic!("invalid: {why}"),
    }
}

async fn call(
    s: &CodingAppsServer,
    action: &policy::Action,
    c: &ChatContext,
) -> (ToolOutput, String) {
    let progress = Arc::new(Mutex::new(String::new()));
    let sink = progress.clone();
    let out = s
        .call(
            &action.tool,
            &Map::new(),
            action,
            CallContext {
                chat: c.clone(),
                cancel: CancellationToken::new(),
                progress: Some(Arc::new(move |t: String| sink.lock().unwrap().push_str(&t))),
            },
        )
        .await;
    let p = progress.lock().unwrap().clone();
    (out, p)
}

const CLAUDE_OK: &[&str] = &[
    r#"{"type":"system","subtype":"init","tools":["Glob","Grep","Read"]}"#,
    r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"notes.txt"}}]}}"#,
    r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking at the notes."}]}}"#,
    r#"{"type":"result","subtype":"success","is_error":false,"result":"FAKE-CLAUDE-ANSWER: the word is heron"}"#,
];

const CODEX_OK: &[&str] = &[
    r#"{"type":"thread.started","thread_id":"t1"}"#,
    r#"{"type":"turn.started"}"#,
    r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"bash -lc ls","status":"in_progress"}}"#,
    r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"FAKE-CODEX-ANSWER"}}"#,
    r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":2}}"#,
];

// ---- Templates and the card --------------------------------------------------------------

/// Every template, word for word.
#[test]
fn arguments_are_exact_per_template_and_mode() {
    let words = |a: CodingApp, e: bool| arguments(a, e);
    assert_eq!(
        words(CodingApp::ClaudeCode, false),
        [
            "-p",
            "--restricted",
            "--safe-mode",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--permission-mode",
            "default",
            "--tools",
            "Read,Grep,Glob",
            "--settings",
            r#"{"disableAllHooks":true,"permissions":{"blockReadsOutsideWorkingDirectories":true}}"#,
            "--output-format",
            "stream-json",
            "--verbose",
        ]
    );
    assert_eq!(
        words(CodingApp::ClaudeCode, true),
        [
            "-p",
            "--restricted",
            "--safe-mode",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--permission-mode",
            "acceptEdits",
            "--tools",
            "Read,Grep,Glob,Edit,Write",
            "--settings",
            r#"{"disableAllHooks":true,"permissions":{"blockReadsOutsideWorkingDirectories":true}}"#,
            "--output-format",
            "stream-json",
            "--verbose",
        ]
    );
    assert_eq!(
        words(CodingApp::Codex, false),
        [
            "exec",
            "--json",
            "--sandbox",
            "read-only",
            "-c",
            "features.hooks=false",
            "-"
        ]
    );
    assert_eq!(
        words(CodingApp::Codex, true),
        [
            "exec",
            "--json",
            "--sandbox",
            "workspace-write",
            "-c",
            "features.hooks=false",
            "-"
        ]
    );
    // The settings are real JSON with exactly the two settings.
    let settings: Value = serde_json::from_str(CLAUDE_SETTINGS).unwrap();
    assert_eq!(
        settings,
        json!({"disableAllHooks": true, "permissions": {"blockReadsOutsideWorkingDirectories": true}})
    );
}

/// No token of the classifier's bypass rule can appear in any template, and every
/// required flag is there; the classifier itself finds no bypass token in the line.
#[test]
fn no_bypass_token_and_every_required_flag() {
    assert_eq!(BYPASS_TOKENS.len(), 8);
    for app in CodingApp::ALL {
        for may_edit in [false, true] {
            let words = arguments(app, may_edit);
            let line = words.join(" ");
            for token in BYPASS_TOKENS {
                let token = token.to_lowercase();
                assert!(
                    words.iter().all(|w| !w.to_lowercase().contains(&token)),
                    "{app:?} {may_edit}: {token}"
                );
            }
            assert_eq!(bypass_token(&line), None, "{line}");
            for flag in required_flags(app) {
                assert!(
                    words.iter().any(|w| w == flag),
                    "{app:?} {may_edit}: {flag} missing"
                );
            }
            // The only word the model's choice changes is the mode.
            let other = arguments(app, !may_edit);
            let differ = words.iter().zip(&other).filter(|(a, b)| a != b).count();
            assert!(differ <= 2 && words.len() == other.len(), "{app:?}");
        }
    }
    // The task cannot run Claude Code's own commands (`/config`, `/mcp`, …), which change
    // its settings without a model call (review of 0.0.6, round 1).
    assert!(required_flags(CodingApp::ClaudeCode).contains(&"--disable-slash-commands"));
    // The same check guards the tools at run time.
    assert!(CodingApp::ALL.into_iter().all(template_ok));
    // Claude Code's mode words are documented values, never bypassPermissions.
    for may_edit in [false, true] {
        let w = arguments(CodingApp::ClaudeCode, may_edit);
        let mode = &w[w.iter().position(|x| x == "--permission-mode").unwrap() + 1];
        assert!(
            ["default", "acceptEdits"].contains(&mode.as_str()),
            "{mode}"
        );
        let tools = &w[w.iter().position(|x| x == "--tools").unwrap() + 1];
        assert!(
            !tools.contains("Bash") && !tools.contains("WebFetch"),
            "{tools}"
        );
    }
}

/// The card says what the template confines, and nothing the template does not.
#[test]
fn the_card_states_only_what_the_template_confines() {
    let folder = "/home/u/proj";
    for app in CodingApp::ALL {
        for may_edit in [false, true] {
            for windows in [false, true] {
                let words = arguments(app, may_edit);
                let card = card_lines(app, may_edit, folder, windows).join("\n");
                match app {
                    CodingApp::ClaudeCode => {
                        // --restricted confines the file tools to the working folder.
                        assert!(words.contains(&"--restricted".to_string()));
                        let edits = words.iter().any(|w| w.contains("Edit"));
                        assert_eq!(edits, may_edit);
                        assert_eq!(words.contains(&"acceptEdits".to_string()), may_edit);
                        if edits {
                            assert_eq!(
                                card,
                                format!("It can read and change files only in {folder}.")
                            );
                        } else {
                            assert_eq!(card, format!("It can only read files in {folder}."));
                        }
                    }
                    CodingApp::Codex => {
                        let sandbox =
                            &words[words.iter().position(|w| w == "--sandbox").unwrap() + 1];
                        // Codex's read-only sandbox does not keep reads in the folder.
                        assert!(card.starts_with(
                            "It can read files elsewhere on this computer that you can read."
                        ));
                        assert!(!card.contains("only"), "{card}");
                        assert_eq!(sandbox == "workspace-write", may_edit);
                        assert_eq!(
                            card.contains(&format!("It can change files in {folder}.")),
                            may_edit
                        );
                        assert_eq!(
                            card.contains("It can also change files in the temporary folder and run commands in its sandbox."),
                            may_edit
                        );
                        assert_eq!(
                            card.contains("On Windows, Codex's sandbox has not been tried yet."),
                            windows
                        );
                    }
                }
            }
        }
    }
    assert_eq!(
        online_line(CodingApp::ClaudeCode, false),
        "It runs online with your Claude Code account and uses your plan or credit. What it reads is sent to Anthropic. It can take several minutes."
    );
    assert_eq!(
        online_line(CodingApp::Codex, false),
        "It runs online with your Codex account and uses your plan or credit. What it reads is sent to OpenAI. It can take several minutes."
    );
    assert_eq!(
        online_line(CodingApp::ClaudeCode, true),
        "Claude Code is set up to use the model in harnessUI, so this task goes to your local model, not online."
    );
}

#[test]
fn versions_and_the_documented_first_versions() {
    assert_eq!(parse_version("2.1.322"), Some((2, 1, 322)));
    assert_eq!(parse_version("v2.1.248"), Some((2, 1, 248)));
    assert_eq!(parse_version("2.1.300-beta.1"), Some((2, 1, 300)));
    assert_eq!(parse_version("2.1"), None);
    assert_eq!(parse_version("x.y.z"), None);
    assert_eq!(parse_version("1.2.3.4"), None);
    assert_eq!(missing_flag(CodingApp::ClaudeCode, Some("2.1.322")), None);
    assert_eq!(missing_flag(CodingApp::ClaudeCode, Some("2.1.257")), None);
    // `--restricted` came in 2.1.248, the read block of the settings in 2.1.257: a version
    // between them has the flag but would drop the setting, so it is too old as well.
    let setting = "permissions.blockReadsOutsideWorkingDirectories";
    assert_eq!(
        missing_flag(CodingApp::ClaudeCode, Some("2.1.256")),
        Some((setting, (2, 1, 257)))
    );
    assert_eq!(
        missing_flag(CodingApp::ClaudeCode, Some("2.1.248")),
        Some((setting, (2, 1, 257)))
    );
    // Older still: the newest requirement is named, so updating to it is enough.
    assert_eq!(
        missing_flag(CodingApp::ClaudeCode, Some("2.1.247")),
        Some((setting, (2, 1, 257)))
    );
    assert_eq!(
        missing_flag(CodingApp::ClaudeCode, Some("1.0.0")),
        Some((setting, (2, 1, 257)))
    );
    // Unknown: not refused here (a refusal by the program itself is reported instead).
    assert_eq!(missing_flag(CodingApp::ClaudeCode, None), None);
    assert_eq!(missing_flag(CodingApp::Codex, Some("0.1.0")), None);
    // Every flag or setting with a documented first version is in its template, in both
    // modes: a flag as a word, a setting inside the settings given with `--settings`.
    assert!(FLAG_SINCE.iter().any(|(_, f, _)| *f == "--restricted"));
    // `--disable-slash-commands` came in 2.0.60 (Claude Code's changelog), before the others.
    assert!(FLAG_SINCE.contains(&(
        CodingApp::ClaudeCode,
        "--disable-slash-commands",
        (2, 0, 60)
    )));
    assert_eq!(
        missing_flag(CodingApp::ClaudeCode, Some("2.0.59")),
        Some((setting, (2, 1, 257)))
    );
    for (app, flag, _) in FLAG_SINCE {
        for may_edit in [false, true] {
            let words = arguments(*app, may_edit);
            let found = if let Some(path) = flag.strip_prefix("permissions.") {
                let at = words
                    .iter()
                    .position(|w| w == "--settings")
                    .expect("--settings");
                let settings: Value = serde_json::from_str(&words[at + 1]).unwrap();
                settings["permissions"][path] == json!(true)
            } else {
                words.iter().any(|w| w == flag)
            };
            assert!(found, "{flag} not in the template");
        }
    }
}

// ---- Finding the programs --------------------------------------------------------------

#[cfg(unix)]
#[test]
fn programs_are_found_on_path_then_in_the_users_bin_and_never_run() {
    let a = temp("path-a");
    let b = temp("path-b");
    let home = temp("home");
    // A marker the program would write if it were run.
    let marker = a.join("was-run");
    script(
        &a,
        "codex",
        &format!("#!/bin/sh\ntouch {}\n", marker.display()),
    );
    script(&b, "claude", "#!/bin/sh\nexit 0\n");
    let path = std::env::join_paths([&a, &b]).unwrap();
    assert_eq!(
        locate(CodingApp::Codex, Some(&path), None, false),
        Some(a.join("codex"))
    );
    assert_eq!(
        locate(CodingApp::ClaudeCode, Some(&path), None, false),
        Some(b.join("claude"))
    );
    assert!(!marker.exists(), "locating ran the program");
    // Not on PATH: ~/.local/bin, where Claude Code's installer puts it.
    let bin = home.join(".local").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(&bin, "claude", "#!/bin/sh\nexit 0\n");
    let only_a = a.clone().into_os_string();
    assert_eq!(
        locate(CodingApp::ClaudeCode, Some(&only_a), Some(&home), false),
        Some(bin.join("claude"))
    );
    // A relative PATH entry (the current folder) is never searched.
    let rel = std::ffi::OsString::from(".");
    assert_eq!(locate(CodingApp::ClaudeCode, Some(&rel), None, false), None);
    // A file that cannot run is not a program.
    let c = temp("path-c");
    std::fs::write(c.join("claude"), "not executable").unwrap();
    assert_eq!(
        locate(CodingApp::ClaudeCode, Some(c.as_os_str()), None, false),
        None
    );
    // Neither found.
    let empty = temp("empty");
    let none = find_app(CodingApp::Codex, Some(empty.as_os_str()), None, &[]);
    assert_eq!(none.problem, Some(Problem::NotFound));
    assert!(!none.usable());
}

#[cfg(unix)]
#[test]
fn a_link_is_followed_and_the_version_read_from_the_files_beside_it() {
    // An npm install: bin/claude -> ../lib/node_modules/@anthropic-ai/claude-code/cli.js
    let prefix = temp("npm");
    let pkg = prefix.join("lib/node_modules/@anthropic-ai/claude-code");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        pkg.join("package.json"),
        r#"{"name":"@anthropic-ai/claude-code","version":"2.1.300"}"#,
    )
    .unwrap();
    script(&pkg, "cli.js", "#!/bin/sh\nexit 0\n");
    std::fs::create_dir_all(prefix.join("bin")).unwrap();
    std::os::unix::fs::symlink(pkg.join("cli.js"), prefix.join("bin/claude")).unwrap();
    let found = find_app(
        CodingApp::ClaudeCode,
        Some(prefix.join("bin").as_os_str()),
        None,
        &[],
    );
    assert_eq!(
        found.path.as_deref(),
        Some(prefix.join("bin/claude").to_str().unwrap())
    );
    assert_eq!(
        found.program.as_deref(),
        Some(pkg.join("cli.js").to_str().unwrap())
    );
    assert_eq!(found.version.as_deref(), Some("2.1.300"));
    assert!(found.usable(), "{found:?}");
    // Another package's package.json is not taken for this one's.
    std::fs::write(
        pkg.join("package.json"),
        r#"{"name":"other","version":"9.9.9"}"#,
    )
    .unwrap();
    assert_eq!(
        version_near(CodingApp::ClaudeCode, &pkg.join("cli.js")),
        None
    );
    // The native installer: ~/.local/share/claude/versions/2.1.200, too old for the template.
    let native = temp("native");
    let versions = native.join("versions");
    std::fs::create_dir_all(&versions).unwrap();
    script(&versions, "2.1.200", "#!/bin/sh\nexit 0\n");
    let bin = native.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(versions.join("2.1.200"), bin.join("claude")).unwrap();
    let old = find_app(CodingApp::ClaudeCode, Some(bin.as_os_str()), None, &[]);
    assert_eq!(old.version.as_deref(), Some("2.1.200"));
    assert_eq!(
        old.problem,
        Some(Problem::TooOld {
            version: "2.1.200".into(),
            flag: "permissions.blockReadsOutsideWorkingDirectories".into(),
            minimum: "2.1.257".into()
        })
    );
    assert!(!old.usable());
    let words = old.problem.unwrap().words(CodingApp::ClaudeCode);
    assert!(
        words.contains("too old") && words.contains("2.1.257"),
        "{words}"
    );
}

/// A program inside an approved folder is refused, when it is found and when the chat
/// approved that folder later.
#[cfg(unix)]
#[tokio::test]
async fn a_program_inside_an_approved_folder_is_refused() {
    let project = temp("project");
    let bin = project.join("node_modules").join(".bin");
    std::fs::create_dir_all(&bin).unwrap();
    fake(&bin, "claude", CLAUDE_OK);
    let found = find_app(
        CodingApp::ClaudeCode,
        Some(bin.as_os_str()),
        None,
        &[project.display().to_string()],
    );
    assert!(
        matches!(&found.problem, Some(Problem::InsideApprovedFolder { folder, .. }) if *folder == project.display().to_string()),
        "{found:?}"
    );
    assert!(!found.usable());
    // Found while the folder was not approved; the chat approves it later.
    let s = server_for(&bin, base_env());
    assert_eq!(
        s.tools().len(),
        1,
        "claude is usable until a chat approves its folder"
    );
    let action = describe(&s, CLAUDE_TOOL, json!({"task": "hello"}), &chat(&project)).await;
    let why = action.hard_limit.clone().expect("refused");
    assert!(why.contains("inside a folder you approved"), "{why}");
    assert_eq!(
        policy::evaluate(
            &action,
            &policy::TurnContext {
                tool_enabled: true,
                server_ready: true,
                ..Default::default()
            },
            &BTreeSet::new()
        ),
        policy::Decision::Denied { reason: why }
    );
    assert!(!bin.join("args.txt").exists(), "it ran");
    // A folder approved only for another chat counts too.
    let elsewhere = temp("elsewhere-work");
    let s = server_for(&bin, base_env()).with_approved(approved_store(
        vec![project.display().to_string()],
        Some(vec![elsewhere.display().to_string()]),
    ));
    let action = describe(&s, CLAUDE_TOOL, json!({"task": "hello"}), &chat(&elsewhere)).await;
    assert!(
        action
            .hard_limit
            .as_deref()
            .is_some_and(|w| w.contains("inside a folder you approved")),
        "{:?}",
        action.hard_limit
    );
    assert!(!bin.join("args.txt").exists(), "it ran");
}

/// A link on `PATH` inside an approved folder is refused wherever it leads: where it
/// stands counts, not only the file it leads to (a link `codex` -> `/bin/sh` would run the
/// file `exec` of the working folder). In Ask first and in Full access (review of 0.0.6,
/// round 1).
#[cfg(unix)]
#[tokio::test]
async fn a_link_inside_an_approved_folder_is_refused_wherever_it_leads() {
    let project = temp("link-project");
    let outside = temp("link-outside");
    let target = fake(&outside, "real-codex", CODEX_OK);
    let bin = project.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(&target, bin.join("codex")).unwrap();
    let approved = vec![project.display().to_string()];
    let found = find_app(CodingApp::Codex, Some(bin.as_os_str()), None, &approved);
    assert!(
        matches!(&found.problem, Some(Problem::InsideApprovedFolder { folder, .. }) if *folder == project.display().to_string()),
        "{found:?}"
    );
    assert!(!found.usable());
    // A link elsewhere that leads through a link inside the folder counts too: the project
    // controls that step.
    let elsewhere = temp("link-elsewhere");
    std::os::unix::fs::symlink(bin.join("codex"), elsewhere.join("codex")).unwrap();
    let through = find_app(
        CodingApp::Codex,
        Some(elsewhere.as_os_str()),
        None,
        &approved,
    );
    assert!(
        matches!(&through.problem, Some(Problem::InsideApprovedFolder { folder, .. }) if *folder == project.display().to_string()),
        "{through:?}"
    );
    // Found while the folder was not approved; the chat approves it later.
    let s = server_for(&bin, base_env());
    assert_eq!(s.tools().len(), 1);
    let refused = |action: &policy::Action| {
        action
            .hard_limit
            .clone()
            .filter(|w| w.contains("inside a folder you approved"))
            .unwrap_or_else(|| panic!("not refused: {:?}", action.hard_limit))
    };
    let action = describe(&s, CODEX_TOOL, json!({"task": "hello"}), &chat(&project)).await;
    let why = refused(&action);
    assert_eq!(
        policy::evaluate(
            &action,
            &turn_in(false, false, false, false),
            &BTreeSet::new()
        ),
        policy::Decision::Denied { reason: why }
    );
    // Full access: the stored folders count, and the link is still refused.
    let home = temp("link-home");
    let s = server_for(&bin, base_env())
        .with_approved(approved_store(approved.clone(), Some(approved.clone())));
    let action = describe(
        &s,
        CODEX_TOOL,
        json!({"task": "hello"}),
        &in_full_access(chat(&project), &home),
    )
    .await;
    let why = refused(&action);
    for allow_all in [false, true] {
        assert_eq!(
            policy::evaluate(
                &action,
                &turn_in(allow_all, true, false, false),
                &BTreeSet::new()
            ),
            policy::Decision::Denied {
                reason: why.clone()
            }
        );
    }
    assert!(!outside.join("args.txt").exists(), "it ran");
    assert!(!bin.join("args.txt").exists(), "it ran");
}

/// When the approved folder holds the home folder, installing the app "in its usual place"
/// cannot help (its usual places are in the home folder): the message says to approve a
/// narrower folder instead.
#[cfg(unix)]
#[test]
fn inside_the_home_folder_the_message_says_to_approve_a_narrower_folder() {
    let home = temp("approved-home");
    let bin = home.join(".local").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(&bin, "claude", "#!/bin/sh\nexit 0\n");
    let empty = temp("approved-home-path");
    let found = find_app(
        CodingApp::ClaudeCode,
        Some(empty.as_os_str()),
        Some(&home),
        &[home.display().to_string()],
    );
    let problem = found.problem.expect("refused");
    assert!(
        matches!(
            &problem,
            Problem::InsideApprovedFolder {
                holds_home: true,
                ..
            }
        ),
        "{problem:?}"
    );
    let words = problem.words(CodingApp::ClaudeCode);
    assert!(
        words.contains("Approve a narrower folder, such as a project folder,")
            && !words.contains("usual place"),
        "{words}"
    );
    // A project folder that does not hold the home folder keeps the old advice.
    let project = temp("approved-project");
    let pbin = project.join("bin");
    std::fs::create_dir_all(&pbin).unwrap();
    script(&pbin, "claude", "#!/bin/sh\nexit 0\n");
    let found = find_app(
        CodingApp::ClaudeCode,
        Some(pbin.as_os_str()),
        Some(&home),
        &[project.display().to_string()],
    );
    let problem = found.problem.expect("refused");
    assert!(
        matches!(
            &problem,
            Problem::InsideApprovedFolder {
                holds_home: false,
                ..
            }
        ),
        "{problem:?}"
    );
    assert!(
        problem
            .words(CodingApp::ClaudeCode)
            .contains("Install Claude Code in its usual place"),
        "{problem:?}"
    );
    // A setup saved before 0.0.6's round-1 fix (no holds_home) still reads.
    let old: Problem =
        serde_json::from_str(r#"{"kind":"insideApprovedFolder","folder":"/p"}"#).unwrap();
    assert_eq!(
        old,
        Problem::InsideApprovedFolder {
            folder: "/p".into(),
            holds_home: false
        }
    );
}

/// The file must be the one found: a replaced file, a changed one and a link that now
/// leads elsewhere are refused before anything runs.
#[cfg(unix)]
#[tokio::test]
async fn a_swapped_or_changed_file_is_refused() {
    let bin = temp("swap-bin");
    let work = temp("swap-work");
    let real = temp("swap-real");
    let first = fake(&real, "claude-1", CLAUDE_OK);
    let second = fake(&real, "claude-2", CLAUDE_OK);
    std::os::unix::fs::symlink(&first, bin.join("claude")).unwrap();
    let s = server_for(&bin, base_env());
    let saved = s.saved.get(CodingApp::ClaudeCode).unwrap().clone();
    assert!(unchanged(&saved).is_ok());
    let action = describe(&s, CLAUDE_TOOL, json!({"task": "hello"}), &chat(&work)).await;
    assert!(action.hard_limit.is_none(), "{:?}", action.hard_limit);
    // The link now leads to another file.
    std::fs::remove_file(bin.join("claude")).unwrap();
    std::os::unix::fs::symlink(&second, bin.join("claude")).unwrap();
    assert_eq!(unchanged(&saved), Err(Problem::Changed));
    let refused = describe(&s, CLAUDE_TOOL, json!({"task": "hello"}), &chat(&work)).await;
    assert!(
        refused
            .hard_limit
            .as_deref()
            .is_some_and(|w| w.contains("changed on this computer")),
        "{:?}",
        refused.hard_limit
    );
    // The approved call does not run either.
    let (out, _) = call(&s, &action, &chat(&work)).await;
    assert!(
        out.is_error && out.joined_text().contains("Nothing was run"),
        "{out:?}"
    );
    assert!(!real.join("args.txt").exists(), "it ran");
    // Back to the first file, which is then rewritten: its size and time change.
    std::fs::remove_file(bin.join("claude")).unwrap();
    std::os::unix::fs::symlink(&first, bin.join("claude")).unwrap();
    assert!(unchanged(&saved).is_ok());
    std::thread::sleep(Duration::from_millis(20));
    let mut body = std::fs::read_to_string(&first).unwrap();
    body.push_str("# changed\n");
    std::fs::write(&first, body).unwrap();
    assert_eq!(unchanged(&saved), Err(Problem::Changed));
    // Replaced by a new file of the same name and content (another inode).
    let again = std::fs::read(&first).unwrap();
    std::fs::remove_file(&first).unwrap();
    std::fs::write(&first, again).unwrap();
    assert_eq!(unchanged(&saved), Err(Problem::Changed));
}

/// The group lists the tools of the usable apps only; before the first search, none.
#[cfg(unix)]
#[test]
fn only_usable_apps_are_offered() {
    assert!(CodingAppsServer::unchecked().tools().is_empty());
    let bin = temp("offer");
    fake(&bin, "codex", CODEX_OK);
    let s = server_for(&bin, base_env());
    let names: Vec<String> = s.tools().into_iter().map(|t| t.name).collect();
    assert_eq!(names, [CODEX_TOOL]);
    let def = &s.tools()[0];
    assert_eq!(def.input_schema["required"], json!(["task"]));
    assert_eq!(def.input_schema["additionalProperties"], json!(false));
    assert_eq!(
        def.input_schema["properties"]["may_edit"]["type"],
        "boolean"
    );
    let status = status_of(&s.saved);
    assert!(status.checked && status.message.is_none());
    assert!(status.apps[1].usable && status.apps[1].path.is_some());
    assert!(!status.apps[0].usable);
    assert_eq!(
        status.apps[0].problem.as_deref(),
        Some("Claude Code is not installed on this computer.")
    );
    // Neither found: the panel says so.
    let none = server_for(&temp("none"), base_env());
    assert!(none.tools().is_empty());
    assert_eq!(
        status_of(&none.saved).message.as_deref(),
        Some(NOT_INSTALLED)
    );
    // Never looked for: nothing to say yet.
    let fresh = status_of(&SavedSetup::default());
    assert!(
        !fresh.checked && fresh.message.is_none() && fresh.apps.iter().all(|a| a.problem.is_none())
    );
}

// ---- Describing a call and the policy decision ---------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn a_call_is_described_for_the_card() {
    let bin = temp("describe-bin");
    let work = temp("describe-work");
    fake(&bin, "claude", CLAUDE_OK);
    fake(&bin, "codex", CODEX_OK);
    let s = server_for(&bin, base_env());
    let a = describe(
        &s,
        CLAUDE_TOOL,
        json!({"task": "Find the bug\nin main.rs"}),
        &chat(&work),
    )
    .await;
    assert_eq!(
        (a.server.as_str(), a.tool.as_str()),
        (SERVER_NAME, CLAUDE_TOOL)
    );
    assert_eq!(a.risk, policy::Risk::AlwaysAsk);
    assert_eq!(a.keep_asking, Some(policy::KeepAsking::Publish));
    assert!(a.network && a.outbound && !a.read_only);
    assert_eq!(a.hard_limit, None);
    assert_eq!(
        a.scope,
        policy::Scope::CodingApp {
            app: "claudeCode".into(),
            folder: work.display().to_string()
        }
    );
    assert_eq!(a.summary, "Ask Claude Code: Find the bug\\nin main.rs");
    let program = bin.join("claude").display().to_string();
    let expected = format!(
        "It can only read files in {w}.\n{online}\nExperimental.\nProgram: {program}\n\nTask:\nFind the bug\nin main.rs",
        w = work.display(),
        online = online_line(CodingApp::ClaudeCode, false)
    );
    assert_eq!(a.preview, expected);
    let edit = describe(
        &s,
        CODEX_TOOL,
        json!({"task": "Fix it", "may_edit": true}),
        &chat(&work),
    )
    .await;
    assert!(
        edit.preview
            .contains("It can also change files in the temporary folder"),
        "{}",
        edit.preview
    );
    assert!(
        edit.preview
            .contains(&format!("Program: {}", bin.join("codex").display()))
    );
    assert_eq!(edit.payload["mayEdit"], true);
    // Invisible characters in the task are shown, not hidden.
    let hidden = describe(&s, CLAUDE_TOOL, json!({"task": "a\u{202e}b"}), &chat(&work)).await;
    assert!(hidden.preview.contains("\\u{202E}"), "{}", hidden.preview);
    // An empty task is not a call.
    for task in ["", "   \n\t"] {
        assert!(matches!(
            s.describe(CLAUDE_TOOL, &args(json!({"task": task})), &chat(&work)).await,
            Described::Invalid(why) if why.contains("task is empty")
        ));
    }
    let long = "x".repeat(MAX_TASK_CHARS + 1);
    assert!(matches!(
        s.describe(CLAUDE_TOOL, &args(json!({"task": long})), &chat(&work)).await,
        Described::Invalid(why) if why.contains("too long")
    ));
    // No approved folder: nowhere to work.
    let nowhere = ChatContext::default();
    let refused = describe(&s, CLAUDE_TOOL, json!({"task": "x"}), &nowhere).await;
    assert!(
        refused
            .hard_limit
            .as_deref()
            .is_some_and(|w| w.contains("nowhere to work"))
    );
    // A working folder that is not approved.
    let mut elsewhere = chat(&work);
    elsewhere.working_folder = Some(temp("not-approved").display().to_string());
    let refused = describe(&s, CLAUDE_TOOL, json!({"task": "x"}), &elsewhere).await;
    assert!(
        refused
            .hard_limit
            .as_deref()
            .is_some_and(|w| w.contains("not approved")),
        "{:?}",
        refused.hard_limit
    );
}

/// Asks with Allow once / Deny in Ask first and in Allow all, before and after untrusted
/// content, native or recovered from text, after "Allow for this chat" was given for
/// exactly this scope.
#[cfg(unix)]
#[tokio::test]
async fn it_always_asks_in_ask_first_and_allow_all_and_after_a_grant() {
    let bin = temp("ask-bin");
    let work = temp("ask-work");
    fake(&bin, "claude", CLAUDE_OK);
    fake(&bin, "codex", CODEX_OK);
    let s = server_for(&bin, base_env());
    for (tool, may_edit) in [
        (CLAUDE_TOOL, false),
        (CLAUDE_TOOL, true),
        (CODEX_TOOL, false),
        (CODEX_TOOL, true),
    ] {
        let action = describe(
            &s,
            tool,
            json!({"task": "x", "may_edit": may_edit}),
            &chat(&work),
        )
        .await;
        let granted: BTreeSet<policy::Scope> = [action.scope.clone()].into();
        for allow_all in [false, true] {
            for untrusted in [false, true] {
                for recovered in [false, true] {
                    let turn = policy::TurnContext {
                        tool_enabled: true,
                        server_ready: true,
                        untrusted_in_turn: untrusted,
                        recovered_from_text: recovered,
                        allow_all,
                        full_access: false,
                    };
                    for grants in [&BTreeSet::new(), &granted] {
                        assert!(
                            matches!(
                                policy::evaluate(&action, &turn, grants),
                                policy::Decision::Ask {
                                    card: policy::CardKind::OnceOnly,
                                    ..
                                }
                            ),
                            "{tool} {may_edit} allow_all {allow_all} untrusted {untrusted} recovered {recovered}"
                        );
                    }
                    // Full access (0.0.5): nothing asks, a task for a coding app neither
                    // (D11 as amended for 0.0.6: the terminal could start the app anyway).
                    let full = policy::TurnContext {
                        full_access: true,
                        ..turn
                    };
                    assert_eq!(
                        policy::evaluate(&action, &full, &BTreeSet::new()),
                        policy::Decision::AllowedByFullAccess,
                        "{tool} {may_edit} in Full access"
                    );
                }
            }
        }
    }
}

/// Full access (D11 as amended for 0.0.6, D12): nothing asks and the call runs, with the
/// same template and the task on standard input. The chat's tool context then lists the
/// whole computer as approved; that must not make every program "inside an approved
/// folder", and the card is described as in the other modes (it is never shown).
#[cfg(unix)]
#[tokio::test]
async fn full_access_runs_without_a_card() {
    let bin = temp("full-bin");
    let work = temp("full-work");
    let home = temp("full-home");
    fake(&bin, "claude", CLAUDE_OK);
    fake(&bin, "codex", CODEX_OK);
    let own = vec![work.display().to_string()];
    let s =
        server_for(&bin, base_env()).with_approved(approved_store(own.clone(), Some(own.clone())));
    let wide = in_full_access(chat(&work), &home);
    assert!(wide.full_access);
    assert!(
        wide.approved_folders.iter().any(|f| f == "/"),
        "{:?}",
        wide.approved_folders
    );
    let task = "-p --yolo \"quoted\" 'single' 100% $HOME `id`\nsecond line";
    for (tool, may_edit, app, answer) in [
        (
            CLAUDE_TOOL,
            false,
            CodingApp::ClaudeCode,
            "FAKE-CLAUDE-ANSWER: the word is heron",
        ),
        (
            CLAUDE_TOOL,
            true,
            CodingApp::ClaudeCode,
            "FAKE-CLAUDE-ANSWER: the word is heron",
        ),
        (CODEX_TOOL, false, CodingApp::Codex, "FAKE-CODEX-ANSWER"),
        (CODEX_TOOL, true, CodingApp::Codex, "FAKE-CODEX-ANSWER"),
    ] {
        let action = describe(&s, tool, json!({"task": task, "may_edit": may_edit}), &wide).await;
        assert_eq!(action.hard_limit, None, "{tool} {may_edit}");
        // The same action as outside Full access: the same card text, scope and facts.
        let narrow = describe(
            &s,
            tool,
            json!({"task": task, "may_edit": may_edit}),
            &chat(&work),
        )
        .await;
        assert_eq!(action.preview, narrow.preview);
        assert_eq!(action.scope, narrow.scope);
        assert_eq!(action.risk, policy::Risk::AlwaysAsk);
        assert!(action.keep_asking.is_some() && action.network && action.outbound);
        let granted: BTreeSet<policy::Scope> = [action.scope.clone()].into();
        for allow_all in [false, true] {
            for untrusted in [false, true] {
                for recovered in [false, true] {
                    for grants in [&BTreeSet::new(), &granted] {
                        assert_eq!(
                            policy::evaluate(
                                &action,
                                &turn_in(allow_all, true, untrusted, recovered),
                                grants
                            ),
                            policy::Decision::AllowedByFullAccess,
                            "{tool} {may_edit} allow_all {allow_all} untrusted {untrusted} recovered {recovered}"
                        );
                    }
                }
            }
        }
        let _ = std::fs::remove_file(bin.join("args.txt"));
        let (out, _) = call(&s, &action, &wide).await;
        assert!(!out.is_error, "{tool} {may_edit}: {out:?}");
        assert_eq!(out.joined_text(), answer);
        let recorded: Vec<String> = std::fs::read_to_string(bin.join("args.txt"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(recorded, arguments(app, may_edit), "{tool} {may_edit}");
        assert_eq!(
            std::fs::read_to_string(bin.join("stdin.txt")).unwrap(),
            task
        );
        assert_eq!(
            std::fs::read_to_string(bin.join("cwd.txt")).unwrap().trim(),
            work.display().to_string()
        );
    }
}

/// Full access keeps steps 1 and 2 (D11): the group off, a program inside a folder the user
/// approved, a swapped file, a working folder the user did not approve, and approved
/// folders that cannot be read each stop the call before anything runs.
#[cfg(unix)]
#[tokio::test]
async fn full_access_keeps_the_hard_limits() {
    let bin = temp("fullhl-bin");
    let work = temp("fullhl-work");
    let home = temp("fullhl-home");
    fake(&bin, "claude", CLAUDE_OK);
    let own = vec![work.display().to_string()];
    let store = || approved_store(own.clone(), Some(own.clone()));
    let refused = |action: &policy::Action, needle: &str| {
        let why = action.hard_limit.clone().unwrap_or_default();
        assert!(why.contains(needle), "{needle:?} not in {why:?}");
        assert!(
            matches!(
                policy::evaluate(
                    action,
                    &turn_in(false, true, false, false),
                    &BTreeSet::new()
                ),
                policy::Decision::Denied { .. }
            ),
            "{needle}"
        );
    };

    // 1. The group off in the chat: refused by the engine, in Full access too.
    let s = server_for(&bin, base_env()).with_approved(store());
    let wide = in_full_access(chat(&work), &home);
    let action = describe(&s, CLAUDE_TOOL, json!({"task": "x"}), &wide).await;
    let off = policy::TurnContext {
        tool_enabled: false,
        ..turn_in(false, true, false, false)
    };
    assert!(matches!(
        policy::evaluate(&action, &off, &BTreeSet::new()),
        policy::Decision::Refused { .. }
    ));

    // 2. A program inside a folder the user approved (here: for this chat).
    let project = temp("fullhl-project");
    let inside = project.join("tools");
    std::fs::create_dir_all(&inside).unwrap();
    fake(&inside, "claude", CLAUDE_OK);
    let mine = vec![project.display().to_string()];
    let s2 = server_for(&inside, base_env())
        .with_approved(approved_store(mine.clone(), Some(mine.clone())));
    let action = describe(
        &s2,
        CLAUDE_TOOL,
        json!({"task": "x"}),
        &in_full_access(chat(&project), &home),
    )
    .await;
    refused(&action, "inside a folder you approved");
    assert!(!inside.join("args.txt").exists(), "it ran");

    // 3. A working folder the user did not approve: a chat with no folder of its own works
    //    in the home folder in Full access, which nobody approved.
    let none = ChatContext {
        approved_folders: Vec::new(),
        working_folder: None,
        ..chat(&work)
    };
    let s3 =
        server_for(&bin, base_env()).with_approved(approved_store(Vec::new(), Some(Vec::new())));
    let wide_none = in_full_access(none, &home);
    assert_eq!(
        wide_none.working_folder.as_deref(),
        Some(home.display().to_string().as_str())
    );
    let action = describe(&s3, CLAUDE_TOOL, json!({"task": "x"}), &wide_none).await;
    refused(&action, "No folder is approved for this chat");
    //    And a working folder outside the chat's own folders.
    let elsewhere = temp("fullhl-elsewhere");
    let moved = ChatContext {
        working_folder: Some(elsewhere.display().to_string()),
        ..in_full_access(chat(&work), &home)
    };
    let action = describe(&s, CLAUDE_TOOL, json!({"task": "x"}), &moved).await;
    refused(&action, "is not approved");

    // 4. A swapped file: described, then replaced before the run.
    let action = describe(&s, CLAUDE_TOOL, json!({"task": "x"}), &wide).await;
    assert_eq!(action.hard_limit, None);
    // The old file stays alive under another name, so its inode cannot be reused.
    let body = std::fs::read(bin.join("claude")).unwrap();
    std::fs::rename(bin.join("claude"), bin.join("claude-old")).unwrap();
    std::fs::write(bin.join("claude"), body).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    let (out, _) = call(&s, &action, &wide).await;
    assert!(
        out.is_error && out.joined_text().contains("Nothing was run"),
        "{out:?}"
    );
    assert!(!bin.join("args.txt").exists(), "it ran");
    refused(
        &describe(&s, CLAUDE_TOOL, json!({"task": "x"}), &wide).await,
        "changed on this computer",
    );

    // 5. Approved folders that cannot be read: refused rather than taken from the widened
    //    context.
    let s5 = server_for(&inside, base_env());
    let action = describe(
        &s5,
        CLAUDE_TOOL,
        json!({"task": "x"}),
        &in_full_access(chat(&work), &home),
    )
    .await;
    refused(&action, "could not be read");
    let s5 = server_for(&inside, base_env()).with_approved(approved_store(own.clone(), None));
    let action = describe(
        &s5,
        CLAUDE_TOOL,
        json!({"task": "x"}),
        &in_full_access(chat(&work), &home),
    )
    .await;
    refused(&action, "could not be read");
}

/// The card says when the app is set up to use the model this app shares (WP5).
#[cfg(unix)]
#[tokio::test]
async fn the_card_says_when_the_app_uses_the_shared_model() {
    let mut env = base_env();
    env.insert("ANTHROPIC_BASE_URL".into(), "http://127.0.0.1:51234".into());
    assert!(uses_shared_model(
        CodingApp::ClaudeCode,
        &env,
        None,
        Some(51234)
    ));
    assert!(!uses_shared_model(
        CodingApp::ClaudeCode,
        &env,
        None,
        Some(51235)
    ));
    assert!(!uses_shared_model(CodingApp::ClaudeCode, &env, None, None));
    env.insert(
        "ANTHROPIC_BASE_URL".into(),
        "https://api.example.com:51234".into(),
    );
    assert!(!uses_shared_model(
        CodingApp::ClaudeCode,
        &env,
        None,
        Some(51234)
    ));
    // Codex: the provider its config.toml chooses, under CODEX_HOME or ~/.codex.
    let home = temp("codex-home");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(
        home.join(".codex/config.toml"),
        "model_provider = \"harnessui\"\n\n[model_providers.harnessui]\nname = \"harnessUI\"\nbase_url = \"http://localhost:51234/v1\"\nwire_api = \"responses\"\n",
    )
    .unwrap();
    assert!(uses_shared_model(
        CodingApp::Codex,
        &base_env(),
        Some(&home),
        Some(51234)
    ));
    assert!(!uses_shared_model(
        CodingApp::Codex,
        &base_env(),
        Some(&home),
        Some(4000)
    ));
    // A provider block that is not the chosen one does not count.
    std::fs::write(
        home.join(".codex/config.toml"),
        "model_provider = \"openai\"\n[model_providers.harnessui]\nbase_url = \"http://localhost:51234/v1\"\n",
    )
    .unwrap();
    assert!(!uses_shared_model(
        CodingApp::Codex,
        &base_env(),
        Some(&home),
        Some(51234)
    ));
    let mut codex_env = base_env();
    codex_env.insert("OPENAI_BASE_URL".into(), "http://127.0.0.1:51234/v1".into());
    assert!(uses_shared_model(
        CodingApp::Codex,
        &codex_env,
        None,
        Some(51234)
    ));

    let bin = temp("share-bin");
    let work = temp("share-work");
    fake(&bin, "claude", CLAUDE_OK);
    let mut shared = base_env();
    shared.insert("ANTHROPIC_BASE_URL".into(), "http://127.0.0.1:51234".into());
    let mut s = server_for(&bin, shared);
    s.share_port = Arc::new(|| Box::pin(async { Some(51234) }));
    let a = describe(&s, CLAUDE_TOOL, json!({"task": "x"}), &chat(&work)).await;
    assert!(
        a.preview
            .contains(&online_line(CodingApp::ClaudeCode, true)),
        "{}",
        a.preview
    );
    assert!(!a.preview.contains("runs online"), "{}", a.preview);
}

// ---- Runs of fake programs --------------------------------------------------------------

/// The fake runs with exactly the template, in the working folder, with the task on
/// standard input byte for byte (quotes, `%`, line breaks, a leading `-`), and its answer
/// comes back; the card got progress lines.
#[cfg(unix)]
#[tokio::test]
async fn the_task_goes_on_standard_input_and_the_answer_comes_back() {
    let bin = temp("run-bin");
    let work = temp("run-work");
    fake(&bin, "claude", CLAUDE_OK);
    fake(&bin, "codex", CODEX_OK);
    let s = server_for(&bin, base_env());
    let task = "-p --dangerously-skip-permissions \"quoted\" 'single' %PATH% $HOME `id`\nsecond line\r\n\tthird";
    for (tool, may_edit, app, answer) in [
        (
            CLAUDE_TOOL,
            false,
            CodingApp::ClaudeCode,
            "FAKE-CLAUDE-ANSWER: the word is heron",
        ),
        (
            CLAUDE_TOOL,
            true,
            CodingApp::ClaudeCode,
            "FAKE-CLAUDE-ANSWER: the word is heron",
        ),
        (CODEX_TOOL, false, CodingApp::Codex, "FAKE-CODEX-ANSWER"),
        (CODEX_TOOL, true, CodingApp::Codex, "FAKE-CODEX-ANSWER"),
    ] {
        let _ = std::fs::remove_file(bin.join("args.txt"));
        let action = describe(
            &s,
            tool,
            json!({"task": task, "may_edit": may_edit}),
            &chat(&work),
        )
        .await;
        let (out, progress) = call(&s, &action, &chat(&work)).await;
        assert!(!out.is_error, "{tool}: {out:?}");
        assert_eq!(out.joined_text(), answer);
        let recorded: Vec<String> = std::fs::read_to_string(bin.join("args.txt"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(recorded, arguments(app, may_edit), "{tool} {may_edit}");
        assert_eq!(
            std::fs::read_to_string(bin.join("stdin.txt")).unwrap(),
            task
        );
        assert_eq!(
            std::fs::read_to_string(bin.join("cwd.txt")).unwrap().trim(),
            work.display().to_string()
        );
        assert!(
            progress.contains(&format!("{} is working… 0 min · Stop", app.label())),
            "{progress}"
        );
        match app {
            CodingApp::ClaudeCode => assert!(progress.contains("Read notes.txt"), "{progress}"),
            CodingApp::Codex => assert!(progress.contains("Runs: bash -lc ls"), "{progress}"),
        }
    }
}

/// Secret-looking variables are not passed on; the others are.
#[cfg(unix)]
#[tokio::test]
async fn secret_variables_stay_stripped() {
    let bin = temp("env-bin");
    let work = temp("env-work");
    fake(&bin, "claude", CLAUDE_OK);
    let mut env = base_env();
    for (k, v) in [
        ("ANTHROPIC_API_KEY", "sk-secret-1"),
        ("OPENAI_API_KEY", "sk-secret-2"),
        ("CODEX_API_KEY", "sk-secret-3"),
        ("GITHUB_TOKEN", "ghp-secret"),
        ("MY_PASSWORD", "hunter2"),
        ("PLAIN_SETTING", "kept-value"),
    ] {
        env.insert(k.into(), v.into());
    }
    let s = server_for(&bin, env);
    let action = describe(&s, CLAUDE_TOOL, json!({"task": "x"}), &chat(&work)).await;
    let (out, _) = call(&s, &action, &chat(&work)).await;
    assert!(!out.is_error, "{out:?}");
    let seen = std::fs::read_to_string(bin.join("env.txt")).unwrap();
    assert!(seen.contains("PLAIN_SETTING=kept-value"), "{seen}");
    for secret in ["sk-secret", "ghp-secret", "hunter2"] {
        assert!(!seen.contains(secret), "{secret} reached the app");
    }
}

/// Stop ends the program at once; what it had written so far comes back.
#[cfg(unix)]
#[tokio::test]
async fn stop_ends_the_run_at_once() {
    let bin = temp("stop-bin");
    let work = temp("stop-work");
    let marker = bin.join("finished");
    script(
        &bin,
        "claude",
        &format!(
            "#!/bin/sh\ncat > /dev/null\nprintf '%s\\n' '{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"halfway there\"}}]}}}}'\nsleep 30\ntouch {}\n",
            marker.display()
        ),
    );
    let s = server_for(&bin, base_env());
    let action = describe(&s, CLAUDE_TOOL, json!({"task": "x"}), &chat(&work)).await;
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        stopper.cancel();
    });
    let started = std::time::Instant::now();
    let out = s
        .call(
            CLAUDE_TOOL,
            &Map::new(),
            &action,
            CallContext {
                chat: chat(&work),
                cancel,
                progress: None,
            },
        )
        .await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(out.is_error);
    let text = out.joined_text();
    assert!(
        text.contains("Claude Code was stopped before it finished"),
        "{text}"
    );
    assert!(text.contains("halfway there"), "{text}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!marker.exists());
}

/// A run that takes longer than allowed is ended.
#[cfg(unix)]
#[tokio::test]
async fn a_run_that_takes_too_long_is_ended() {
    let bin = temp("slow-bin");
    let work = temp("slow-work");
    script(&bin, "codex", "#!/bin/sh\ncat > /dev/null\nsleep 30\n");
    let mut s = server_for(&bin, base_env());
    s.max_run = Duration::from_millis(500);
    let action = describe(&s, CODEX_TOOL, json!({"task": "x"}), &chat(&work)).await;
    let started = std::time::Instant::now();
    let (out, _) = call(&s, &action, &chat(&work)).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        out.is_error && out.joined_text().contains("the longest a task may run"),
        "{out:?}"
    );
}

/// Not signed in: the plain message, also when the app only says so in a retry event
/// (the run then ends at once instead of retrying).
#[cfg(unix)]
#[tokio::test]
async fn not_signed_in_says_what_to_do() {
    let bin = temp("auth-bin");
    let work = temp("auth-work");
    fake(
        &bin,
        "claude",
        &[
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#,
        ],
    );
    script(
        &bin,
        "codex",
        "#!/bin/sh\ncat > /dev/null\necho 'Error: Not logged in. Run codex login.' >&2\nexit 1\n",
    );
    let s = server_for(&bin, base_env());
    for tool in [CLAUDE_TOOL, CODEX_TOOL] {
        let action = describe(&s, tool, json!({"task": "x"}), &chat(&work)).await;
        let (out, _) = call(&s, &action, &chat(&work)).await;
        let label = CodingApp::from_tool(tool).unwrap().label();
        assert!(out.is_error);
        assert_eq!(
            out.joined_text(),
            format!("{label} is not signed in. Open it once and sign in, then try again.")
        );
    }
    let retrying = temp("auth-retry");
    script(
        &retrying,
        "claude",
        "#!/bin/sh\ncat > /dev/null\necho '{\"type\":\"system\",\"subtype\":\"api_retry\",\"attempt\":1,\"error\":\"authentication_failed\"}'\nsleep 30\n",
    );
    let s = server_for(&retrying, base_env());
    let action = describe(&s, CLAUDE_TOOL, json!({"task": "x"}), &chat(&work)).await;
    let started = std::time::Instant::now();
    let (out, _) = call(&s, &action, &chat(&work)).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(out.joined_text().contains("is not signed in"), "{out:?}");
}

/// A long answer is shortened like any other long result, the whole text kept.
#[cfg(unix)]
#[tokio::test]
async fn a_long_answer_is_shortened() {
    let bin = temp("long-bin");
    let work = temp("long-work");
    let answer: String = (0..4000).map(|i| format!("line {i}\\n")).collect();
    let line = format!(r#"{{"type":"result","is_error":false,"result":"{answer}"}}"#);
    fake(&bin, "claude", &[&line]);
    let s = server_for(&bin, base_env());
    let mut c = chat(&work);
    c.result_cap_tokens = 500;
    let action = describe(&s, CLAUDE_TOOL, json!({"task": "x"}), &c).await;
    let (out, _) = call(&s, &action, &c).await;
    assert!(!out.is_error);
    let full = out.full_text.clone().expect("the whole text kept");
    assert!(
        full.contains("line 0\n") && full.contains("line 3999"),
        "{}",
        &full[..100]
    );
    assert!(out.joined_text().len() < full.len() / 4);
}

// ---- Reading the output -----------------------------------------------------------------

#[test]
fn lines_are_split_and_capped() {
    let mut lines = Lines::default();
    assert_eq!(lines.push(b"ab"), Vec::<String>::new());
    assert_eq!(lines.push(b"c\nde\n"), ["abc", "de"]);
    assert_eq!(lines.push(b"tail"), Vec::<String>::new());
    assert_eq!(lines.finish().as_deref(), Some("tail"));
    // A line past the cap is dropped whole; the next one is read.
    let mut lines = Lines::default();
    let big = vec![b'x'; 5 * 1024 * 1024];
    assert!(lines.push(&big).is_empty());
    assert_eq!(lines.push(b"\nnext\n"), ["next"]);
}

#[test]
fn results_say_what_happened() {
    let budget = 100_000;
    let claude = |lines: &[&str]| {
        let mut p = Parsed::new(CodingApp::ClaudeCode);
        for l in lines {
            p.line(l);
        }
        p
    };
    let codex = |lines: &[&str]| {
        let mut p = Parsed::new(CodingApp::Codex);
        for l in lines {
            p.line(l);
        }
        p
    };
    let ok = result(
        CodingApp::ClaudeCode,
        &End::Exited(Some(0)),
        &claude(CLAUDE_OK),
        "",
        budget,
    );
    assert_eq!(ok.joined_text(), "FAKE-CLAUDE-ANSWER: the word is heron");
    let ok = result(
        CodingApp::Codex,
        &End::Exited(Some(0)),
        &codex(CODEX_OK),
        "",
        budget,
    );
    assert_eq!(ok.joined_text(), "FAKE-CODEX-ANSWER");
    // Not JSON: ignored.
    assert_eq!(claude(&["not json", ""]).answer, None);
    // An unknown flag: update the app.
    let old = result(
        CodingApp::ClaudeCode,
        &End::Exited(Some(1)),
        &claude(&[]),
        "error: unknown option '--restricted'\n",
        budget,
    );
    assert!(
        old.is_error
            && old
                .joined_text()
                .contains("does not know a setting the app needs"),
        "{old:?}"
    );
    assert!(old.joined_text().contains("--restricted"));
    // Codex outside a Git repository.
    let git = result(
        CodingApp::Codex,
        &End::Exited(Some(1)),
        &codex(&[]),
        "Not inside a trusted directory and --skip-git-repo-check was not specified.",
        budget,
    );
    assert!(git.joined_text().contains("Git repository"), "{git:?}");
    // Codex reports a failed turn.
    let failed = result(
        CodingApp::Codex,
        &End::Exited(Some(1)),
        &codex(&[r#"{"type":"turn.failed","error":{"message":"the model is overloaded"}}"#]),
        "",
        budget,
    );
    assert_eq!(
        failed.joined_text(),
        "Codex reported a problem: the model is overloaded"
    );
    // Claude Code's result marked as an error.
    let err = result(
        CodingApp::ClaudeCode,
        &End::Exited(Some(1)),
        &claude(&[r#"{"type":"result","is_error":true,"result":"Prompt is too long"}"#]),
        "",
        budget,
    );
    assert_eq!(
        err.joined_text(),
        "Claude Code reported a problem: Prompt is too long"
    );
    // Nothing came back.
    let empty = result(
        CodingApp::Codex,
        &End::Exited(Some(2)),
        &codex(&[]),
        "boom\n",
        budget,
    );
    assert_eq!(
        empty.joined_text(),
        "Codex finished without an answer (exit code 2).\n\nWhat it printed last:\nboom"
    );
}
