//! What the app says about its own tools (0.0.4, adaptive tool selection).
//!
//! For every built-in tool: the group it belongs to, one sentence saying what it is for,
//! words that point at it, how much its group needs it, and one or two example argument
//! objects. `find_tools` prints this when the model looks for a tool, and the selection
//! (`tool_select.rs`) uses the group words to decide which groups fit a request.
//!
//! Tools of external (MCP) servers have no entry: their own description and schema are
//! used, with an example made from the schema's required fields ([`example_from_schema`]).
//!
//! Nothing here decides what a tool may do. That stays with each tool's describer and the
//! policy engine.

use std::collections::BTreeSet;

use mcp_host::RegisteredTool;
use serde_json::{Map, Value};

/// One built-in tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// The tool's name.
    pub name: &'static str,
    /// The built-in server that offers it (its group in the Tools panel).
    pub group: &'static str,
    /// One sentence: what the tool is for.
    pub purpose: &'static str,
    /// Words in a request or a search that point at this tool, besides the words of its
    /// name. They order tools; they never make a group match by themselves.
    pub keywords: &'static [&'static str],
    /// 1: the group cannot do its work without it. 2: used often. 3: now and then. When
    /// not every tool of a matching group can be shown, lower numbers go first.
    pub rank: u8,
    /// One or two argument objects as JSON text, each valid for the tool's schema.
    pub examples: &'static [&'static str],
}

/// One built-in group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Group {
    /// The server's name.
    pub server: &'static str,
    /// What the Tools panel calls it.
    pub label: &'static str,
    /// Words and phrases that mean this group by themselves.
    pub strong: &'static [&'static str],
    /// Words that only count together with another one.
    pub weak: &'static [&'static str],
}

/// The skill named in the examples of the skill tools. `find_tools` puts the name of an
/// enabled skill in its place.
pub const EXAMPLE_SKILL: &str = "pdf-forms";

/// The built-in groups. A phrase (more than one word) must appear as written; a single
/// word is written in its plain form and also matches its plural and its -ing and -ed
/// forms in a request.
pub const GROUPS: &[Group] = &[
    Group {
        server: "files",
        label: "Files",
        // The tools for reading and changing files are always shown; these words bring in
        // `delete_file` (and, in Full access, `delete_folder`). The other folder tools come in
        // by their own keywords.
        strong: &["delete", "remove", "erase", "get rid of"],
        weak: &[],
    },
    Group {
        server: "terminal",
        label: "Terminal",
        strong: &[
            "run",
            "command",
            "install",
            "build",
            "test",
            "npm",
            "cargo",
            "pip",
            "script",
            "compile",
            "execute",
            "shell",
            "terminal",
            "powershell",
            "bash",
            "python",
            "node",
            "pytest",
            "yarn",
            "pnpm",
            "lint",
            "docker",
            "command line",
        ],
        weak: &[
            "start",
            "launch",
            "package",
            "dependency",
            "server",
            "program",
        ],
    },
    Group {
        server: "git",
        label: "Git",
        strong: &[
            "git",
            "commit",
            "branch",
            "diff",
            "push",
            "pull",
            "merge",
            "repository",
            "repo",
            "clone",
            "rebase",
            "stash",
            "checkout",
            "staged",
            "pull request",
        ],
        weak: &["history", "revert", "stage", "remote", "upstream", "change"],
    },
    Group {
        server: "skills",
        label: "Skills",
        strong: &["skill"],
        weak: &[],
    },
    Group {
        server: "browser",
        label: "Browser",
        strong: &[
            "website", "url", "http", "https", "www", "browser", "browse", "click", "log in",
            "sign in", "login", "navigate", "visit", "webpage", "web page", "site",
        ],
        weak: &[
            "page", "open", "form", "submit", "web", "online", "link", "tab",
        ],
    },
    Group {
        server: "computer",
        label: "Computer",
        strong: &[
            "screen",
            "desktop",
            "mouse",
            "click on",
            "keyboard",
            "cursor",
            "my computer",
        ],
        weak: &["window", "app", "application", "program", "pointer"],
    },
    Group {
        server: "subagents",
        label: "Subagents",
        strong: &["helper", "subagent", "delegate"],
        weak: &[],
    },
    Group {
        server: "coding_apps",
        label: "Coding apps",
        strong: &["claude code", "codex", "coding app"],
        weak: &["claude"],
    },
    Group {
        server: "artifacts",
        label: "Artifacts",
        strong: &[
            "html",
            "diagram",
            "chart",
            "draw",
            "mermaid",
            "svg",
            "calculator",
            "artifact",
            "flowchart",
            "visualize",
            "visualise",
            "mockup",
            "prototype",
            "dashboard",
            "presentation",
            "a document",
            "a report",
            "a page",
        ],
        weak: &[
            "document",
            "report",
            "page",
            "tool",
            "table",
            "interactive",
            "preview",
            "widget",
            "design",
            "graph",
            "timeline",
            "sketch",
        ],
    },
    Group {
        server: "search",
        label: "Web search",
        strong: &[
            "search",
            "look up",
            "looking up",
            "latest",
            "documentation",
            "docs",
            "find out",
            "news",
            "google",
            "internet",
            "research",
            "up to date",
        ],
        weak: &[
            "web", "online", "current", "recent", "release", "version", "source", "cite",
            "official",
        ],
    },
    Group {
        server: "outputs",
        label: "Stored output",
        strong: &[],
        weak: &[],
    },
    Group {
        server: "task",
        label: "Task",
        strong: &[],
        weak: &[],
    },
    Group {
        server: "toolfinder",
        label: "Tool finder",
        strong: &[],
        weak: &[],
    },
    // [0.0.6 item 5] rea. Words match whole words, so a file name such as "notepad.exe"
    // counts through "exe".
    // TODO(0.0.6): the selection also counts the words of the tools' names, and rea's names
    // hold everyday ones ("function", "strings", "python", "rust", "view"), so "Write a Python
    // function" points at this group once it is on. Decide in `tool_select::group_score`
    // whether this group should match by the words below only (item5-spec.md R18 assumes so).
    Group {
        server: "reverse",
        label: "Reverse engineering",
        strong: &[
            "decompile",
            "decompiler",
            "disassemble",
            "disassembly",
            "reverse engineer",
            "reverse engineering",
            "binary",
            "pseudocode",
            "exe",
            "dll",
            "dylib",
        ],
        weak: &["program", "function", "library"],
    },
];

