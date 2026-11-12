//! The selection as a pure function: which groups a request is about, and which tools a
//! turn shows.

use std::collections::BTreeSet;

use agent::history::{CallRecord, HistoryMessage, Part};
use serde_json::json;

use super::*;
use crate::tool_catalog;

/// Built-in tools as the selection sees them, in the order the app registers their
/// servers: the given groups' tools, from the catalog, in a chat that is not in Full access
/// (the tools that organise folders are removed before the selection, `turns::assemble`).
fn on(groups: &[&str]) -> Vec<Candidate> {
    on_in(groups, false)
}

fn on_in(groups: &[&str], full_access: bool) -> Vec<Candidate> {
    tool_catalog::ENTRIES
        .iter()
        .filter(|e| groups.contains(&e.group))
        .filter(|e| full_access || !(e.group == "files" && tools::files::organize::is_tool(e.name)))
        .map(|e| Candidate {
            name: e.name.to_string(),
            server: e.group.to_string(),
            description: e.purpose.to_string(),
            builtin: true,
            cost: 100,
        })
        .collect()
}

/// What a new chat has on: files, the browser, artifacts, search, and the companions.
fn default_set() -> Vec<Candidate> {
    on(&[
        "files",
        "browser",
        "artifacts",
        "search",
        "toolfinder",
        "outputs",
        "task",
    ])
}

