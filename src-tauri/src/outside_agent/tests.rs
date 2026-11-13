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

fn no_share() -> ReadShare {
    share_is(ShareNow::default())
}

/// Whether the app would send a task to the model shared on `port` rather than online.
fn uses_shared_model(
    app: CodingApp,
    env: &BTreeMap<String, String>,
    home: Option<&Path>,
    port: Option<u16>,
) -> bool {
    shared_setup(app, env, home, port).is_some()
}

/// The sharing key Sharing holds in these tests.
const SHARING_KEY: &str = "harnessui-share-0123456789abcdef0123456789abcdef";

/// Sharing as `Sharing::now` says it.
fn share_is(now: ShareNow) -> ReadShare {
    Arc::new(move || {
        let now = now.clone();
        Box::pin(async move { now })
    })
}

/// Sharing switched on and listening on `port`.
fn live(port: u16) -> ShareNow {
    ShareNow {
        port: Some(port),
        enabled: true,
        listening: Some(port),
        key: Some(SHARING_KEY.into()),
    }
}

/// A Codex settings folder whose `config.toml` chooses a provider at `base_url` that reads
/// its key from `env_key`, as the Other apps setup card writes it.
fn codex_home(name: &str, base_url: &str, env_key: &str) -> PathBuf {
    let dir = temp(name);
    std::fs::write(
        dir.join("config.toml"),
        format!(
            "model_provider = \"harnessui\"\nmodel = \"harnessui-local\"\nmodel_providers.harnessui = {{ name = \"harnessUI\", base_url = \"{base_url}\", env_key = \"{env_key}\", wire_api = \"responses\" }}\n"
        ),
    )
    .unwrap();
    dir
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

/// A file the search takes for a program, never run: executable on Unix; on Windows any
/// file whose extension is in `PATHEXT` (here `.exe` and `.cmd`). Run, it would leave
/// `<name>.was-run` beside itself.
#[cfg(unix)]
fn program_file(dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    script(dir, name, "#!/bin/sh\ntouch \"$0.was-run\"\n")
}

#[cfg(windows)]
fn program_file(dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, "@type nul > \"%~f0.was-run\"\r\n").unwrap();
    path
}

/// The files in `dirs` that say a program was run ([`program_file`]).
fn runs_left(dirs: &[&Path]) -> Vec<PathBuf> {
    dirs.iter()
        .flat_map(|d| std::fs::read_dir(d).unwrap())
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".was-run"))
        .collect()
}