/// The built-in tools, by group.
pub const ENTRIES: &[Entry] = &[
    // ---- files ----
    Entry {
        name: "list_dir",
        group: "files",
        purpose: "List the files and folders inside a folder.",
        keywords: &["folder", "directory", "ls", "tree"],
        rank: 1,
        examples: &[r#"{"path": "."}"#, r#"{"path": "src", "depth": 2}"#],
    },
    Entry {
        name: "read_file",
        group: "files",
        purpose: "Read a text file, or a range of lines from a long one.",
        keywords: &["open", "view", "show", "content", "cat"],
        rank: 1,
        examples: &[
            r#"{"path": "notes.txt"}"#,
            r#"{"path": "src/main.py", "offset": 120, "limit": 80}"#,
        ],
    },
    Entry {
        name: "search_files",
        group: "files",
        purpose: "Find text in the files under a folder and get the matching lines.",
        keywords: &["grep", "find", "where", "occurrence"],
        rank: 1,
        examples: &[
            r#"{"path": ".", "query": "TODO"}"#,
            r#"{"path": "src", "query": "fn main", "glob": "*.rs"}"#,
        ],
    },
    Entry {
        name: "write_file",
        group: "files",
        purpose: "Create a file, or replace everything in an existing one.",
        keywords: &["create", "save", "new"],
        rank: 1,
        examples: &[r##"{"path": "notes/todo.md", "content": "# To do\n\n- Write the report\n"}"##],
    },
    Entry {
        name: "edit_file",
        group: "files",
        purpose: "Change part of an existing file by replacing exact text.",
        keywords: &["change", "modify", "replace", "fix", "update"],
        rank: 1,
        examples: &[
            r#"{"path": "src/app.py", "old_text": "DEBUG = True", "new_text": "DEBUG = False"}"#,
            r#"{"path": "README.md", "old_text": "colour", "new_text": "color", "replace_all": true}"#,
        ],
    },
    Entry {
        name: "delete_file",
        group: "files",
        purpose: "Delete one file; the user is always asked first.",
        keywords: &["remove", "erase"],
        rank: 3,
        examples: &[r#"{"path": "notes/old-draft.txt"}"#],
    },
    // [0.0.5 full access] Offered only while the chat is in Full access.
    Entry {
        name: "make_folder",
        group: "files",
        purpose: "Create a folder (Full access only).",
        keywords: &["folder", "directory", "mkdir", "organize", "organise"],
        rank: 2,
        examples: &[r#"{"path": "Documents/Invoices/2026"}"#],
    },
    Entry {
        name: "move_path",
        group: "files",
        purpose: "Move or rename a file or folder, never replacing anything (Full access only).",
        keywords: &["move", "rename", "organize", "organise", "sort", "tidy"],
        rank: 2,
        examples: &[
            r#"{"from": "Downloads/report.pdf", "to": "Documents/Reports"}"#,
            r#"{"from": "notes/draft.md", "to": "notes/final.md"}"#,
        ],
    },
    Entry {
        name: "delete_folder",
        group: "files",
        purpose: "Delete a folder and everything in it (Full access only).",
        keywords: &["remove", "erase", "folder", "directory"],
        rank: 3,
        examples: &[r#"{"path": "Downloads/old-installers"}"#],
    },
    // ---- terminal ----
    Entry {
        name: "run_command",
        group: "terminal",
        purpose: "Run one shell command in an approved folder and get its output and exit code.",
        keywords: &[
            "shell", "terminal", "execute", "install", "build", "test", "script", "npm", "cargo",
            "pip", "python",
        ],
        rank: 1,
        examples: &[
            r#"{"command": "npm test"}"#,
            r#"{"command": "python build.py", "cwd": "tools", "timeout_seconds": 300}"#,
        ],
    },
    // ---- git ----
    Entry {
        name: "git_status",
        group: "git",
        purpose: "Show the current branch and which files have changed.",
        keywords: &["changed", "branch", "modified"],
        rank: 1,
        examples: &[r#"{}"#],
    },
    Entry {
        name: "git_diff",
        group: "git",
        purpose: "Show what changed in the files, or what is staged for the next commit.",
        keywords: &["change", "staged", "compare"],
        rank: 1,
        examples: &[r#"{"staged": true}"#, r#"{"path": "src/main.rs"}"#],
    },
    Entry {
        name: "git_log",
        group: "git",
        purpose: "List recent commits.",
        keywords: &["history", "commit", "recent"],
        rank: 2,
        examples: &[r#"{"limit": 10}"#, r#"{"path": "README.md", "limit": 5}"#],
    },
    Entry {
        name: "git_commit",
        group: "git",
        purpose: "Stage the given paths, or all changes when none are given, and commit them.",
        keywords: &["save", "stage", "message"],
        rank: 1,
        examples: &[
            r#"{"message": "Fix the date parser"}"#,
            r#"{"message": "Add tests for dates", "paths": ["tests/test_dates.py"]}"#,
        ],
    },
    Entry {
        name: "git_clone",
        group: "git",
        purpose: "Copy a repository from an https or ssh address into a folder.",
        keywords: &["download", "repository", "checkout"],
        rank: 3,
        examples: &[
            r#"{"url": "https://github.com/example/project.git", "destination": "project"}"#,
        ],
    },
    Entry {
        name: "git_pull",
        group: "git",
        purpose: "Bring the current branch up to date with its upstream.",
        keywords: &["update", "fetch", "upstream"],
        rank: 2,
        examples: &[r#"{}"#],
    },
    Entry {
        name: "git_push",
        group: "git",
        purpose: "Send the current branch's commits to a remote; the user is always asked first.",
        keywords: &["upload", "publish", "remote"],
        rank: 2,
        examples: &[r#"{}"#, r#"{"remote": "origin"}"#],
    },
    // ---- skills ----
    Entry {
        name: "load_skill",
        group: "skills",
        purpose: "Load the instructions of an enabled skill before following it.",
        keywords: &["instructions", "use"],
        rank: 1,
        examples: &[r#"{"name": "pdf-forms"}"#],
    },
    Entry {
        name: "read_skill_file",
        group: "skills",
        purpose: "Read one file from a skill's folder.",
        keywords: &["reference", "script"],
        rank: 2,
        examples: &[r#"{"name": "pdf-forms", "path": "reference/fields.md"}"#],
    },
    // ---- subagents ----
    Entry {
        name: "run_subagent",
        group: "subagents",
        purpose: "Hand a self-contained task to a helper that works in its own context and returns only its answer.",
        keywords: &["helper", "delegate", "side task"],
        rank: 1,
        examples: &[
            r#"{"task": "Read every file in docs and list the ones that mention the old API.", "context": "The project is in the working folder."}"#,
        ],
    },
    // ---- coding apps (0.0.6) ----
    Entry {
        name: "ask_claude_code",
        group: "coding_apps",
        purpose: "Ask Claude Code on this computer to do a task in the working folder; it works online and asks first, except in Full access.",
        keywords: &["claude code", "claude", "coding app", "second opinion"],
        rank: 1,
        examples: &[
            r#"{"task": "Find why the tests in tests/test_calc.py fail and explain the cause."}"#,
            r#"{"task": "Fix the failing test in tests/test_calc.py.", "may_edit": true}"#,
        ],
    },
    Entry {
        name: "ask_codex",
        group: "coding_apps",
        purpose: "Ask Codex on this computer to do a task in the working folder; it works online and asks first, except in Full access.",
        keywords: &["codex", "coding app", "second opinion"],
        rank: 1,
        examples: &[
            r#"{"task": "Review src/main.rs and list likely bugs."}"#,
            r#"{"task": "Add a unit test for parse_args in src/cli.rs.", "may_edit": true}"#,
        ],
    },
    // ---- toolfinder ----
    Entry {
        name: "find_tools",
        group: "toolfinder",
        purpose: "Search the tools that are on in this chat, including ones not in your list.",
        keywords: &[],
        rank: 1,
        examples: &[r#"{"query": "commit"}"#],
    },
    Entry {
        name: "call_tool",
        group: "toolfinder",
        purpose: "Run a tool that is on in this chat but not in your list this turn.",
        keywords: &[],
        rank: 1,
        examples: &[r#"{"name": "git_status", "arguments": {}}"#],
    },
    // ---- browser ----
    Entry {
        name: "browser_navigate",
        group: "browser",
        purpose: "Open a web page in the agent's browser and list what is on it.",
        keywords: &["open", "visit", "go", "url", "website", "link"],
        rank: 1,
        examples: &[r#"{"url": "https://example.com"}"#],
    },
    Entry {
        name: "browser_snapshot",
        group: "browser",
        purpose: "List what is visible on the current page, with a reference for each element.",
        keywords: &["look", "see", "element", "page"],
        rank: 1,
        examples: &[r#"{}"#, r#"{"full": true}"#],
    },
    Entry {
        name: "browser_find",
        group: "browser",
        purpose: "Find elements on the current page by their text, label or role.",
        keywords: &["locate", "element", "button"],
        rank: 2,
        examples: &[r#"{"query": "Sign in"}"#],
    },
    Entry {
        name: "browser_read_text",
        group: "browser",
        purpose: "Read the text of the current page, or of one element.",
        keywords: &["article", "content", "say"],
        rank: 1,
        examples: &[r#"{}"#, r#"{"ref": "e12"}"#],
    },
    Entry {
        name: "browser_click",
        group: "browser",
        purpose: "Click an element on the current page.",
        keywords: &["press", "button", "link"],
        rank: 1,
        examples: &[r#"{"ref": "e12"}"#, r#"{"x": 640, "y": 360}"#],
    },
    Entry {
        name: "browser_type",
        group: "browser",
        purpose: "Type text into a field on the current page.",
        keywords: &["enter", "input", "write", "field", "search box"],
        rank: 1,
        examples: &[
            r#"{"ref": "e7", "text": "weather in Paris"}"#,
            r#"{"ref": "e7", "text": "weather in Paris", "submit": true}"#,
        ],
    },
    Entry {
        name: "browser_fill_form",
        group: "browser",
        purpose: "Fill in several form fields at once, without submitting.",
        keywords: &["field", "checkbox", "dropdown"],
        rank: 2,
        examples: &[
            r#"{"fields": [{"ref": "e3", "value": "Ada"}, {"ref": "e4", "value": "ada@example.com"}]}"#,
        ],
    },
    Entry {
        name: "browser_press_key",
        group: "browser",
        purpose: "Press a key on the current page, such as Enter, Tab or Escape.",
        keywords: &["enter", "tab", "escape", "keyboard", "shortcut"],
        rank: 2,
        examples: &[r#"{"key": "Enter"}"#],
    },
    Entry {
        name: "browser_screenshot",
        group: "browser",
        purpose: "Take a picture of what the current page shows.",
        keywords: &["picture", "image", "capture", "look"],
        rank: 3,
        examples: &[r#"{}"#],
    },
    Entry {
        name: "browser_back",
        group: "browser",
        purpose: "Go back to the previous page.",
        keywords: &["previous", "return"],
        rank: 3,
        examples: &[r#"{}"#],
    },
    // ---- computer ----
    Entry {
        name: "computer_screenshot",
        group: "computer",
        purpose: "Take a picture of the screen to see what is on it.",
        keywords: &["picture", "image", "capture", "look", "see"],
        rank: 1,
        examples: &[r#"{}"#, r#"{"monitor": 2}"#],
    },
    Entry {
        name: "computer_click",
        group: "computer",
        purpose: "Click at a point on the screen.",
        keywords: &["button", "press", "double"],
        rank: 1,
        examples: &[
            r#"{"coordinate": [640, 360]}"#,
            r#"{"coordinate": [640, 360], "clicks": 2}"#,
        ],
    },
    Entry {
        name: "computer_move",
        group: "computer",
        purpose: "Move the mouse pointer to a point on the screen without clicking.",
        keywords: &["hover", "pointer"],
        rank: 3,
        examples: &[r#"{"coordinate": [200, 150]}"#],
    },
    Entry {
        name: "computer_drag",
        group: "computer",
        purpose: "Drag from one point on the screen to another.",
        keywords: &["drop", "select", "slider"],
        rank: 3,
        examples: &[r#"{"from": [100, 200], "to": [400, 200]}"#],
    },
    Entry {
        name: "computer_scroll",
        group: "computer",
        purpose: "Turn the mouse wheel at a point on the screen.",
        keywords: &["wheel", "down", "up"],
        rank: 2,
        examples: &[
            r#"{"coordinate": [640, 400], "direction": "down"}"#,
            r#"{"coordinate": [640, 400], "direction": "up", "amount": 5}"#,
        ],
    },
    Entry {
        name: "computer_type",
        group: "computer",
        purpose: "Type text into the application in front, as if on the keyboard.",
        keywords: &["keyboard", "enter", "write", "input"],
        rank: 1,
        examples: &[r#"{"text": "Hello"}"#],
    },
    Entry {
        name: "computer_key",
        group: "computer",
        purpose: "Press a key or a key combination in the application in front.",
        keywords: &["shortcut", "hotkey", "keyboard", "enter", "escape"],
        rank: 1,
        examples: &[r#"{"keys": "ctrl+s"}"#, r#"{"keys": "Enter"}"#],
    },
    // ---- outputs ----
    Entry {
        name: "read_output",
        group: "outputs",
        purpose: "Read more of a tool result that was shortened: a range of lines, or the lines that contain some text.",
        keywords: &["output", "full", "shortened", "truncated", "line", "rest"],
        rank: 1,
        examples: &[
            r#"{"id": 3, "lines": "1-80"}"#,
            r#"{"id": 3, "grep": "error", "context": 3}"#,
        ],
    },
    // ---- artifacts ----
    Entry {
        name: "create_artifact",
        group: "artifacts",
        purpose: "Create a document, web page, SVG picture or diagram that opens beside the chat.",
        keywords: &[
            "document", "report", "page", "html", "diagram", "chart", "svg", "mermaid", "draw",
            "make", "new",
        ],
        rank: 1,
        examples: &[
            r##"{"title": "Trip plan", "kind": "markdown", "content": "# Trip plan\n\n1. Book the train\n"}"##,
            r#"{"title": "Sign-up flow", "kind": "mermaid", "content": "flowchart TD\n  A[Start] --> B[Form]\n  B --> C[Done]"}"#,
        ],
    },
    Entry {
        name: "update_artifact",
        group: "artifacts",
        purpose: "Change an artifact: replace all of it, or one piece of text in it.",
        keywords: &["edit", "change", "fix", "revise", "correct"],
        rank: 1,
        examples: &[
            r#"{"id": 1, "old_text": "Book the train", "new_text": "Book the flight"}"#,
            r##"{"id": 1, "content": "# Trip plan\n\n1. Book the flight\n"}"##,
        ],
    },
    Entry {
        name: "read_artifact",
        group: "artifacts",
        purpose: "Read an artifact's current content, or an earlier version of it.",
        keywords: &["content", "version", "view", "open"],
        rank: 2,
        examples: &[r#"{"id": 1}"#, r#"{"id": 1, "version": 2}"#],
    },
    Entry {
        name: "list_artifacts",
        group: "artifacts",
        purpose: "List this chat's artifacts with their ids.",
        keywords: &["all", "id", "which"],
        rank: 2,
        examples: &[r#"{}"#],
    },
    // ---- search ----
    Entry {
        name: "web_search",
        group: "search",
        purpose: "Search the web and get numbered sources with excerpts to cite.",
        keywords: &[
            "internet",
            "online",
            "google",
            "look up",
            "latest",
            "news",
            "documentation",
            "docs",
        ],
        rank: 1,
        examples: &[
            r#"{"query": "tokio timeout example"}"#,
            r#"{"query": "how to set a timeout", "library": "tokio", "max_results": 5}"#,
        ],
    },
    Entry {
        name: "read_source",
        group: "search",
        purpose: "Read more of one source from an earlier web search.",
        keywords: &["more", "page", "excerpt", "cite", "details"],
        rank: 2,
        examples: &[r#"{"n": 1}"#, r#"{"n": 2, "query": "timeout"}"#],
    },
    // ---- task ----
    Entry {
        name: "update_task",
        group: "task",
        purpose: "Record the goal, the steps and the next action of the task this chat is working on.",
        keywords: &["goal", "plan", "step", "progress", "todo", "next"],
        rank: 1,
        examples: &[
            r#"{"goal": "Fix the failing date tests", "steps": [{"text": "Run the tests", "status": "doing"}, {"text": "Fix the parser", "status": "todo"}], "next_action": "Run the tests"}"#,
            r#"{"steps": [{"text": "Run the tests", "status": "done"}, {"text": "Fix the parser", "status": "doing"}], "next_action": "Fix the parser", "note": "Two tests fail in test_dates.py"}"#,
        ],
    },
    // ---- reverse (0.0.6 item 5, rea; the schemas are `reverse::allow`'s) ----
    Entry {
        name: "open_binary",
        group: "reverse",
        purpose: "Open a program or library file in rea so the other reverse engineering tools can analyse it.",
        keywords: &[
            "open",
            "decompile",
            "binary",
            "exe",
            "dll",
            "executable",
            "program",
        ],
        rank: 1,
        examples: &[r#"{"path": "hello.exe"}"#, r#"{"path": "bin/tool"}"#],
    },
    Entry {
        name: "close_binary",
        group: "reverse",
        purpose: "Close the file that is open in rea for this chat.",
        keywords: &["close"],
        rank: 1,
        examples: &[r#"{}"#],
    },
    Entry {
        name: "binary_session",
        group: "reverse",
        purpose: "Say which file is open in rea for this chat, with which decompiler.",
        keywords: &["session", "decompiler", "which"],
        rank: 2,
        examples: &[r#"{}"#],
    },
    Entry {
        name: "binary_overview",
        group: "reverse",
        purpose: "Summarise the open file: how many functions and strings, and its sections.",
        keywords: &["overview", "summary", "sections"],
        rank: 2,
        examples: &[r#"{}"#],
    },
    Entry {
        name: "list_procedures",
        group: "reverse",
        purpose: "List the functions found in the open file, in pages.",
        keywords: &["functions", "procedures"],
        rank: 1,
        examples: &[r#"{}"#],
    },
    Entry {
        name: "search_procedures",
        group: "reverse",
        purpose: "Find functions in the open file by name.",
        keywords: &["function", "find", "name"],
        rank: 1,
        examples: &[
            r#"{"pattern": "main"}"#,
            r#"{"pattern": "^crypt", "mode": "regex"}"#,
        ],
    },
    Entry {
        name: "procedure_pseudo_code",
        group: "reverse",
        purpose: "Show readable code (pseudocode) worked out for one function of the open file.",
        keywords: &["pseudocode", "decompile", "readable", "code", "source"],
        rank: 1,
        examples: &[r#"{"procedure": "main"}"#, r#"{"procedure": "0x401000"}"#],
    },
    Entry {
        name: "procedure_assembly",
        group: "reverse",
        purpose: "Show the machine instructions of one function of the open file.",
        keywords: &["assembly", "disassemble", "instructions", "asm"],
        rank: 2,
        examples: &[r#"{"procedure": "main"}"#],
    },
    Entry {
        name: "procedure_info",
        group: "reverse",
        purpose: "Show the facts known about one function of the open file.",
        keywords: &["signature", "facts"],
        rank: 2,
        examples: &[r#"{"procedure": "main"}"#],
    },
    Entry {
        name: "procedure_callers",
        group: "reverse",
        purpose: "List the functions that call one function.",
        keywords: &["callers", "called"],
        rank: 2,
        examples: &[r#"{"procedure": "check_license"}"#],
    },
    Entry {
        name: "procedure_callees",
        group: "reverse",
        purpose: "List the functions one function calls.",
        keywords: &["callees", "calls"],
        rank: 2,
        examples: &[r#"{"procedure": "main"}"#],
    },
    Entry {
        name: "procedure_references",
        group: "reverse",
        purpose: "List where one function is referred to.",
        keywords: &["references", "used"],
        rank: 2,
        examples: &[
            r#"{"procedure": "main"}"#,
            r#"{"procedure": "main", "direction": "outgoing"}"#,
        ],
    },
    Entry {
        name: "list_strings",
        group: "reverse",
        purpose: "List the text found in the open file, in pages.",
        keywords: &["strings", "text"],
        rank: 2,
        examples: &[r#"{}"#, r#"{"address": "0x404000"}"#],
    },
    Entry {
        name: "search_strings",
        group: "reverse",
        purpose: "Find text in the open file.",
        keywords: &["strings", "text", "message"],
        rank: 1,
        examples: &[r#"{"pattern": "password"}"#],
    },
    Entry {
        name: "list_names",
        group: "reverse",
        purpose: "List the named places (symbols) in the open file, in pages.",
        keywords: &["symbols", "names", "labels"],
        rank: 2,
        examples: &[r#"{}"#],
    },
    Entry {
        name: "list_segments",
        group: "reverse",
        purpose: "List the sections of the open file.",
        keywords: &["segments", "sections"],
        rank: 2,
        examples: &[r#"{}"#],
    },
    Entry {
        name: "xrefs",
        group: "reverse",
        purpose: "List what refers to one address in the open file.",
        keywords: &["xref", "references", "address"],
        rank: 2,
        examples: &[r#"{"address": "0x401000"}"#],
    },
    Entry {
        name: "find_xrefs_to_name",
        group: "reverse",
        purpose: "List what refers to one name in the open file.",
        keywords: &["xref", "references", "name"],
        rank: 2,
        examples: &[r#"{"name": "CreateFileW"}"#],
    },
    Entry {
        name: "get_call_graph",
        group: "reverse",
        purpose: "Show which functions call which, starting from one function.",
        keywords: &["call graph", "graph", "calls"],
        rank: 2,
        examples: &[
            r#"{"address": "main"}"#,
            r#"{"address": "0x401000", "direction": "backward"}"#,
        ],
    },
    Entry {
        name: "analyze_function",
        group: "reverse",
        purpose: "Summarise one function: its code, calls, text and references.",
        keywords: &["analyse", "analyze", "explain"],
        rank: 2,
        examples: &[r#"{"procedure": "main"}"#],
    },
    Entry {
        name: "batch_decompile",
        group: "reverse",
        purpose: "Show readable code for up to 20 functions at once.",
        keywords: &["decompile", "several", "batch"],
        rank: 2,
        examples: &[r#"{"addresses": ["main", "0x401200"]}"#],
    },
    Entry {
        name: "trace_call_path",
        group: "reverse",
        purpose: "Find a chain of calls from one function to another.",
        keywords: &["chain", "reach", "path"],
        rank: 2,
        examples: &[r#"{"start": "main", "goal": "send_report"}"#],
    },
    Entry {
        name: "trace_feature",
        group: "reverse",
        purpose: "Find a word in the open file's functions and text, and what refers to it.",
        keywords: &["feature", "where", "trace"],
        rank: 2,
        examples: &[r#"{"query": "license"}"#],
    },
    Entry {
        name: "inspect_analysis_view",
        group: "reverse",
        purpose: "Read more of an earlier rea result in this chat, by the id it gave.",
        keywords: &["more", "page", "next", "result"],
        rank: 2,
        examples: &[
            r#"{"source": {"kind": "retained-evidence", "evidence_id": "ev_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}, "view": {"kind": "summary"}}"#,
        ],
    },
    Entry {
        name: "inspect_pe_imports",
        group: "reverse",
        purpose: "List which functions a Windows program or library uses from other libraries.",
        keywords: &["imports", "dll", "windows"],
        rank: 3,
        examples: &[r#"{"path": "app.exe"}"#],
    },
    Entry {
        name: "inspect_pe_resources",
        group: "reverse",
        purpose: "List the resources (icons, version text, manifests) inside a Windows program.",
        keywords: &["resources", "icon", "version", "manifest"],
        rank: 3,
        examples: &[r#"{"path": "app.exe"}"#],
    },
    Entry {
        name: "inspect_pe_certificates",
        group: "reverse",
        purpose: "Show who signed a Windows program.",
        keywords: &["signed", "signature", "certificate", "publisher"],
        rank: 3,
        examples: &[r#"{"path": "app.exe"}"#],
    },
    Entry {
        name: "inspect_native_exports",
        group: "reverse",
        purpose: "List the functions a program or library offers to others.",
        keywords: &["exports", "exported"],
        rank: 3,
        examples: &[r#"{"path": "lib/plugin.dll"}"#],
    },
    Entry {
        name: "inspect_binary_hardening",
        group: "reverse",
        purpose: "Show which built-in protections a program was built with.",
        keywords: &["hardening", "protections", "aslr", "security"],
        rank: 3,
        examples: &[r#"{"path": "bin/server"}"#],
    },
    Entry {
        name: "inspect_go_binary",
        group: "reverse",
        purpose: "Show the Go version and modules a Go program was built from.",
        keywords: &["go", "golang", "modules"],
        rank: 3,
        examples: &[
            r#"{"path": "bin/tool"}"#,
            r#"{"path": "bin/tool", "architecture": "arm64"}"#,
        ],
    },
    Entry {
        name: "inspect_rust_binary",
        group: "reverse",
        purpose: "Show the Rust packages a Rust program was built from, when it records them.",
        keywords: &["rust", "crates", "packages"],
        rank: 3,
        examples: &[r#"{"path": "bin/tool"}"#],
    },
    Entry {
        name: "inspect_managed_artifact",
        group: "reverse",
        purpose: "Show what a .NET program or library is made of.",
        keywords: &["dotnet", "assembly", "managed"],
        rank: 3,
        examples: &[r#"{"path": "App.dll"}"#],
    },
    Entry {
        name: "inspect_managed_members",
        group: "reverse",
        purpose: "List the classes and methods of a .NET program or library.",
        keywords: &["dotnet", "classes", "methods"],
        rank: 3,
        examples: &[
            r#"{"path": "App.dll"}"#,
            r#"{"path": "App.dll", "type_names": ["App.Program"]}"#,
        ],
    },
    Entry {
        name: "inspect_python_pyc",
        group: "reverse",
        purpose: "Show what a compiled Python file (.pyc) contains.",
        keywords: &["pyc", "python", "bytecode"],
        rank: 3,
        examples: &[r#"{"path": "__pycache__/main.cpython-312.pyc"}"#],
    },
    Entry {
        name: "inspect_desktop_application",
        group: "reverse",
        purpose: "Find out what an installed app folder is built with (Electron, .NET, Unity and others).",
        keywords: &["installed", "app", "electron", "unity", "framework"],
        rank: 3,
        examples: &[r#"{"path": "MyApp"}"#],
    },
    Entry {
        name: "inspect_electron_fuses",
        group: "reverse",
        purpose: "Show the security switches (fuses) of an Electron app.",
        keywords: &["electron", "fuses"],
        rank: 3,
        examples: &[r#"{"path": "MyApp.app"}"#],
    },
    Entry {
        name: "analyze_javascript_application",
        group: "reverse",
        purpose: "Map the code of a JavaScript or Electron app folder or .asar file without running it.",
        keywords: &["javascript", "asar", "electron", "js"],
        rank: 3,
        examples: &[
            r#"{"input_path": "resources/app.asar"}"#,
            r#"{"input_path": "app", "source_paths": ["main.js"]}"#,
        ],
    },
];

/// The entry of a built-in tool.
pub fn entry(name: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.name == name)
}

/// A built-in group.
pub fn group(server: &str) -> Option<&'static Group> {
    GROUPS.iter().find(|g| g.server == server)
}

/// What a group is called: the built-in label, or an external server's own name.
pub fn label(server: &str) -> String {
    match group(server) {
        Some(g) => g.label.to_string(),
        None => policy::sanitize::one_line(server, 60),
    }
}

/// The names of the built-in servers of a host.
pub fn builtin_servers(host: &mcp_host::Host) -> BTreeSet<String> {
    host.servers()
        .into_iter()
        .filter(|s| s.kind == mcp_host::ServerKind::Builtin)
        .map(|s| s.name)
        .collect()
}

/// The entry for a registered tool, when it is a built-in one: an entry is only ever used
/// for the tool of that name on the server it was written for.
pub fn entry_for(tool: &RegisteredTool, builtin: bool) -> Option<&'static Entry> {
    if !builtin {
        return None;
    }
    entry(&tool.exposed).filter(|e| e.group == tool.server)
}

/// Checks one tool's arguments by the tool's name: `Ok`, or one message per problem.
pub type Validate<'a> = dyn Fn(&str, &Value) -> Result<(), Vec<String>> + 'a;

/// An entry's examples as JSON text, as far as they are argument objects that `fits`
/// accepts (the tool's schema as it is registered now). `rewrite` may change an example
/// first (a skill's real name in place of the one in the catalog). An example that does
/// not fit is left out rather than shown.
pub fn fitting_examples(
    entry: &Entry,
    rewrite: &dyn Fn(&str) -> String,
    fits: &dyn Fn(&Value) -> bool,
) -> Vec<String> {
    entry
        .examples
        .iter()
        .map(|text| rewrite(text))
        .filter(|text| {
            serde_json::from_str::<Value>(text).is_ok_and(|value| value.is_object() && fits(&value))
        })
        .collect()
}

/// How deep [`example_from_schema`] follows nested required objects.
const EXAMPLE_DEPTH: usize = 3;

/// An example argument object made from a schema's required fields, for a tool without
/// an entry. `None` when the schema gives too little to make one.
pub fn example_from_schema(schema: &Value) -> Option<Map<String, Value>> {
    match example_value(schema, EXAMPLE_DEPTH)? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// A value a schema position accepts, as far as its plain keywords say. `None` when the
/// position is too open to guess (several types, combinators).
fn example_value(schema: &Value, depth: usize) -> Option<Value> {
    let object = schema.as_object()?;
    if let Some(value) = object.get("const") {
        return Some(value.clone());
    }
    if let Some(value) = object.get("default") {
        return Some(value.clone());
    }
    if let Some(first) = object
        .get("examples")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
    {
        return Some(first.clone());
    }
    if let Some(first) = object
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
    {
        return Some(first.clone());
    }
    let kind = match object.get("type") {
        Some(Value::String(kind)) => kind.as_str(),
        None if object.contains_key("properties") => "object",
        _ => return None,
    };
    match kind {
        "string" => Some(Value::String("...".into())),
        "integer" | "number" => {
            let low = object.get("minimum").and_then(Value::as_i64);
            Some(Value::from(match (kind, low) {
                (_, Some(low)) => low,
                ("integer", None) => 1,
                _ => 0,
            }))
        }
        "boolean" => Some(Value::Bool(true)),
        "array" => {
            let wanted = object.get("minItems").and_then(Value::as_u64).unwrap_or(0);
            let item = object
                .get("items")
                .and_then(|items| example_value(items, depth.saturating_sub(1)));
            match (wanted, item) {
                (0, None) => Some(Value::Array(Vec::new())),
                (_, None) => None,
                (n, Some(item)) => Some(Value::Array(vec![item; n.clamp(1, 3) as usize])),
            }
        }
        "object" => {
            if depth == 0 {
                return None;
            }
            let properties = object.get("properties").and_then(Value::as_object);
            let mut out = Map::new();
            for name in object
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                let property = properties.and_then(|p| p.get(name))?;
                out.insert(name.to_string(), example_value(property, depth - 1)?);
            }
            Some(Value::Object(out))
        }
        _ => None,
    }
}

/// The most arguments [`arguments_line`] names.
const MAX_ARGUMENTS_LISTED: usize = 12;

/// The arguments of a tool on one line: name, type, and whether it is required. Required
/// arguments come first, in the schema's order; the others follow by name.
pub fn arguments_line(schema: &Value) -> String {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return "none".into();
    };
    if properties.is_empty() {
        return "none".into();
    }
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|name| properties.contains_key(*name))
        .collect();
    let mut optional: Vec<&str> = properties
        .keys()
        .map(String::as_str)
        .filter(|name| !required.contains(name))
        .collect();
    optional.sort_unstable();
    let total = required.len() + optional.len();
    let mut parts: Vec<String> = required
        .iter()
        .map(|name| (*name, true))
        .chain(optional.iter().map(|name| (*name, false)))
        .take(MAX_ARGUMENTS_LISTED)
        .map(|(name, is_required)| {
            let shown = policy::sanitize::one_line(name, 40);
            let mut notes: Vec<String> = type_words(&properties[name]).into_iter().collect();
            if is_required {
                notes.push("required".into());
            }
            if notes.is_empty() {
                shown
            } else {
                format!("{shown} ({})", notes.join(", "))
            }
        })
        .collect();
    if total > MAX_ARGUMENTS_LISTED {
        parts.push(format!("and {} more", total - MAX_ARGUMENTS_LISTED));
    }
    parts.join(", ")
}

/// A schema position's type in a few words, when it says one.
fn type_words(schema: &Value) -> Option<String> {
    let object = schema.as_object()?;
    if let Some(values) = object.get("enum").and_then(Value::as_array)
        && !values.is_empty()
        && values.len() <= 8
    {
        let list: Vec<String> = values
            .iter()
            .map(|v| policy::sanitize::one_line(&v.to_string(), 40))
            .collect();
        return Some(format!("one of {}", list.join(", ")));
    }
    let kind = match object.get("type") {
        Some(Value::String(kind)) => kind.clone(),
        Some(Value::Array(kinds)) => kinds
            .iter()
            .filter_map(Value::as_str)
            .filter(|kind| *kind != "null")
            .collect::<Vec<_>>()
            .join(" or "),
        _ => return None,
    };
    match kind.as_str() {
        "" => None,
        "array" => Some(
            match object
                .get("items")
                .and_then(|items| items.get("type"))
                .and_then(Value::as_str)
            {
                Some(item) => format!("list of {item}s"),
                None => "list".into(),
            },
        ),
        _ => Some(policy::sanitize::one_line(&kind, 40)),
    }
}

/// What is wrong between the catalog and the registered built-in tools: a built-in tool
/// without an entry, an entry under the wrong group, an example that does not fit the
/// tool's schema. Empty when all is well.
pub fn problems(
    tools: &[RegisteredTool],
    builtin_servers: &BTreeSet<String>,
    validate: &Validate<'_>,
) -> Vec<String> {
    let mut out = Vec::new();
    for tool in tools.iter().filter(|t| builtin_servers.contains(&t.server)) {
        let name = &tool.exposed;
        let Some(entry) = entry(name) else {
            out.push(format!(
                "{name} ({}): no entry in the tool catalog",
                tool.server
            ));
            continue;
        };
        if entry.group != tool.server {
            out.push(format!(
                "{name}: the catalog lists it under {}, but {} offers it",
                entry.group, tool.server
            ));
            continue;
        }
        if entry.examples.is_empty() {
            out.push(format!("{name}: the catalog has no example"));
        }
        for example in entry.examples {
            match serde_json::from_str::<Value>(example) {
                Ok(value) if value.is_object() => {
                    if let Err(errors) = validate(name, &value) {
                        out.push(format!(
                            "{name}: the example {example} does not fit its schema: {}",
                            errors.join("; ")
                        ));
                    }
                }
                _ => out.push(format!(
                    "{name}: the example {example} is not a JSON object"
                )),
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use mcp_host::{CompiledSchema, Host, HostOptions};
    use serde_json::json;

    use super::*;

    /// A host with every built-in server that can be built without the running app: the
    /// same servers `toolset::build_host` and `lib.rs` register, in the same order. The
    /// servers that need the app's database (`outputs`, `artifacts`, `search`, `task`)
    /// cannot be built here; their entries are checked against the agreed argument shapes
    /// below, and against the real tools by `tool_catalog_problems` in the running app
    /// (the `tool-selection` end-to-end scenario calls it).
    pub(crate) fn builtin_host() -> Host {
        let host = Host::new(HostOptions::default());
        host.add_builtin(Arc::new(tools::FilesServer::default()));
        host.add_builtin(Arc::new(tools::TerminalServer::new(
            tools::TerminalConfig::default(),
        )));
        host.add_builtin(Arc::new(tools::GitServer::new(tools::GitConfig {
            git: None,
            env: Vec::new(),
            credential_helpers: Vec::new(),
        })));
        host.add_builtin(Arc::new(tools::SkillsServer::new()));
        host.add_builtin(Arc::new(crate::subagent::SubagentsServer));
        host.add_builtin(Arc::new(crate::outside_agent::listing_both()));
        host.add_builtin(Arc::new(crate::tool_select::ToolFinderServer));
        host.add_builtin(Arc::new(browser::BrowserServer::new(
            browser::BrowserConfig::new(std::env::temp_dir().join("catalog-test-profile")),
            Arc::new(browser::DenyAll),
        )));
        let desktop = computer::platform::fake::FakeDesktop::new(vec![computer::MonitorInfo {
            id: 1,
            name: "Test".into(),
            bounds: computer::Rect::new(0.0, 0.0, 1000.0, 800.0),
            pixels_per_unit: 1.0,
            ui_scale: 1.0,
            primary: true,
        }]);
        host.add_builtin(Arc::new(computer::ComputerServer::new(
            desktop.capture(),
            desktop.injector(),
            computer::SelfProcess::new(1, Vec::new),
            computer::ComputerConfig::default(),
        )));
        // [0.0.6 item 5] rea's server, not installed: its 37 tools are always registered,
        // so every example is checked against the narrowed schemas.
        host.add_builtin(reverse::ReverseServer::new(
            reverse::ReverseLayout::new(std::env::temp_dir().join("catalog-test-reverse")),
            reverse::Platform::current(),
        ));
        host
    }

    /// The argument shapes agreed for the servers other features add (the shared spec's
    /// table of built-in tool servers), as schemas.
    fn agreed_shapes() -> Vec<(&'static str, &'static str, Value)> {
        let object = |properties: Value, required: Value| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
        vec![
            (
                "outputs",
                "read_output",
                object(
                    json!({"id": {"type": "integer"}, "lines": {"type": "string"},
                        "grep": {"type": "string"}, "context": {"type": "integer"}}),
                    json!(["id"]),
                ),
            ),
            (
                "artifacts",
                "create_artifact",
                object(
                    json!({"title": {"type": "string"},
                        "kind": {"type": "string", "enum": ["markdown", "html", "svg", "mermaid"]},
                        "content": {"type": "string"}}),
                    json!(["title", "kind", "content"]),
                ),
            ),
            (
                "artifacts",
                "update_artifact",
                json!({"type": "object",
                    "properties": {"id": {"type": "integer"}, "content": {"type": "string"},
                        "old_text": {"type": "string"}, "new_text": {"type": "string"}},
                    "required": ["id"],
                    "oneOf": [
                        {"required": ["content"], "not": {"anyOf": [{"required": ["old_text"]}, {"required": ["new_text"]}]}},
                        {"required": ["old_text", "new_text"], "not": {"required": ["content"]}}
                    ],
                    "additionalProperties": false}),
            ),
            (
                "artifacts",
                "read_artifact",
                object(
                    json!({"id": {"type": "integer"}, "version": {"type": "integer"}}),
                    json!(["id"]),
                ),
            ),
            ("artifacts", "list_artifacts", object(json!({}), json!([]))),
            (
                "search",
                "web_search",
                object(
                    json!({"query": {"type": "string"}, "library": {"type": "string"},
                        "max_results": {"type": "integer"}}),
                    json!(["query"]),
                ),
            ),
            (
                "search",
                "read_source",
                object(
                    json!({"n": {"type": "integer"}, "query": {"type": "string"}}),
                    json!(["n"]),
                ),
            ),
            (
                "task",
                "update_task",
                object(
                    json!({"goal": {"type": "string"},
                        "constraints": {"type": "array", "items": {"type": "string"}},
                        "steps": {"type": "array", "items": {"type": "object",
                            "properties": {"text": {"type": "string"},
                                "status": {"type": "string", "enum": ["todo", "doing", "done"]}},
                            "required": ["text", "status"], "additionalProperties": false}},
                        "next_action": {"type": "string"},
                        "status": {"type": "string", "enum": ["active", "done"]},
                        "note": {"type": "string"}}),
                    json!([]),
                ),
            ),
        ]
    }

    /// Every built-in tool that is registered has an entry under its own group, with
    /// examples its schema accepts.
    #[test]
    fn every_registered_built_in_tool_has_an_entry_with_valid_examples() {
        let host = builtin_host();
        let tools = host.tools();
        assert!(
            tools.len() >= 30,
            "the test host lost its servers: {}",
            tools.len()
        );
        let builtin = builtin_servers(&host);
        let found = problems(&tools, &builtin, &|name, args| host.validate(name, args));
        assert!(found.is_empty(), "{}", found.join("\n"));
        for tool in &tools {
            let entry =
                entry_for(tool, true).unwrap_or_else(|| panic!("no entry: {}", tool.exposed));
            assert_eq!(entry.group, tool.server, "{}", tool.exposed);
            let valid = fitting_examples(entry, &|text| text.to_string(), &|args| {
                host.validate(&tool.exposed, args).is_ok()
            });
            assert_eq!(valid.len(), entry.examples.len(), "{}", tool.exposed);
        }
    }

    /// The check itself notices what it is for.
    #[test]
    fn the_check_reports_a_missing_entry_and_a_bad_example() {
        let host = builtin_host();
        let builtin = builtin_servers(&host);
        let mut tools = host.tools();
        // A built-in tool nobody wrote an entry for.
        let mut unknown = tools[0].clone();
        unknown.exposed = "brand_new_tool".into();
        unknown.original = "brand_new_tool".into();
        tools.push(unknown);
        let found = problems(&tools, &builtin, &|name, args| host.validate(name, args));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("brand_new_tool") && found[0].contains("no entry"),
            "{found:?}"
        );
        // A schema the examples no longer fit.
        let tools = host.tools();
        let found = problems(&tools, &builtin, &|name, args| {
            if name == "read_file" {
                Err(vec!["path: expected integer, got string".into()])
            } else {
                host.validate(name, args)
            }
        });
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found
                .iter()
                .all(|p| p.contains("read_file") && p.contains("expected integer")),
            "{found:?}"
        );
        // An external server's tools are not the catalog's business.
        let mut external = host.tools()[0].clone();
        external.server = "weather".into();
        external.exposed = "weather__forecast".into();
        assert!(problems(&[external], &builtin, &|_, _| Ok(())).is_empty());
    }

    /// Entries for servers that are not registered in this test (they need the running
    /// app) fit the argument shapes those features agreed on.
    #[test]
    fn entries_for_servers_built_elsewhere_fit_the_agreed_shapes() {
        let host = builtin_host();
        let registered: BTreeSet<String> = host.tools().into_iter().map(|t| t.exposed).collect();
        let shapes = agreed_shapes();
        for (server, name, schema) in &shapes {
            let entry = entry(name).unwrap_or_else(|| panic!("no entry for {name}"));
            assert_eq!(entry.group, *server, "{name}");
            let compiled =
                CompiledSchema::compile(schema).unwrap_or_else(|e| panic!("{name}: {e}"));
            let valid = fitting_examples(entry, &|text| text.to_string(), &|args| {
                compiled.validate(args).is_ok()
            });
            assert_eq!(
                valid.len(),
                entry.examples.len(),
                "an example of {name} does not fit the agreed shape"
            );
        }
        // No entry escapes both checks: one that is not registered here has a shape above.
        for entry in ENTRIES {
            assert!(
                registered.contains(entry.name) || shapes.iter().any(|(_, n, _)| *n == entry.name),
                "{} is neither registered in the test host nor covered by an agreed shape",
                entry.name
            );
        }
    }

    #[test]
    fn entries_are_well_formed() {
        let mut seen = BTreeSet::new();
        for e in ENTRIES {
            assert!(seen.insert(e.name), "{} is listed twice", e.name);
            assert!(
                group(e.group).is_some(),
                "{}: unknown group {}",
                e.name,
                e.group
            );
            assert!((1..=3).contains(&e.rank), "{}: rank {}", e.name, e.rank);
            assert!(
                (1..=2).contains(&e.examples.len()),
                "{}: {} examples",
                e.name,
                e.examples.len()
            );
            let p = e.purpose;
            assert!(
                p.ends_with('.') && !p.contains('\n') && p.chars().count() <= 130,
                "{}: {p}",
                e.name
            );
            assert!(!p.contains('!'), "{}: {p}", e.name);
            for example in e.examples {
                let value: Value =
                    serde_json::from_str(example).unwrap_or_else(|err| panic!("{}: {err}", e.name));
                assert!(value.is_object(), "{}: {example}", e.name);
                assert!(
                    !example.contains('\n'),
                    "{}: an example spans lines",
                    e.name
                );
            }
            for k in e.keywords {
                assert_eq!(*k, k.to_lowercase(), "{}: keyword {k}", e.name);
            }
        }
        let mut servers = BTreeSet::new();
        for g in GROUPS {
            assert!(servers.insert(g.server), "{} is listed twice", g.server);
            assert!(!g.label.is_empty());
            for k in g.strong.iter().chain(g.weak) {
                assert_eq!(*k, k.to_lowercase(), "{}: keyword {k}", g.server);
            }
            assert!(
                ENTRIES.iter().any(|e| e.group == g.server),
                "{} has no tools",
                g.server
            );
        }
        assert_eq!(label("git"), "Git");
        assert_eq!(label("search"), "Web search");
        // An external server is called by its own name, made safe to print.
        assert_eq!(label("weather"), "weather");
        assert_eq!(label("a\nb"), "a\\nb");
    }

    /// An entry is only used for the built-in tool of that name on that server.
    #[test]
    fn an_external_tool_never_gets_a_built_in_entry() {
        let host = builtin_host();
        let read = host
            .tools()
            .into_iter()
            .find(|t| t.exposed == "read_file")
            .expect("read_file");
        assert_eq!(entry_for(&read, true).map(|e| e.name), Some("read_file"));
        assert!(entry_for(&read, false).is_none());
        let mut other = read.clone();
        other.server = "elsewhere".into();
        assert!(entry_for(&other, true).is_none());
    }

    #[test]
    fn an_example_is_made_from_the_required_fields() {
        let schema = json!({"type": "object",
            "properties": {
                "city": {"type": "string", "description": "City name"},
                "days": {"type": "integer", "minimum": 1, "maximum": 14},
                "units": {"type": "string", "enum": ["metric", "imperial"]},
                "detailed": {"type": "boolean"},
                "tags": {"type": "array", "items": {"type": "string"}, "minItems": 1},
                "where": {"type": "object", "properties": {"lat": {"type": "number"}, "lon": {"type": "number"}}, "required": ["lat", "lon"]},
                "optional": {"type": "string"}
            },
            "required": ["city", "days", "units", "detailed", "tags", "where"]});
        let example = example_from_schema(&schema).expect("an example");
        assert_eq!(
            Value::Object(example.clone()),
            json!({"city": "...", "days": 1, "units": "metric", "detailed": true, "tags": ["..."], "where": {"lat": 0, "lon": 0}})
        );
        // What it makes is valid for the schema.
        let compiled = CompiledSchema::compile(&schema).expect("schema");
        compiled.validate(&Value::Object(example)).expect("valid");
        // Nothing required: an empty object.
        let none = json!({"type": "object", "properties": {"q": {"type": "string"}}});
        assert_eq!(example_from_schema(&none), Some(Map::new()));
        // A default or the schema's own example is used when there is one.
        let given = json!({"type": "object", "properties": {
            "mode": {"type": "string", "default": "fast"},
            "path": {"type": "string", "examples": ["/tmp/a.txt"]},
            "n": {"type": "integer", "minimum": 5}},
            "required": ["mode", "path", "n"]});
        assert_eq!(
            example_from_schema(&given).map(Value::Object),
            Some(json!({"mode": "fast", "path": "/tmp/a.txt", "n": 5}))
        );
        // Too little to go on: no example rather than a wrong one.
        assert_eq!(
            example_from_schema(
                &json!({"type": "object", "required": ["x"], "properties": {"x": {"anyOf": [{"type": "string"}, {"type": "integer"}]}}})
            ),
            None
        );
        assert_eq!(example_from_schema(&json!("not a schema")), None);
    }

    #[test]
    fn the_arguments_line_names_type_and_required() {
        let schema = json!({"type": "object",
            "properties": {
                "message": {"type": "string"},
                "repo": {"type": "string"},
                "paths": {"type": "array", "items": {"type": "string"}},
                "kind": {"type": "string", "enum": ["markdown", "html"]},
                "limit": {"type": ["integer", "null"]},
                "where": {"type": "object"},
                "anything": {}
            },
            "required": ["message", "kind"]});
        assert_eq!(
            arguments_line(&schema),
            "message (string, required), kind (one of \"markdown\", \"html\", required), anything, limit (integer), paths (list of strings), repo (string), where (object)"
        );
        assert_eq!(
            arguments_line(&json!({"type": "object", "properties": {}})),
            "none"
        );
        assert_eq!(arguments_line(&json!({"type": "object"})), "none");
        // A server's property names are untrusted: shown on one line, and not without end.
        let odd = json!({"type": "object", "properties": {"a\nb": {"type": "string"}}});
        assert_eq!(arguments_line(&odd), "a\\nb (string)");
        let many: Map<String, Value> = (0..40)
            .map(|i| (format!("p{i:02}"), json!({"type": "string"})))
            .collect();
        let line = arguments_line(&json!({"type": "object", "properties": many}));
        assert!(line.ends_with("and 28 more"), "{line}");
    }

    /// [0.0.6 item 5] rea's server always offers exactly its 37 allow-listed tools, under
    /// rea's names and in the `reverse` group. Every other one of the pinned rea's 255
    /// tools does not exist for the model: absent from the tool list, and "no such tool"
    /// for a direct call and for one through `call_tool`. An allow-listed tool reaches the
    /// describer, which answers in plain words while rea is not set up.
    #[tokio::test]
    async fn rea_tools_outside_the_allow_list_do_not_exist() {
        let host = builtin_host();
        let tools = host.tools();
        let ours: Vec<&str> = tools
            .iter()
            .filter(|t| t.server == "reverse")
            .map(|t| t.exposed.as_str())
            .collect();
        assert_eq!(ours, reverse::allow::MODEL_TOOLS.to_vec());
        for t in tools.iter().filter(|t| t.server == "reverse") {
            assert_eq!(t.exposed, t.original, "rea's own name");
            assert!(t.schema_error.is_none(), "{}", t.exposed);
            assert_eq!(
                group(&t.server).map(|g| g.label),
                Some("Reverse engineering")
            );
        }
        let fixture: Value = serde_json::from_str(include_str!(
            "../../crates/reverse/tests/fixtures/rea-6.9.0-catalog.json"
        ))
        .unwrap();
        let chat = mcp_host::ChatContext {
            chat_id: 1,
            full_access: true,
            approved_folders: vec!["/".into()],
            ..Default::default()
        };
        let routing = crate::tool_select::finder::Routing {
            host: &host,
            shown: &tools,
            hidden: &[],
        };
        let mut checked = 0;
        for name in fixture["tool_names"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .filter(|n| !reverse::allow::MODEL_TOOLS.contains(n))
        {
            assert!(
                !tools.iter().any(|t| t.exposed == name),
                "{name} is registered"
            );
            match host
                .describe(name, &Map::new(), &chat, mcp_host::ToolPrefs::default())
                .await
            {
                mcp_host::Described::Invalid(why) => {
                    assert!(why.contains("there is no tool named"), "{name}: {why}")
                }
                mcp_host::Described::Action(a) => panic!("{name}: {a:?}"),
            }
            let call = json!({"name": name, "arguments": {"path": "/etc/passwd"}});
            match routing
                .describe(
                    crate::tool_select::finder::CALL_TOOL,
                    call.as_object().unwrap(),
                    &chat,
                    &std::collections::HashMap::new(),
                )
                .await
            {
                mcp_host::Described::Invalid(why) => {
                    assert!(why.contains("There is no tool named"), "{name}: {why}")
                }
                mcp_host::Described::Action(a) => panic!("{name}: {a:?}"),
            }
            checked += 1;
        }
        assert_eq!(checked, 255 - 37);
        // An allow-listed tool reaches rea's describer.
        let args = json!({"path": "hello.exe"});
        match host
            .describe(
                "open_binary",
                args.as_object().unwrap(),
                &chat,
                mcp_host::ToolPrefs::default(),
            )
            .await
        {
            mcp_host::Described::Invalid(why) => {
                assert_eq!(why, reverse::describe::messages::NOT_SET_UP)
            }
            mcp_host::Described::Action(a) => panic!("{a:?}"),
        }
        // A path left out fails the schema before anything is described (the two .NET
        // checks would otherwise reuse an earlier file).
        for name in ["inspect_managed_members", "inspect_managed_artifact"] {
            assert!(host.validate(name, &json!({})).is_err(), "{name}");
            let call = json!({"name": name, "arguments": {}});
            assert!(
                routing.unwrap_call(call.as_object().unwrap()).is_err(),
                "{name}"
            );
        }
    }

    /// [0.0.6 item 5] A request about a program file points at the `reverse` group, through
    /// its words or a file name ending in .exe or .dll.
    #[test]
    fn a_request_about_a_program_points_at_reverse_engineering() {
        let host = builtin_host();
        let candidates: Vec<crate::tool_select::Candidate> = host
            .tools()
            .iter()
            .map(|t| crate::tool_select::Candidate::of(t, true))
            .collect();
        for request in [
            "Open notepad.exe and show me the readable code of its first function.",
            "Decompile hello-linux-x64",
            "Which functions from other libraries does helper.dll use?",
            "Can you reverse engineer this binary?",
            "show the pseudocode of main",
        ] {
            let groups = crate::tool_select::matching_groups(
                &candidates,
                &crate::tool_select::Wording::of(request),
                &[],
            );
            assert!(
                groups.iter().any(|(g, _)| g == "reverse"),
                "{request}: {groups:?}"
            );
        }
    }

    /// [0.0.6 item 5] rea working on a chat's open file never asks: its audit row says "no
    /// approval needed", like the app's own data, except in Full access, where the mode is
    /// decided first and recorded. Opening a file is an ordinary grantable read.
    #[test]
    fn rea_session_work_is_audited_as_needing_no_approval() {
        use agent::history::{CallRecord, CallStatus, ResultRecord};
        use agent::turn::CallDecision;
        let call = CallRecord {
            id: "c1".into(),
            name: "procedure_pseudo_code".into(),
            arguments: serde_json::from_value(json!({"procedure": "main"})).unwrap(),
            recovered: false,
        };
        let result = ResultRecord::synthetic(&call, CallStatus::Ok, Vec::new());
        let action = |scope: policy::Scope| policy::Action {
            server: "reverse".into(),
            tool: "procedure_pseudo_code".into(),
            summary: String::new(),
            preview: String::new(),
            scope,
            risk: policy::Risk::Normal,
            read_only: true,
            outbound: false,
            keep_asking: None,
            network: false,
            hard_limit: None,
            payload: Value::Null,
        };
        let session = action(policy::Scope::FileAnalysisSession);
        let (label, entry) = crate::turns::audit_entry(
            7,
            json!({"procedure": "main"}),
            &result,
            &CallDecision::AllowedByGrant,
            Some(&session),
            3,
        );
        assert_eq!(label, "no approval needed");
        assert_eq!(entry.decision, store::AuditDecision::AllowedWithoutPrompt);
        let (label, entry) = crate::turns::audit_entry(
            7,
            json!({"procedure": "main"}),
            &result,
            &CallDecision::AllowedByFullAccess,
            Some(&session),
            3,
        );
        assert_eq!(label, "allowed by Full access");
        assert_eq!(
            entry.note.as_deref(),
            Some(crate::approvals::FULL_ACCESS_NOTE)
        );
        // Opening a file under a grant is recorded as the grant.
        let open = action(policy::Scope::FileAnalysis {
            folder: "/work".into(),
        });
        let (label, entry) = crate::turns::audit_entry(
            7,
            json!({"path": "/work/a.exe"}),
            &result,
            &CallDecision::AllowedByGrant,
            Some(&open),
            3,
        );
        assert_eq!(label, "allowed by an earlier grant");
        assert_eq!(entry.decision, store::AuditDecision::AllowedByGrant);
    }
}