fn external(server: &str, tool: &str, description: &str) -> Candidate {
    Candidate {
        name: format!("{server}__{tool}"),
        server: server.to_string(),
        description: description.to_string(),
        builtin: false,
        cost: 100,
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn texts(list: &[&str]) -> Vec<String> {
    names(list)
}

fn pick(
    tools: &[Candidate],
    messages: &[&str],
    shown: &[String],
    wanted: &[String],
) -> Vec<String> {
    let messages = texts(messages);
    select(&Selecting {
        tools,
        messages: &messages,
        shown,
        wanted,
        skills: &[],
        skill_loaded: false,
        max: DEFAULT_MAX,
        allowance: None,
    })
}

const BASE: &[&str] = &[
    "list_dir",
    "read_file",
    "search_files",
    "write_file",
    "edit_file",
    "find_tools",
    "call_tool",
    "read_output",
    "update_task",
];

/// `earlier` in order, possibly with other names in between, inside `later`.
fn is_subsequence(earlier: &[String], later: &[String]) -> bool {
    let mut rest = later.iter();
    earlier.iter().all(|name| rest.any(|l| l == name))
}

// ---- words --------------------------------------------------------------------------------

#[test]
fn a_keyword_matches_its_plural_and_its_ing_and_ed_forms() {
    for (text, keyword) in [
        ("two commits", "commit"),
        ("committing now", "commit"),
        ("it was committed", "commit"),
        ("list the branches", "branch"),
        ("both repositories", "repository"),
        ("installing it", "install"),
        ("is it running", "run"),
        ("they merged it", "merge"),
        ("it compiled", "compile"),
        ("three pages", "page"),
        ("the tests", "test"),
        ("keep searching", "search"),
        ("a drawing", "draw"),
        ("two websites", "website"),
        ("Commit THIS", "commit"),
        ("git_commit", "commit"),
        ("see https://example.com/x", "https"),
        ("please look up the docs", "look up"),
        ("click on OK", "click on"),
    ] {
        assert!(
            Wording::of(text).has(keyword),
            "\"{text}\" should match {keyword}"
        );
    }
    for (text, keyword) in [
        ("a commitment", "commit"),
        ("too pushy", "push"),
        ("the testament", "test"),
        ("gitter chat", "git"),
        ("the outlook upgrade", "look up"),
        ("nothing here", "commit"),
        ("", "commit"),
    ] {
        assert!(
            !Wording::of(text).has(keyword),
            "\"{text}\" should not match {keyword}"
        );
    }
}

// ---- groups -------------------------------------------------------------------------------

/// The groups a request is about: a table of messages and what they must match. Every
/// built-in tool that is not always shown takes part, so a message that should match
/// nothing proves that no group's words are too loose.
#[test]
fn requests_match_the_groups_they_are_about() {
    let all: Vec<Candidate> = on(&[
        "files",
        "terminal",
        "git",
        "skills",
        "browser",
        "computer",
        "artifacts",
        "search",
    ])
    .into_iter()
    .filter(|t| !is_always_shown(t) && !is_core(t))
    .collect();
    let skills = vec![(
        "pdf-forms".to_string(),
        "Fill in PDF forms. Use when a PDF has fields to complete.".to_string(),
    )];
    let table: &[(&[&str], &[&str])] = &[
        (&["Please read notes.txt and tell me what it says"], &[]),
        (&["thanks, that looks good"], &[]),
        (&["what does this function do?"], &[]),
        (&["open the file notes.txt"], &[]),
        (
            &["explain the second paragraph on this page of the pdf"],
            &[],
        ),
        (&["rename the variable in main.py"], &[]),
        (&["run the tests and fix what fails"], &["terminal"]),
        (&["install the dependencies with npm"], &["terminal"]),
        (&["does it compile?"], &["terminal"]),
        (&["commit these changes with a good message"], &["git"]),
        (&["what changed on this branch? show me the diff"], &["git"]),
        (&["build the project and push it"], &["terminal", "git"]),
        (
            &["open https://example.com and tell me what the page says"],
            &["browser"],
        ),
        (
            &["log in to the website and fill in the form"],
            &["browser"],
        ),
        (
            &["go to the pricing page and click the first button"],
            &["browser"],
        ),
        (
            &["look up the latest documentation for tokio timeouts"],
            &["search"],
        ),
        (&["search the web for news about the release"], &["search"]),
        (&["find out who maintains this library"], &["search"]),
        (&["draw a diagram of the sign-up flow"], &["artifacts"]),
        (&["make a small calculator as an html page"], &["artifacts"]),
        (&["write a report about the meeting"], &["artifacts"]),
        (&["run the tests and report the results"], &["terminal"]),
        (
            &["click on the Start button on my screen"],
            &["browser", "computer"],
        ),
        (
            &["move the mouse to the corner of the desktop"],
            &["computer"],
        ),
        (&["take a screenshot of the page"], &["browser", "computer"]),
        (&["use the pdf-forms skill for this"], &["skills"]),
        (&["fill in the pdf form I attached"], &["skills"]),
        (&["delete the old draft"], &["files"]),
        (&["test the login page"], &["terminal", "browser"]),
        // The two messages before the latest one count too.
        (
            &["search the web for the release date", "ok", "now try again"],
            &["search"],
        ),
        (&["look at the website", "thanks", "open it"], &["browser"]),
    ];
    for (messages, expected) in table {
        let wording = Wording::of(&messages.join("\n"));
        let got: BTreeSet<String> = matching_groups(&all, &wording, &skills)
            .into_iter()
            .map(|(group, _)| group)
            .collect();
        let want: BTreeSet<String> = expected.iter().map(|g| g.to_string()).collect();
        assert_eq!(got, want, "{messages:?}");
    }
}

/// An external server has no catalog entry: its name, its tools' names and their
/// descriptions are what a request is matched against.
#[test]
fn an_external_server_matches_by_its_names_and_descriptions() {
    let tools = vec![
        external(
            "weather",
            "get_forecast",
            "Get the weather forecast for a city.",
        ),
        external(
            "tracker",
            "create_issue",
            "Create an issue in the project tracker with a title and a body.",
        ),
    ];
    let groups = |text: &str| -> Vec<String> {
        matching_groups(&tools, &Wording::of(text), &[])
            .into_iter()
            .map(|(g, _)| g)
            .collect()
    };
    assert_eq!(
        groups("what is the weather in Paris tomorrow?"),
        names(&["weather"])
    );
    assert_eq!(groups("give me the forecast"), names(&["weather"]));
    assert_eq!(groups("create an issue for this bug"), names(&["tracker"]));
    assert_eq!(groups("put it in the tracker"), names(&["tracker"]));
    assert!(groups("read the file").is_empty());
    assert!(
        groups("create a file with a title").is_empty(),
        "one everyday word is not a match"
    );
}

/// The best match comes first; a tie keeps the order the tools are in.
#[test]
fn matching_groups_are_ordered_by_score() {
    let all: Vec<Candidate> = on(&["terminal", "git", "browser"]);
    let wording = Wording::of("commit and push the branch, then run the build");
    let got: Vec<String> = matching_groups(&all, &wording, &[])
        .into_iter()
        .map(|(g, _)| g)
        .collect();
    assert_eq!(got, names(&["git", "terminal"]));
    let scores = matching_groups(&all, &wording, &[]);
    assert!(scores[0].1 > scores[1].1, "{scores:?}");
}

// ---- the shown set ------------------------------------------------------------------------

/// [0.0.5 full access] A request to organise brings in the tools that create and move
/// folders, when the chat is in Full access and they are offered at all.
#[test]
fn a_request_to_organise_shows_the_folder_tools_in_full_access() {
    let tools = on_in(&["files", "toolfinder", "outputs", "task"], true);
    assert!(tools.iter().any(|t| t.name == "move_path"));
    let shown = pick(
        &tools,
        &["Organize my Downloads folder: put the PDFs in a new folder"],
        &[],
        &[],
    );
    for tool in ["make_folder", "move_path"] {
        assert!(shown.contains(&tool.to_string()), "{tool}: {shown:?}");
    }
    assert!(
        on(&["files"])
            .iter()
            .all(|t| !tools::files::organize::is_tool(&t.name))
    );
}

#[test]
fn a_plain_request_shows_the_companions_and_the_core_file_tools() {
    let tools = default_set();
    assert_eq!(tools.len(), 26);
    let shown = pick(
        &tools,
        &["Please read notes.txt and tell me what it says"],
        &[],
        &[],
    );
    assert_eq!(shown, names(BASE));
    // No message at all (the Tools panel before the first turn): the same.
    assert_eq!(pick(&tools, &[], &[], &[]), names(BASE));
}

#[test]
fn a_matching_group_is_shown_whole_when_it_fits() {
    let tools = default_set();
    let shown = pick(&tools, &["draw a diagram of the sign-up flow"], &[], &[]);
    let mut want = names(BASE);
    // In the order the tools are registered: artifacts come before the task tool.
    want.splice(
        8..8,
        names(&[
            "create_artifact",
            "update_artifact",
            "read_artifact",
            "list_artifacts",
        ]),
    );
    assert_eq!(shown, want);
    assert_eq!(shown.len(), 13);
}

/// 14 by default. Companions and the core are kept, then what the chat used or found, then
/// the best matches of the matching group.
#[test]
fn the_cap_keeps_companions_then_used_and_found_then_the_best_matches() {
    let tools = default_set();
    let wanted = names(&["browser_back", "browser_fill_form"]);
    let shown = pick(
        &tools,
        &["open the website and click the sign-in button"],
        &[],
        &wanted,
    );
    assert_eq!(shown.len(), DEFAULT_MAX);
    for name in BASE {
        assert!(
            shown.contains(&name.to_string()),
            "{name} was dropped: {shown:?}"
        );
    }
    let browser: Vec<&str> = shown
        .iter()
        .map(String::as_str)
        .filter(|n| n.starts_with("browser_"))
        .collect();
    // Used and found first (two of the five places), then the three tools the request
    // points at most: "click" and "button" (click), "open" and "website" (navigate),
    // "button" (find). The others of the group stay behind `find_tools`.
    assert_eq!(
        browser,
        [
            "browser_navigate",
            "browser_find",
            "browser_click",
            "browser_fill_form",
            "browser_back"
        ]
    );
    // Without the cap in the way the whole group is shown.
    let messages = texts(&["open the website and click the sign-in button"]);
    let roomy = select(&Selecting {
        tools: &tools,
        messages: &messages,
        shown: &[],
        wanted: &[],
        skills: &[],
        skill_loaded: false,
        max: 30,
        allowance: None,
    });
    assert_eq!(
        roomy.iter().filter(|n| n.starts_with("browser_")).count(),
        10
    );
}

/// Two groups match and not everything fits: they take turns, so the smaller group is not
/// starved by the larger one.
#[test]
fn matching_groups_share_the_places_that_are_left() {
    let tools = default_set();
    let shown = pick(
        &tools,
        &["search the web, then open the website it finds"],
        &[],
        &[],
    );
    assert_eq!(shown.len(), DEFAULT_MAX);
    let extra: Vec<&str> = shown
        .iter()
        .map(String::as_str)
        .filter(|n| !BASE.contains(n))
        .collect();
    assert!(
        extra.contains(&"web_search") && extra.contains(&"read_source"),
        "{extra:?}"
    );
    assert!(extra.contains(&"browser_navigate"), "{extra:?}");
    assert_eq!(
        extra.iter().filter(|n| n.starts_with("browser_")).count(),
        3,
        "{extra:?}"
    );
}

/// The cap is about choosing among many tools. A chat whose tools all fit under it gets
/// them all: nothing is held back for no gain.
#[test]
fn everything_is_shown_when_it_fits_under_the_cap() {
    let tools = on(&["files", "terminal", "toolfinder", "outputs", "task"]);
    assert_eq!(tools.len(), 11);
    let shown = pick(&tools, &["hello"], &[], &[]);
    assert_eq!(
        shown,
        tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
    );
}

/// The invariant behind the prompt cache: across the turns of a chat the list only grows,
/// and it is always in the one fixed order (the order of the chat's tools), so an earlier
/// turn's list is the later one's with nothing moved and nothing taken out.
#[test]
fn across_turns_the_list_only_grows_and_keeps_its_order() {
    let tools = on(&[
        "files",
        "terminal",
        "git",
        "browser",
        "artifacts",
        "search",
        "toolfinder",
        "outputs",
        "task",
    ]);
    let order: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
    let turns: &[(&str, &[&str])] = &[
        ("read the notes file", &[]),
        ("thanks", &[]),
        ("run the tests", &[]),
        ("commit the fix", &["git_status"]),
        ("something else entirely", &[]),
        ("draw a diagram of it", &["create_artifact"]),
        ("look it up on the web", &[]),
        ("open the website", &["browser_navigate", "browser_click"]),
        ("read the notes file", &[]),
    ];
    let mut stored: Vec<String> = Vec::new();
    let mut previous: Vec<String> = Vec::new();
    let mut said: Vec<String> = Vec::new();
    for (i, (message, found)) in turns.iter().enumerate() {
        said.push(message.to_string());
        let recent: Vec<String> = said.iter().rev().take(3).cloned().collect();
        let wanted = names(found);
        let input = Selecting {
            tools: &tools,
            messages: &recent,
            shown: &stored,
            wanted: &wanted,
            skills: &[],
            skill_loaded: false,
            max: DEFAULT_MAX,
            allowance: None,
        };
        let shown = select(&input);
        // The same question gives the same answer.
        assert_eq!(select(&input), shown, "turn {i}");
        // One fixed order: the list is the chat's tools with some left out.
        let filtered: Vec<String> = order
            .iter()
            .filter(|n| shown.contains(n))
            .cloned()
            .collect();
        assert_eq!(
            shown, filtered,
            "turn {i}: not in the order of the chat's tools"
        );
        // Only grows: everything shown before is still there, in the same order.
        assert!(
            is_subsequence(&previous, &shown),
            "turn {i}: {previous:?} then {shown:?}"
        );
        assert!(shown.len() >= previous.len(), "turn {i}");
        assert!(
            shown.len() <= DEFAULT_MAX,
            "turn {i}: {} tools",
            shown.len()
        );
        stored = remembered(&stored, &shown);
        previous = shown;
    }
    // By the end the chat has grown to the cap, and what it used early is still there.
    assert_eq!(previous.len(), DEFAULT_MAX);
    for name in [
        "run_command",
        "git_status",
        "git_commit",
        "find_tools",
        "call_tool",
        "read_file",
    ] {
        assert!(previous.contains(&name.to_string()), "{name}: {previous:?}");
    }
    // What was remembered names every tool ever shown, once.
    let unique: BTreeSet<&String> = stored.iter().collect();
    assert_eq!(unique.len(), stored.len());
    assert!(previous.iter().all(|n| stored.contains(n)));
}

/// A tool that is not on in the chat is never shown, whatever was remembered, used or found.
#[test]
fn only_tools_that_are_on_are_ever_shown() {
    let tools = on(&[
        "files",
        "terminal",
        "browser",
        "artifacts",
        "toolfinder",
        "outputs",
        "task",
    ]);
    let stored = names(&["git_push", "computer_click", "weather__forecast"]);
    let wanted = names(&["git_commit", "run_subagent", "browser_back"]);
    let shown = pick(
        &tools,
        &["commit and push with git, then click on my screen"],
        &stored,
        &wanted,
    );
    let on_names: BTreeSet<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        shown.iter().all(|n| on_names.contains(n.as_str())),
        "{shown:?}"
    );
    assert!(shown.contains(&"browser_back".to_string()));
    // The names of tools that are off are still remembered, for when they are on again.
    let kept = remembered(&stored, &shown);
    assert!(kept.starts_with(&stored));
}

/// `run_subagent` cannot be reached through `call_tool`, and the system message tells the
/// model to use `load_skill` by name: both are always in the list when they are on.
#[test]
fn run_subagent_and_load_skill_are_always_shown_when_on() {
    let tools = on(&[
        "files",
        "git",
        "skills",
        "subagents",
        "browser",
        "toolfinder",
        "outputs",
        "task",
    ]);
    let shown = pick(&tools, &["hello"], &[], &[]);
    assert!(shown.contains(&"run_subagent".to_string()));
    assert!(shown.contains(&"load_skill".to_string()));
    assert!(!shown.contains(&"read_skill_file".to_string()));
    assert_eq!(shown.len(), BASE.len() + 2);
}

/// A skill that is loaded in the chat refers to its files and its scripts: the tool that
/// reads them and the terminal are shown from the next turn on.
#[test]
fn a_loaded_skill_brings_its_file_tool_and_the_terminal() {
    let tools = on(&[
        "files",
        "terminal",
        "git",
        "skills",
        "browser",
        "toolfinder",
        "outputs",
        "task",
    ]);
    let messages = texts(&["carry on"]);
    let input = |skill_loaded| Selecting {
        tools: &tools,
        messages: &messages,
        shown: &[],
        wanted: &[],
        skills: &[],
        skill_loaded,
        max: DEFAULT_MAX,
        allowance: None,
    };
    let without = select(&input(false));
    assert!(
        !without.contains(&"read_skill_file".to_string())
            && !without.contains(&"run_command".to_string())
    );
    let with = select(&input(true));
    assert!(
        with.contains(&"read_skill_file".to_string()) && with.contains(&"run_command".to_string()),
        "{with:?}"
    );
}

/// What was shown stays shown even when the cap is lowered below it, and the companions
/// are shown whatever the cap is.
#[test]
fn the_cap_never_takes_away_what_was_shown() {
    let tools = default_set();
    let before = pick(&tools, &["draw a diagram"], &[], &[]);
    assert_eq!(before.len(), 13);
    let messages = texts(&["open the website"]);
    let after = select(&Selecting {
        tools: &tools,
        messages: &messages,
        shown: &before,
        wanted: &[],
        skills: &[],
        skill_loaded: false,
        max: MIN_MAX,
        allowance: None,
    });
    assert_eq!(after, before, "a lower cap removed tools, or added some");
}

/// A chat that used many tools (a long history, or one from before 0.0.4) still gets the
/// core file tools: what it used takes the places that are left after them, newest first.
#[test]
fn what_a_chat_used_never_pushes_out_the_core_file_tools() {
    let tools = on(&[
        "files",
        "terminal",
        "git",
        "browser",
        "toolfinder",
        "outputs",
        "task",
    ]);
    let used = names(&[
        "browser_navigate",
        "browser_snapshot",
        "browser_click",
        "browser_type",
        "browser_read_text",
        "git_status",
        "git_diff",
        "git_commit",
        "git_push",
        "run_command",
        "browser_back",
    ]);
    let shown = pick(&tools, &["carry on"], &[], &used);
    assert_eq!(shown.len(), DEFAULT_MAX);
    for name in BASE {
        assert!(
            shown.contains(&name.to_string()),
            "{name} was pushed out: {shown:?}"
        );
    }
    for name in &used[..5] {
        assert!(shown.contains(name), "{name} was used last: {shown:?}");
    }
    assert!(!shown.contains(&used[5]), "{shown:?}");
}

/// A request that matches a group whose tools are already shown changes nothing.
#[test]
fn matching_again_adds_nothing() {
    let tools = default_set();
    let first = pick(&tools, &["draw a diagram"], &[], &[]);
    let second = pick(&tools, &["now draw a chart"], &first, &[]);
    assert_eq!(first, second);
}

// ---- settings -----------------------------------------------------------------------------

/// With "choose tools for each task" off, every tool that is on is shown, as before
/// 0.0.4: nothing is kept back, whatever the chat remembers.
#[test]
fn with_the_selection_off_every_tool_that_is_on_is_shown() {
    let tools = on(&[
        "files",
        "terminal",
        "git",
        "browser",
        "artifacts",
        "search",
        "outputs",
        "task",
    ]);
    let everything: BTreeSet<String> = tools.iter().map(|t| t.name.clone()).collect();
    let prefs = crate::turns::ChatTools {
        shown: names(&["read_file"]),
        found: names(&["git_commit"]),
        ..Default::default()
    };
    let off = ToolSelection {
        auto: false,
        max: DEFAULT_MAX,
    };
    assert_eq!(
        shown_before_a_message(&tools, &prefs, &[], false, off, None),
        everything
    );
    // On: the companions, the core, and what waits to be shown; the rest on request.
    let chosen = shown_before_a_message(&tools, &prefs, &[], false, ToolSelection::default(), None);
    assert!(chosen.len() < everything.len());
    assert!(chosen.contains("read_file") && chosen.contains("git_commit"));
    assert!(chosen.contains("read_output") && chosen.contains("update_task"));
    assert!(!chosen.contains("git_push") && !chosen.contains("browser_navigate"));
}

#[test]
fn the_setting_is_on_with_14_unless_it_says_otherwise() {
    assert_eq!(
        ToolSelection::from_stored(None),
        ToolSelection {
            auto: true,
            max: 14
        }
    );
    assert_eq!(
        ToolSelection::from_stored(Some(&json!({"auto": false}))),
        ToolSelection {
            auto: false,
            max: 14
        }
    );
    assert_eq!(
        ToolSelection::from_stored(Some(&json!({"auto": true, "max": 20}))),
        ToolSelection {
            auto: true,
            max: 20
        }
    );
    // A cap that makes no sense is brought into range; anything unreadable is the default.
    assert_eq!(
        ToolSelection::from_stored(Some(&json!({"max": 1}))).max,
        MIN_MAX
    );
    assert_eq!(
        ToolSelection::from_stored(Some(&json!({"max": 5000}))).max,
        MAX_MAX
    );
    for bad in [
        json!(null),
        json!("on"),
        json!({"auto": "yes"}),
        json!({"max": -3}),
        json!([1]),
    ] {
        assert_eq!(
            ToolSelection::from_stored(Some(&bad)),
            ToolSelection::default(),
            "{bad}"
        );
    }
    assert_eq!(
        serde_json::to_value(ToolSelection::default()).unwrap(),
        json!({"auto": true, "max": 14})
    );
}

// ---- what the chat said and used --------------------------------------------------------------

fn call(name: &str, arguments: serde_json::Value) -> CallRecord {
    CallRecord {
        id: format!("c-{name}"),
        name: name.into(),
        arguments: arguments.as_object().cloned().unwrap(),
        recovered: false,
    }
}

#[test]
fn the_three_latest_user_messages_are_what_a_request_is_matched_on() {
    let mut with_attachment = HistoryMessage::user("third: look at this");
    with_attachment.parts.push(Part::Attachment {
        name: "notes.txt".into(),
        text: "commit push browser website".into(),
    });
    let history = vec![
        HistoryMessage::user("first"),
        HistoryMessage::assistant("commit push"),
        HistoryMessage::user("second"),
        HistoryMessage::assistant_calls("", vec![call("read_file", json!({"path": "a"}))]),
        with_attachment,
        HistoryMessage::assistant("an answer about git"),
        HistoryMessage::user("fourth"),
    ];
    assert_eq!(
        recent_user_messages(&history),
        texts(&["fourth", "third: look at this", "second"])
    );
    assert!(recent_user_messages(&[]).is_empty());
    // Long messages are cut, on a character boundary.
    let long = "é".repeat(MESSAGE_CHARS + 50);
    let cut = recent_user_messages(&[HistoryMessage::user(long)]);
    assert_eq!(cut[0].chars().count(), MESSAGE_CHARS);
}

#[test]
fn the_tools_a_chat_used_come_from_its_history_newest_first() {
    let history = vec![
        HistoryMessage::user("go"),
        HistoryMessage::assistant_calls("", vec![call("read_file", json!({"path": "a"}))]),
        HistoryMessage::assistant_calls(
            "",
            vec![
                call("call_tool", json!({"name": "git_status", "arguments": {}})),
                call("find_tools", json!({"query": "commit"})),
            ],
        ),
        HistoryMessage::assistant_calls("", vec![call("read_file", json!({"path": "b"}))]),
        HistoryMessage::assistant_calls("", vec![call("call_tool", json!({"name": 7}))]),
    ];
    assert_eq!(used_tools(&history), names(&["read_file", "git_status"]));
    assert!(used_tools(&[]).is_empty());
}

#[test]
fn the_tools_are_split_in_their_order() {
    let host = tool_catalog::tests::builtin_host();
    let all = host.tools();
    let shown = names(&["read_file", "git_status", "nothing_like_this"]);
    let (yes, no) = split(all.clone(), &shown);
    assert_eq!(
        yes.iter().map(|t| t.exposed.as_str()).collect::<Vec<_>>(),
        ["read_file", "git_status"]
    );
    assert_eq!(yes.len() + no.len(), all.len());
    let merged: BTreeSet<&str> = yes.iter().chain(&no).map(|t| t.exposed.as_str()).collect();
    assert_eq!(merged.len(), all.len());
    // The rest keep their order too.
    let rest: Vec<&str> = all
        .iter()
        .map(|t| t.exposed.as_str())
        .filter(|n| !shown.iter().any(|s| s == n))
        .collect();
    assert_eq!(
        no.iter().map(|t| t.exposed.as_str()).collect::<Vec<_>>(),
        rest
    );
}

/// What is remembered for a chat stays bounded, and names that are shown now are kept
/// before names that are only remembered.
#[test]
fn what_is_remembered_is_bounded() {
    let old: Vec<String> = (0..REMEMBERED_NAMES + 40)
        .map(|i| format!("old_{i}"))
        .collect();
    let shown = names(&["read_file", "old_3"]);
    let kept = remembered(&old, &shown);
    assert_eq!(kept.len(), REMEMBERED_NAMES);
    assert!(kept.contains(&"read_file".to_string()) && kept.contains(&"old_3".to_string()));
    // Nothing new: nothing changes, so nothing needs to be written.
    let same = remembered(&shown, &shown);
    assert_eq!(same, shown);
}

// ---- A size allowance: small context windows (0.0.4) ---------------------------------

fn pick_within(tools: &[Candidate], shown: &[String], allowance: Option<u32>) -> Vec<String> {
    select(&Selecting {
        tools,
        messages: &[],
        shown,
        wanted: &[],
        skills: &[],
        skill_loaded: false,
        max: DEFAULT_MAX,
        allowance,
    })
}

fn cost_of(tools: &[Candidate], shown: &[String]) -> u32 {
    tools
        .iter()
        .filter(|t| shown.contains(&t.name))
        .map(|t| t.cost)
        .sum()
}

/// The core file tools, in the order the files server offers them.
const CORE: &[&str] = &[
    "list_dir",
    "read_file",
    "search_files",
    "write_file",
    "edit_file",
];

/// What a new chat has on when no browser is found: the set that did not fit a model
/// loaded with 4,096 tokens of context, so that no message could be sent.
fn small_default_set() -> Vec<Candidate> {
    on(&["files", "skills", "artifacts", "search", "toolfinder"])
}

#[test]
fn a_small_window_gets_the_core_file_tools_and_then_only_what_fits() {
    let tools = small_default_set();
    assert_eq!(tools.len(), 16);
    let core = names(CORE);
    // Room enough: the list is the one chosen by count, as without an allowance.
    let by_count = pick_within(&tools, &[], None);
    assert_eq!(
        by_count,
        names(&[
            "list_dir",
            "read_file",
            "search_files",
            "write_file",
            "edit_file",
            "load_skill",
            "find_tools",
            "call_tool"
        ])
    );
    assert_eq!(pick_within(&tools, &[], Some(1_600)), by_count);
    assert_eq!(pick_within(&tools, &[], Some(800)), by_count);

    // Room for five tools: the core file tools, and not the two finder tools.
    let five = pick_within(&tools, &[], Some(500));
    assert_eq!(five, core);
    // Room for six: one more tool comes along, but not half of the finder pair.
    let six = pick_within(&tools, &[], Some(650));
    assert!(
        six.len() == 6 && core.iter().all(|c| six.contains(c)),
        "{six:?}"
    );
    assert!(!six.contains(&"find_tools".to_string()) && !six.contains(&"call_tool".to_string()));
    // Room for seven: the finder pair comes before anything else that is optional.
    let seven = pick_within(&tools, &[], Some(700));
    assert_eq!(seven.len(), 7);
    assert!(seven.contains(&"find_tools".to_string()) && seven.contains(&"call_tool".to_string()));

    // Never more than the allowance, whatever it is.
    for allowance in [0, 99, 100, 250, 499, 500, 701, 999, 1_234, 1_599] {
        let shown = pick_within(&tools, &[], Some(allowance));
        assert!(
            cost_of(&tools, &shown) <= allowance,
            "{allowance}: {shown:?}"
        );
    }
    // No room at all: no tools, and the turn is an ordinary chat turn.
    assert!(pick_within(&tools, &[], Some(99)).is_empty());
}

#[test]
fn what_a_chat_was_shown_before_is_kept_while_it_fits() {
    let tools = small_default_set();
    let before = names(&["list_dir", "read_file", "web_search", "read_source"]);
    // It fits: all of it is shown again, and the core tools join it.
    let roomy = pick_within(&tools, &before, Some(800));
    for name in &before {
        assert!(roomy.contains(name), "{name} was dropped: {roomy:?}");
    }
    assert!(names(CORE).iter().all(|c| roomy.contains(c)));
    // The window got smaller: the core file tools first, then what still fits of the rest.
    let tight = pick_within(&tools, &before, Some(600));
    assert!(names(CORE).iter().all(|c| tight.contains(c)), "{tight:?}");
    assert_eq!(tight.len(), 6);
    assert!(tight.contains(&"web_search".to_string()));
}

#[test]
fn a_tool_costs_about_what_its_definition_takes_in_a_request() {
    let small = definition_cost("t", "Short.", &json!({"type": "object"}));
    let large = definition_cost(
        "t",
        &"A long description of what the tool does. ".repeat(10),
        &json!({"type": "object", "properties": {"path": {"type": "string", "description": "Where."}}}),
    );
    assert!((15..60).contains(&small), "{small}");
    assert!(large > small + 100, "{large} vs {small}");
}

#[test]
fn the_allowance_follows_the_loops_own_limit() {
    // 4,096 tokens: the loop offers tools only while the fixed part is at most 40% of what
    // a prompt may use (2,992), so about 1,196. The allowance stays a little under that
    // and leaves room for the rest of the fixed part.
    let budget = agent::context::ContextBudget::new(4_096, 0).budget;
    assert_eq!(budget, 2_992);
    let all = allowance(4_096, 0);
    assert!(all < 1_196 && all > 1_100, "{all}");
    assert_eq!(allowance(4_096, 200), all - 200);
    assert_eq!(allowance(4_096, 5_000), 0);
    // More context, more room.
    assert!(allowance(8_192, 200) > 2 * allowance(4_096, 200));
    assert!(allowance(32_768, 200) > 8_000);
}

/// [0.0.5 memory] The summary of earlier messages is part of the fixed part: it counts
/// with its lines, and the tools get that much less room. A chat summarised under a large
/// context and continued at 4,096 tokens then shows fewer tools instead of refusing the
/// turn because its fixed part no longer fits.
#[test]
fn the_summary_takes_from_the_tools_share() {
    let prompt = "You are a helpful assistant.";
    let without = fixed_tokens(prompt, None);
    assert_eq!(without, agent::estimate::estimate_tokens(prompt) + 220);
    // Nothing to count for an empty summary.
    assert_eq!(fixed_tokens(prompt, Some("  \n")), without);
    // A summary of about 300 tokens.
    let summary = "Goal: plan the trip. ".repeat(43);
    let with = fixed_tokens(prompt, Some(&summary));
    assert_eq!(
        with,
        without + agent::estimate::estimate_tokens(summary.trim()) + SUMMARY_LINES_TOKENS
    );
    // At 4,096 tokens the tools get exactly that much less room.
    let room_without = allowance(4_096, without);
    let room_with = allowance(4_096, with);
    assert!(room_with > 0);
    assert_eq!(room_without - room_with, with - without);
    // The longest summary the loop writes at 32,768 tokens (1,024 tokens, 4,096
    // characters), counted whole, would leave the tools no room of their own at 4,096 ...
    let longest = "x".repeat(4_096);
    assert_eq!(allowance(4_096, fixed_tokens(prompt, Some(&longest))), 0);
    // ... but a turn counts it as the loop sends it (next test).
    let sent = summary_as_sent(&longest, 4_096);
    assert!(allowance(4_096, fixed_tokens(prompt, Some(&sent))) > 0);
}

/// [0.0.5 memory] A summary written when the model could remember more is folded again
/// under the smaller context's cap before the loop measures anything (or cut to it when
/// the model cannot fold it), so a turn counts it at that length: the tools are not
/// crowded out by text the request will not carry. A summary that fits is counted as it is.
#[test]
fn a_summary_is_counted_as_the_loop_sends_it() {
    let cap = |n_ctx| agent::context::ContextBudget::new(n_ctx, 0).summary_max_chars();
    let longest = format!(
        "Goal: fix the build.\n{}Exact paths and identifiers: /p/src/main.rs",
        "Work done: a step.\n".repeat(180)
    );
    assert!(longest.chars().count() > cap(4_096));
    assert!(longest.chars().count() <= cap(32_768));
    // At 32,768 tokens it fits: unchanged.
    assert_eq!(summary_as_sent(&longest, 32_768), longest.trim());
    // At 4,096 tokens it is held to that context's cap.
    let sent = summary_as_sent(&longest, 4_096);
    assert!(
        sent.chars().count() <= cap(4_096),
        "{}",
        sent.chars().count()
    );
    assert!(sent.starts_with("Goal: fix the build."));
    // The tools get the room the folded summary leaves them.
    let prompt = "You are a helpful assistant.";
    let whole = allowance(4_096, fixed_tokens(prompt, Some(&longest)));
    let folded = allowance(4_096, fixed_tokens(prompt, Some(&sent)));
    assert!(folded > whole, "{folded} > {whole}");
    assert!(
        folded
            >= allowance(4_096, fixed_tokens(prompt, None))
                - agent::estimate::estimate_tokens(&"x".repeat(cap(4_096)))
                - SUMMARY_LINES_TOKENS
    );
    // A short summary is never changed.
    assert_eq!(summary_as_sent("Goal: x.", 4_096), "Goal: x.");
}

/// [0.0.6] The Tools panel reads the fixed part before it knows the context
/// ([`other_fixed`]) and counts it for the context it then knows, with the summary as a turn
/// in that context sends it: after a chat summarised at 32,768 tokens moves to a 4,096-token
/// model, the panel marks the tools the next turn shows, not none.
#[test]
fn the_tools_panel_counts_the_summary_as_a_turn_sends_it() {
    let prompt = "You are a helpful assistant.";
    // The longest summary the loop writes at 32,768 tokens.
    let longest = "x".repeat(4_096);
    let fixed = OtherFixed {
        text: prompt.to_owned(),
        summary: Some(longest.clone()),
    };
    let turn = allowance(
        4_096,
        fixed_tokens(prompt, Some(&summary_as_sent(&longest, 4_096))),
    );
    assert!(turn > 0);
    assert_eq!(allowance(4_096, fixed.clone()), turn);
    // Where it fits, it is counted whole.
    assert_eq!(
        allowance(32_768, fixed),
        allowance(32_768, fixed_tokens(prompt, Some(&longest)))
    );
    // Without a summary, the text alone.
    let none = OtherFixed {
        text: prompt.to_owned(),
        summary: None,
    };
    assert_eq!(
        allowance(4_096, none),
        allowance(4_096, fixed_tokens(prompt, None))
    );
}