/// [0.0.6 release review] `locate` as it runs on Windows (`windows: true`), on every
/// system, so the order the owner's PC gets is tested everywhere: `<name>.exe` anywhere on
/// `PATH` comes before the shim's name earlier on `PATH` (on Windows `PATHEXT` makes that
/// name npm's `<name>.cmd`; elsewhere the search sees the file of that name), `PATH` comes
/// before the user's bin, and there only `<home>/.local/bin/<name>.exe` is looked for,
/// where Claude Code's installer puts it on Windows. Nothing is run.
#[test]
fn locate_as_on_windows_takes_the_exe_then_the_shim_then_the_users_bin() {
    let shim_name = |app: CodingApp| {
        if cfg!(windows) {
            format!("{}.cmd", app.program())
        } else {
            app.program().to_owned()
        }
    };
    let shims = temp("win-shims");
    let exes = temp("win-exes");
    let empty = temp("win-empty");
    let home = temp("win-home");
    let bin = home.join(".local").join("bin");
    let both = std::env::join_paths([&shims, &exes]).unwrap();
    let reversed = std::env::join_paths([&exes, &shims]).unwrap();
    for app in CodingApp::ALL {
        let name = app.program();
        let exe_name = format!("{name}.exe");
        let shim = program_file(&shims, &shim_name(app));
        let exe = program_file(&exes, &exe_name);
        // The .exe wherever it is on PATH, also after the shim.
        for path in [&both, &reversed] {
            assert_eq!(
                locate(app, Some(path), Some(&home), true),
                Some(exe.clone()),
                "{name} {path:?}"
            );
        }
        // No .exe on PATH: the shim.
        assert_eq!(
            locate(app, Some(shims.as_os_str()), Some(&home), true),
            Some(shim.clone()),
            "{name}"
        );
        // PATH before the user's bin.
        let in_bin = program_file(&bin, &exe_name);
        assert_eq!(
            locate(app, Some(shims.as_os_str()), Some(&home), true),
            Some(shim.clone()),
            "{name}"
        );
        // Not on PATH (or no PATH, or an empty one): <home>/.local/bin/<name>.exe.
        for path in [Some(empty.as_os_str()), Some(OsStr::new("")), None] {
            assert_eq!(
                locate(app, path, Some(&home), true),
                Some(in_bin.clone()),
                "{name} {path:?}"
            );
        }
        assert_eq!(locate(app, Some(empty.as_os_str()), None, true), None);
        // Only the .exe is looked for there, not the shim or the plain name.
        std::fs::remove_file(&in_bin).unwrap();
        program_file(&bin, name);
        program_file(&bin, &shim_name(app));
        assert_eq!(
            locate(app, Some(empty.as_os_str()), Some(&home), true),
            None,
            "{name}"
        );
        // The Unix rules, for contrast: the first on PATH, then <home>/.local/bin/<name>.
        if cfg!(unix) {
            assert_eq!(
                locate(app, Some(&both), Some(&home), false),
                Some(shim.clone())
            );
            assert_eq!(
                locate(app, Some(empty.as_os_str()), Some(&home), false),
                Some(bin.join(name))
            );
        }
    }
    // A relative PATH entry (the current folder) is never searched.
    let rel = std::ffi::OsString::from(".");
    assert_eq!(locate(CodingApp::ClaudeCode, Some(&rel), None, true), None);
    assert_eq!(runs_left(&[&shims, &exes, &bin]), Vec::<PathBuf>::new());
}

/// [0.0.6 release review] Windows, with the files laid out as npm and Claude Code's
/// installer lay them out: npm's folder holds `claude` (a shell script), `claude.cmd`,
/// `claude.ps1` and the package under `node_modules`; the installer puts `claude.exe` in
/// `<home>\.local\bin`. A `claude.exe` on `PATH` is used before the shim earlier on
/// `PATH`; without one, the `.cmd` shim (never the script beside it), with the version
/// from its package; with neither on `PATH`, `<home>\.local\bin\claude.exe`. The same for
/// Codex. A shim inside an approved folder is refused. Nothing is run.
#[cfg(windows)]
#[test]
fn on_windows_the_exe_then_the_npm_shim_then_the_users_bin_are_used() {
    fn text(p: &Path) -> String {
        p.display().to_string()
    }
    let npm = temp("win-npm");
    let other = temp("win-other");
    let empty = temp("win-none");
    let home = temp("win-profile");
    let bin = home.join(".local").join("bin");
    for (app, version) in [
        (CodingApp::ClaudeCode, "2.1.322"),
        (CodingApp::Codex, "0.130.0"),
    ] {
        let name = app.program();
        std::fs::write(
            npm.join(name),
            "#!/bin/sh\nbasedir=$(dirname \"$0\")\nexec node \"$basedir/node_modules/x/cli.js\" \"$@\"\n",
        )
        .unwrap();
        let shim = program_file(&npm, &format!("{name}.cmd"));
        std::fs::write(npm.join(format!("{name}.ps1")), "#!/usr/bin/env pwsh\r\n").unwrap();
        let mut package = npm.join("node_modules");
        for part in app.package().split('/') {
            package = package.join(part);
        }
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("package.json"),
            format!(r#"{{"name":"{}","version":"{version}"}}"#, app.package()),
        )
        .unwrap();
        let exe = program_file(&other, &format!("{name}.exe"));
        let in_bin = program_file(&bin, &format!("{name}.exe"));

        let both = std::env::join_paths([&npm, &other]).unwrap();
        let found = find_app(app, Some(&both), Some(&home), &[]);
        assert_eq!(found.path, Some(text(&exe)), "{name}");
        assert_eq!(found.program, Some(text(&exe)), "{name}");
        assert!(found.usable(), "{name}: {found:?}");

        let found = find_app(app, Some(npm.as_os_str()), Some(&home), &[]);
        assert_eq!(found.path, Some(text(&shim)), "{name}");
        assert_eq!(found.program, Some(text(&shim)), "{name}");
        assert_eq!(found.version.as_deref(), Some(version), "{name}");
        assert!(found.usable(), "{name}: {found:?}");

        let found = find_app(app, Some(empty.as_os_str()), Some(&home), &[]);
        assert_eq!(found.path, Some(text(&in_bin)), "{name}");
        assert!(found.usable(), "{name}: {found:?}");

        let found = find_app(app, Some(npm.as_os_str()), Some(&home), &[text(&npm)]);
        assert!(
            matches!(
                found.problem,
                Some(Problem::InsideApprovedFolder {
                    holds_home: false,
                    ..
                })
            ),
            "{name}: {found:?}"
        );
        assert!(!found.usable());

        let found = find_app(app, Some(empty.as_os_str()), None, &[]);
        assert_eq!(found.problem, Some(Problem::NotFound), "{name}");
    }
    assert_eq!(runs_left(&[&npm, &other, &bin]), Vec::<PathBuf>::new());
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

/// A folder link inside an approved folder counts where it stands too: the `PATH` folder
/// itself, or a folder above it, can be a link the project controls that leads outside
/// (review of 0.0.6, round 2).
#[cfg(unix)]
#[tokio::test]
async fn a_folder_link_inside_an_approved_folder_is_refused_wherever_it_leads() {
    let project = temp("dirlink-project");
    let approved = vec![project.display().to_string()];
    let inside = |found: &SavedApp| matches!(&found.problem, Some(Problem::InsideApprovedFolder { folder, .. }) if *folder == project.display().to_string());
    // The PATH folder is the link: project/bin -> outside, which holds `codex`.
    let outside = temp("dirlink-outside");
    fake(&outside, "codex", CODEX_OK);
    let bin = project.join("bin");
    std::os::unix::fs::symlink(&outside, &bin).unwrap();
    let found = find_app(CodingApp::Codex, Some(bin.as_os_str()), None, &approved);
    assert!(inside(&found), "{found:?}");
    assert!(!found.usable());
    // A folder above the PATH folder is the link: project/tools -> outside2, PATH entry
    // project/tools/bin.
    let outside2 = temp("dirlink-outside2");
    std::fs::create_dir_all(outside2.join("bin")).unwrap();
    fake(&outside2.join("bin"), "codex", CODEX_OK);
    std::os::unix::fs::symlink(&outside2, project.join("tools")).unwrap();
    let tools_bin = project.join("tools").join("bin");
    let found = find_app(
        CodingApp::Codex,
        Some(tools_bin.as_os_str()),
        None,
        &approved,
    );
    assert!(inside(&found), "{found:?}");
    // A link with a relative target counts the same way: project/rel -> ../<outside2>.
    let relative = Path::new("..").join(outside2.file_name().unwrap());
    std::os::unix::fs::symlink(&relative, project.join("rel")).unwrap();
    let rel_bin = project.join("rel").join("bin");
    let found = find_app(CodingApp::Codex, Some(rel_bin.as_os_str()), None, &approved);
    assert!(inside(&found), "{found:?}");
    // Found while the folder was not approved; the chat approves it later: refused before
    // the run, in every mode.
    for path in [&bin, &tools_bin] {
        let s = server_for(path, base_env());
        assert_eq!(s.tools().len(), 1);
        let action = describe(&s, CODEX_TOOL, json!({"task": "hello"}), &chat(&project)).await;
        let why = action
            .hard_limit
            .clone()
            .filter(|w| w.contains("inside a folder you approved"))
            .unwrap_or_else(|| panic!("not refused: {:?}", action.hard_limit));
        for (allow_all, full_access) in [(false, false), (true, false), (true, true)] {
            assert_eq!(
                policy::evaluate(
                    &action,
                    &turn_in(allow_all, full_access, false, false),
                    &BTreeSet::new()
                ),
                policy::Decision::Denied {
                    reason: why.clone()
                }
            );
        }
    }
    // The same program reached by a path outside the approved folder is accepted.
    let found = find_app(CodingApp::Codex, Some(outside.as_os_str()), None, &approved);
    assert_eq!(found.problem, None, "{found:?}");
    assert!(!outside.join("args.txt").exists(), "it ran");
    assert!(!outside2.join("bin").join("args.txt").exists(), "it ran");
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
    s.share = share_is(live(51234));
    let a = describe(&s, CLAUDE_TOOL, json!({"task": "x"}), &chat(&work)).await;
    assert!(
        a.preview
            .contains(&online_line(CodingApp::ClaudeCode, true)),
        "{}",
        a.preview
    );
    assert!(!a.preview.contains("runs online"), "{}", a.preview);
}

/// An app set up for the model this app shares is not asked while sharing is off, or on but
/// not listening (the port taken): the card would promise the local model while the app
/// connects to a port nothing serves, or another program took. An app not set up for it is
/// asked as usual, and the card says it runs online.
#[cfg(unix)]
#[tokio::test]
async fn with_sharing_off_an_app_set_up_for_it_is_not_asked() {
    let bin = temp("off-bin");
    let work = temp("off-work");
    fake(&bin, "claude", CLAUDE_OK);
    fake(&bin, "codex", CODEX_OK);
    let codex = codex_home(
        "off-codex",
        "http://127.0.0.1:51234/v1",
        "HARNESSUI_SHARE_KEY",
    );
    let mut env = base_env();
    env.insert("ANTHROPIC_BASE_URL".into(), "http://127.0.0.1:51234".into());
    env.insert("CODEX_HOME".into(), codex.display().to_string());
    let off = ShareNow {
        port: Some(51234),
        enabled: false,
        listening: None,
        key: None,
    };
    let not_listening = ShareNow {
        enabled: true,
        ..off.clone()
    };
    // On, but listening on another port than the one the apps are set up for.
    let elsewhere = ShareNow {
        port: Some(51234),
        ..live(51999)
    };
    for (share, words) in [
        (off, "and sharing is off, so"),
        (not_listening.clone(), "but sharing is not working now, so"),
        (elsewhere, "but sharing is not working now, so"),
    ] {
        let mut s = server_for(&bin, env.clone());
        s.share = share_is(share.clone());
        for (tool, app) in [
            (CLAUDE_TOOL, CodingApp::ClaudeCode),
            (CODEX_TOOL, CodingApp::Codex),
        ] {
            let a = describe(&s, tool, json!({"task": "x"}), &chat(&work)).await;
            let why = a.hard_limit.clone().unwrap_or_default();
            assert_eq!(why, sharing_off(app, share.enabled), "{share:?}");
            assert!(why.contains(words), "{why}");
            assert!(
                why.starts_with(&format!(
                    "{} is set up to use the model in harnessUI",
                    app.label()
                )),
                "{why}"
            );
            for full_access in [false, true] {
                assert!(
                    matches!(
                        policy::evaluate(
                            &a,
                            &turn_in(false, full_access, false, false),
                            &BTreeSet::new()
                        ),
                        policy::Decision::Denied { .. }
                    ),
                    "{tool} {full_access}"
                );
            }
        }
    }
    assert_eq!(
        sharing_off(CodingApp::Codex, false),
        "Codex is set up to use the model in harnessUI, and sharing is off, so Codex was not asked. The user can turn on sharing in Settings → Other apps, then ask again."
    );

    // Not set up for the shared model: asked as usual, online, whatever sharing says.
    let mut s = server_for(&bin, base_env());
    s.share = share_is(not_listening);
    let a = describe(&s, CLAUDE_TOOL, json!({"task": "x"}), &chat(&work)).await;
    assert_eq!(a.hard_limit, None);
    assert!(
        a.preview
            .contains(&online_line(CodingApp::ClaudeCode, false)),
        "{}",
        a.preview
    );

    // Sharing switched off between the card and the run: nothing runs.
    let switch = Arc::new(Mutex::new(live(51234)));
    let reader = switch.clone();
    let mut s = server_for(&bin, env.clone());
    s.share = Arc::new(move || {
        let now = reader.lock().unwrap().clone();
        Box::pin(async move { now })
    });
    for tool in [CLAUDE_TOOL, CODEX_TOOL] {
        *switch.lock().unwrap() = live(51234);
        let _ = std::fs::remove_file(bin.join("args.txt"));
        let a = describe(&s, tool, json!({"task": "x"}), &chat(&work)).await;
        assert_eq!(a.hard_limit, None, "{tool}");
        assert_eq!(a.payload["localModel"], true, "{tool}");
        switch.lock().unwrap().enabled = false;
        switch.lock().unwrap().listening = None;
        switch.lock().unwrap().key = None;
        let (out, _) = call(&s, &a, &chat(&work)).await;
        assert!(out.is_error, "{tool}: {out:?}");
        assert!(out.joined_text().contains("sharing is off"), "{out:?}");
        assert!(out.joined_text().ends_with("Nothing was run."), "{out:?}");
        assert!(!bin.join("args.txt").exists(), "{tool} ran");
    }

    // The card said online and the app was set up for the shared model before the run (or
    // the other way round): the card no longer says where the task goes, so nothing runs.
    let mut online = server_for(&bin, base_env());
    online.share = share_is(live(51234));
    let mut local = server_for(&bin, env.clone());
    local.share = share_is(live(51234));
    for (described_by, run_by) in [(&online, &local), (&local, &online)] {
        let _ = std::fs::remove_file(bin.join("args.txt"));
        let a = describe(
            described_by,
            CLAUDE_TOOL,
            json!({"task": "x"}),
            &chat(&work),
        )
        .await;
        let (out, _) = call(run_by, &a, &chat(&work)).await;
        assert!(out.is_error, "{out:?}");
        assert!(
            out.joined_text()
                .contains("changed after the request was approved"),
            "{out:?}"
        );
        assert!(!bin.join("args.txt").exists());
    }
}

/// Codex's built-in provider is the one `OPENAI_BASE_URL` moves; a `config.toml` that
/// chooses another provider sends the task there, whatever that variable says.
#[test]
fn codex_uses_openai_base_url_only_for_its_built_in_provider() {
    let mut env = base_env();
    env.insert("OPENAI_BASE_URL".into(), "http://127.0.0.1:51234/v1".into());
    let online = codex_home("codex-online", "https://api.example.com/v1", "EXAMPLE_KEY");
    env.insert("CODEX_HOME".into(), online.display().to_string());
    assert!(!uses_shared_model(
        CodingApp::Codex,
        &env,
        None,
        Some(51234)
    ));
    // Its own provider at the port: the key goes in the provider's env_key.
    let local = codex_home("codex-local", "http://localhost:51234/v1", "MY_SHARE_KEY");
    env.insert("CODEX_HOME".into(), local.display().to_string());
    assert_eq!(
        shared_setup(CodingApp::Codex, &env, None, Some(51234)),
        Some(SharedSetup {
            key_variable: Some("MY_SHARE_KEY".into())
        })
    );
    // An env_key that does not look like a secret's name could stand for a variable the
    // app keeps (PATH, LD_PRELOAD): the key is not put there.
    for name in ["PATH", "LD_PRELOAD", "HOME", "BAD NAME_KEY", "-X_KEY", ""] {
        let odd = codex_home("codex-odd", "http://127.0.0.1:51234/v1", name);
        env.insert("CODEX_HOME".into(), odd.display().to_string());
        assert_eq!(
            shared_setup(CodingApp::Codex, &env, None, Some(51234)),
            Some(SharedSetup { key_variable: None }),
            "{name}"
        );
    }
    // No config.toml: the built-in provider, moved by OPENAI_BASE_URL; it reads no key
    // this app could fill.
    env.insert(
        "CODEX_HOME".into(),
        temp("codex-empty").display().to_string(),
    );
    assert_eq!(
        shared_setup(CodingApp::Codex, &env, None, Some(51234)),
        Some(SharedSetup { key_variable: None })
    );
    // Claude Code: ANTHROPIC_AUTH_TOKEN, only when ANTHROPIC_BASE_URL is at the port.
    env.insert("ANTHROPIC_BASE_URL".into(), "http://127.0.0.1:51234".into());
    assert_eq!(
        shared_setup(CodingApp::ClaudeCode, &env, None, Some(51234)),
        Some(SharedSetup {
            key_variable: Some("ANTHROPIC_AUTH_TOKEN".into())
        })
    );
    assert_eq!(
        shared_setup(CodingApp::ClaudeCode, &env, None, Some(51235)),
        None
    );
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

/// Names the folder where this test binary, started as a fake app by a shim
/// ([`shim_to_this_binary`]), records its run.
const FAKE_RECORD_ENV: &str = "HARNESS_TEST_CODING_APP_RECORD";

/// This process's environment, as text.
fn this_env() -> BTreeMap<String, String> {
    std::env::vars_os()
        .map(|(k, v)| {
            let text = |s: std::ffi::OsString| s.to_string_lossy().into_owned();
            (text(k), text(v))
        })
        .collect()
}

/// What a shim puts before the app's command line: this test binary runs only
/// [`coding_app_fake_process`], and everything after `--` is the app's.
const FAKE_WORDS: &str =
    "outside_agent::tests::coding_app_fake_process --exact --nocapture --test-threads=1 --";

/// This test binary as a fake Claude Code or Codex, when a shim ([`shim_to_this_binary`])
/// starts it: it records the app's command line (after `--`), its standard input, working
/// folder and environment in the folder [`FAKE_RECORD_ENV`] names, then answers as the app
/// does. A plain test run does nothing.
#[test]
fn coding_app_fake_process() {
    use std::io::{Read, Write};
    let Some(record) = std::env::var_os(FAKE_RECORD_ENV).map(PathBuf::from) else {
        return;
    };
    let args: Vec<String> = std::env::args().skip_while(|a| a != "--").skip(1).collect();
    let mut stdin = Vec::new();
    std::io::stdin().read_to_end(&mut stdin).unwrap();
    let env = this_env();
    let cwd = std::env::current_dir().unwrap();
    std::fs::write(record.join("args.json"), serde_json::to_vec(&args).unwrap()).unwrap();
    std::fs::write(record.join("stdin.txt"), stdin).unwrap();
    std::fs::write(record.join("cwd.txt"), cwd.display().to_string()).unwrap();
    std::fs::write(record.join("env.json"), serde_json::to_vec(&env).unwrap()).unwrap();
    let answer = match args.first().map(String::as_str) {
        Some("-p") => CLAUDE_OK,
        Some("exec") => CODEX_OK,
        _ => &[],
    };
    // A line break first: libtest has written "test <name> ... " without one.
    let mut out = std::io::stdout().lock();
    writeln!(out).unwrap();
    for line in answer {
        writeln!(out, "{line}").unwrap();
    }
    out.flush().unwrap();
}

/// A shim as npm writes one, `<program>.cmd` that `cmd.exe` runs on Windows, passing on its
/// command line with `%*`, that starts this test binary as [`coding_app_fake_process`].
#[cfg(windows)]
fn shim_to_this_binary(dir: &Path, program: &str) -> PathBuf {
    let path = dir.join(format!("{program}.cmd"));
    // `%%` is a percent sign in a batch file.
    let exe = std::env::current_exe()
        .unwrap()
        .display()
        .to_string()
        .replace('%', "%%");
    std::fs::write(
        &path,
        format!("@echo off\r\n\"{exe}\" {FAKE_WORDS} %*\r\nexit /b %ERRORLEVEL%\r\n"),
    )
    .unwrap();
    path
}

/// A shim elsewhere: a shell script `<program>` passing on its command line with `"$@"`.
#[cfg(unix)]
fn shim_to_this_binary(dir: &Path, program: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap().display().to_string();
    script(
        dir,
        program,
        &format!(
            "#!/bin/sh\nexec '{}' {FAKE_WORDS} \"$@\"\n",
            exe.replace('\'', r"'\''")
        ),
    )
}

/// [0.0.6 release review] A run of each app through a shim, on every system: on Windows
/// through `<name>.cmd` as npm installs it, which `cmd.exe` runs (the one way to start the
/// apps that adds a layer of quoting: Claude Code's `--settings` holds double quotes), and
/// elsewhere through a shell script. The shim starts this test binary as the fake app. It
/// gets the template word for word, the task byte for byte on standard input, the chat's
/// working folder, and the environment without secret-looking variables; its answer comes
/// back.
#[tokio::test]
async fn a_run_through_a_shim_gets_the_template_the_task_the_folder_and_the_environment() {
    let bin = temp("shim-bin");
    let work = temp("shim-work");
    let record = temp("shim-record");
    let shims = [
        (CodingApp::ClaudeCode, shim_to_this_binary(&bin, "claude")),
        (CodingApp::Codex, shim_to_this_binary(&bin, "codex")),
    ];
    // The app's own environment, as the apps get it outside macOS.
    let mut env = this_env();
    env.insert(FAKE_RECORD_ENV.into(), record.display().to_string());
    env.insert("CODING_APPS_PLAIN_SETTING".into(), "kept-value".into());
    env.insert("ANTHROPIC_API_KEY".into(), "sk-shim-secret".into());
    let s = server_for(&bin, env);
    for (app, shim) in &shims {
        let found = s.saved.get(*app).and_then(|a| a.path.clone());
        assert_eq!(found, Some(shim.display().to_string()));
    }
    let task = "-p --dangerously-skip-permissions \"quoted\" 'single' %PATH% !PATH! $HOME `id` ^& | < >\nsecond line\r\n\tthird";
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
        for file in ["args.json", "stdin.txt", "cwd.txt", "env.json"] {
            let _ = std::fs::remove_file(record.join(file));
        }
        let action = describe(
            &s,
            tool,
            json!({"task": task, "may_edit": may_edit}),
            &chat(&work),
        )
        .await;
        assert_eq!(action.hard_limit, None, "{tool}: {}", action.preview);
        let (out, _) = call(&s, &action, &chat(&work)).await;
        assert!(!out.is_error, "{tool} {may_edit}: {out:?}");
        assert_eq!(out.joined_text(), answer, "{tool} {may_edit}");
        let read = |file: &str| std::fs::read(record.join(file)).unwrap();
        let args: Vec<String> = serde_json::from_slice(&read("args.json")).unwrap();
        assert_eq!(args, arguments(app, may_edit), "{tool} {may_edit}");
        assert_eq!(read("stdin.txt"), task.as_bytes(), "{tool} {may_edit}");
        let cwd = String::from_utf8(read("cwd.txt")).unwrap();
        assert_eq!(dunce::canonicalize(cwd).unwrap(), work, "{tool} {may_edit}");
        let seen: BTreeMap<String, String> = serde_json::from_slice(&read("env.json")).unwrap();
        assert_eq!(
            seen.get("CODING_APPS_PLAIN_SETTING").map(String::as_str),
            Some("kept-value")
        );
        let secrets: Vec<&String> = seen
            .iter()
            .filter(|(k, v)| tools::terminal::looks_secret(k) || v.contains("sk-shim-secret"))
            .map(|(k, _)| k)
            .collect();
        assert!(secrets.is_empty(), "{tool}: {secrets:?} reached the app");
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

/// The secret-looking lines of the environment a fake app recorded.
fn secrets_seen(bin: &Path) -> Vec<String> {
    std::fs::read_to_string(bin.join("env.txt"))
        .unwrap()
        .lines()
        .filter(|l| {
            l.split_once('=')
                .is_some_and(|(k, _)| tools::terminal::looks_secret(k))
        })
        .map(str::to_owned)
        .collect()
}

/// While sharing listens, an app set up for it gets the sharing key Sharing holds, in the
/// one variable its own setup reads it from (Claude Code: ANTHROPIC_AUTH_TOKEN; Codex: the
/// chosen provider's env_key), and no other secret-looking variable: the owner's own values
/// of those names stay stripped like every other. An app set up for another port, or not
/// at all, gets no key.
#[cfg(unix)]
#[tokio::test]
async fn the_sharing_key_reaches_only_an_app_set_up_for_the_live_port() {
    let bin = temp("key-bin");
    let work = temp("key-work");
    fake(&bin, "claude", CLAUDE_OK);
    fake(&bin, "codex", CODEX_OK);
    let mut owner = base_env();
    for (k, v) in [
        ("ANTHROPIC_AUTH_TOKEN", "owner-auth-token"),
        ("HARNESSUI_SHARE_KEY", "owner-share-key"),
        ("ANTHROPIC_API_KEY", "owner-anthropic-key"),
        ("OPENAI_API_KEY", "owner-openai-key"),
        ("GITHUB_TOKEN", "owner-github-token"),
        ("PLAIN_SETTING", "kept-value"),
    ] {
        owner.insert(k.into(), v.into());
    }
    let run = async |env: &BTreeMap<String, String>, tool: &str, share: ShareNow| {
        let mut s = server_for(&bin, env.clone());
        s.share = share_is(share);
        let a = describe(&s, tool, json!({"task": "x"}), &chat(&work)).await;
        assert_eq!(a.hard_limit, None, "{tool}");
        let (out, _) = call(&s, &a, &chat(&work)).await;
        assert!(!out.is_error, "{tool}: {out:?}");
        let seen = std::fs::read_to_string(bin.join("env.txt")).unwrap();
        assert!(seen.contains("PLAIN_SETTING=kept-value"), "{seen}");
        assert!(seen.contains("PATH=/usr/bin:/bin"), "{seen}");
        assert!(
            !seen.contains("owner-"),
            "an owner's secret reached {tool}: {seen}"
        );
        (a, secrets_seen(&bin))
    };

    // Claude Code set up for the live port.
    let mut claude = owner.clone();
    claude.insert("ANTHROPIC_BASE_URL".into(), "http://127.0.0.1:51234".into());
    let (a, seen) = run(&claude, CLAUDE_TOOL, live(51234)).await;
    assert!(
        a.preview
            .contains(&online_line(CodingApp::ClaudeCode, true))
    );
    assert_eq!(seen, vec![format!("ANTHROPIC_AUTH_TOKEN={SHARING_KEY}")]);

    // Codex set up for the live port through its config.toml, as the setup card writes it.
    let mut codex = owner.clone();
    let home = codex_home(
        "key-codex",
        "http://127.0.0.1:51234/v1",
        "HARNESSUI_SHARE_KEY",
    );
    codex.insert("CODEX_HOME".into(), home.display().to_string());
    let (a, seen) = run(&codex, CODEX_TOOL, live(51234)).await;
    assert!(a.preview.contains(&online_line(CodingApp::Codex, true)));
    assert_eq!(seen, vec![format!("HARNESSUI_SHARE_KEY={SHARING_KEY}")]);

    // Set up for another port than the live one: no key, and the card says online.
    let mut other = owner.clone();
    other.insert("ANTHROPIC_BASE_URL".into(), "http://127.0.0.1:4000".into());
    let other_home = codex_home(
        "key-codex-other",
        "http://127.0.0.1:4000/v1",
        "HARNESSUI_SHARE_KEY",
    );
    other.insert("CODEX_HOME".into(), other_home.display().to_string());
    for (tool, app) in [
        (CLAUDE_TOOL, CodingApp::ClaudeCode),
        (CODEX_TOOL, CodingApp::Codex),
    ] {
        let (a, seen) = run(&other, tool, live(51234)).await;
        assert!(
            a.preview.contains(&online_line(app, false)),
            "{}",
            a.preview
        );
        assert!(seen.is_empty(), "{tool}: {seen:?}");
    }

    // A provider whose env_key names a variable the app keeps: the card says local (it is),
    // but the key is not put there and the variable keeps its value.
    let mut odd = owner.clone();
    let odd_home = codex_home("key-codex-odd", "http://127.0.0.1:51234/v1", "PATH");
    odd.insert("CODEX_HOME".into(), odd_home.display().to_string());
    let (a, seen) = run(&odd, CODEX_TOOL, live(51234)).await;
    assert!(a.preview.contains(&online_line(CodingApp::Codex, true)));
    assert!(seen.is_empty(), "{seen:?}");
    let recorded = std::fs::read_to_string(bin.join("env.txt")).unwrap();
    assert!(!recorded.contains(SHARING_KEY), "{recorded}");
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
