//! Every row of the coding-assistant rule (0.0.5), in both directions and both approval
//! modes: what asks with Allow once / Deny and counts as going online, and what stays an
//! ordinary command.

use super::*;
use crate::allow_all::command_facts;
use crate::command::{Shell, classify, parse};
use crate::engine::{
    Action, AskReason, CardKind, Decision, Grants, Risk, Scope, TurnContext, evaluate,
};

const SH: Shell = Shell::Posix;
const PS: Shell = Shell::PowerShell;

/// Grants for everything: what this rule finds must ask whatever was granted.
struct Everything;
impl Grants for Everything {
    fn covers(&self, _: &Scope) -> bool {
        true
    }
}

/// The action the terminal describer builds for a line (see `tools::terminal`).
fn command_action(shell: Shell, line: &str) -> Action {
    let verdict = classify(shell, line);
    Action {
        server: "terminal".into(),
        tool: "run_command".into(),
        summary: line.into(),
        preview: line.into(),
        scope: Scope::Command {
            program: "p".into(),
            args: vec![line.into()],
            cwd: "/work".into(),
        },
        risk: if verdict.always_ask.is_some() {
            Risk::AlwaysAsk
        } else {
            Risk::Normal
        },
        read_only: verdict.read_only,
        outbound: true,
        keep_asking: verdict.keep_asking.map(|(kind, _)| kind),
        network: verdict.network,
        hard_limit: None,
        payload: serde_json::Value::Null,
    }
}

fn turn(allow_all: bool, untrusted: bool) -> TurnContext {
    TurnContext {
        tool_enabled: true,
        server_ready: true,
        untrusted_in_turn: untrusted,
        allow_all,
        ..Default::default()
    }
}

/// The line starts a coding assistant (or turns off one's checks): Ask first shows Allow
/// once / Deny whatever was granted, Allow all asks too, before and after untrusted
/// content, and the line counts as going online.
fn assert_asks(shell: Shell, line: &str) {
    let verdict = classify(shell, line);
    let facts = command_facts(shell, line, &parse(shell, line));
    assert!(facts.coding_app.is_some(), "{shell:?} {line:?}: not seen");
    assert!(
        verdict.always_ask.is_some(),
        "{shell:?} {line:?}: not always-ask"
    );
    assert!(verdict.keep_asking.is_some(), "{shell:?} {line:?}: no keep-asking");
    assert!(verdict.network, "{shell:?} {line:?}: not online");
    assert!(!verdict.read_only, "{shell:?} {line:?}: read-only");
    let action = command_action(shell, line);
    for untrusted in [false, true] {
        assert_eq!(
            evaluate(&action, &turn(false, untrusted), &Everything),
            Decision::Ask {
                card: CardKind::OnceOnly,
                reason: AskReason::AlwaysAsk
            },
            "Ask first, {shell:?} {line:?}"
        );
        assert_eq!(
            evaluate(&action, &turn(true, untrusted), &Everything),
            Decision::Ask {
                card: CardKind::OnceOnly,
                reason: AskReason::StillAsksInAllowAll
            },
            "Allow all, {shell:?} {line:?}"
        );
    }
}

/// The line is not a coding assistant: the rule finds nothing.
fn assert_not_seen(shell: Shell, line: &str) {
    let facts = command_facts(shell, line, &parse(shell, line));
    assert_eq!(facts.coding_app, None, "{shell:?} {line:?}");
}

/// (a) The program is a coding assistant, by name, path or Windows form.
#[test]
fn a_the_program_is_a_coding_assistant() {
    for line in [
        "claude",
        "claude -p 'fix the failing test'",
        "claude --print \"fix the failing test\"",
        "claude-code -p x",
        "/usr/local/bin/claude -p x",
        "./claude -p x",
        "codex",
        "codex exec 'refactor utils.py'",
        "codex exec - ",
        "gemini -p hello",
        "qwen",
        "aider --message hello",
        "opencode run hello",
        "cursor-agent -p hello",
        "goose run --text hello",
        "amp",
        "crush",
        "copilot -p hello",
        "q chat",
        "ls && claude -p next",
        "git status; codex exec next",
        "CLAUDE -p x",
    ] {
        assert_asks(SH, line);
    }
    for line in [
        "claude -p 'fix it'",
        "claude.exe -p 'fix it'",
        "claude.cmd -p x",
        "codex.cmd exec x",
        "codex.exe exec x",
        "C:\\Users\\me\\AppData\\Roaming\\npm\\claude.cmd -p x",
        "C:\\Users\\me\\.local\\bin\\claude.exe -p x",
        "claude.ps1 -p x",
        "gemini.cmd -p x",
        "Get-ChildItem; codex exec x",
        "claude -p \"fix it\"",
    ] {
        assert_asks(PS, line);
    }
}

/// (a) After a wrapper, or in the command a shell is handed.
#[test]
fn a_after_a_wrapper_or_inside_a_shell() {
    for line in [
        "env FOO=1 claude -p x",
        "env -i PATH=/usr/bin claude -p x",
        "timeout 600 codex exec x",
        "nohup claude -p x",
        "nice -n 5 claude -p x",
        "time codex exec x",
        "exec claude -p x",
        "command claude -p x",
        "xargs claude -p",
        "stdbuf -oL claude -p x",
        "setsid claude -p x",
        "flock /tmp/lock claude -p x",
        "sudo claude -p x",
        "doas codex exec x",
        "env npx claude",
        "sudo env timeout 9 npx @openai/codex exec x",
        "timeout 60 node /usr/lib/node_modules/@anthropic-ai/claude-code/cli.js -p x",
        "sh -c 'claude -p x'",
        "bash -lc 'codex exec x'",
        "zsh -c \"env FOO=1 claude -p x\"",
        "sh -c 'npx @anthropic-ai/claude-code -p x'",
    ] {
        assert_asks(SH, line);
    }
    for line in [
        "cmd /c claude -p x",
        "cmd /c \"claude -p x\"",
        "Start-Process claude -ArgumentList x",
        "Start-Process -FilePath codex.exe -ArgumentList exec",
        "powershell -Command claude -p x",
        "pwsh -c 'codex exec x'",
        "wsl claude -p x",
        "Invoke-Expression 'claude -p x'",
    ] {
        assert_asks(PS, line);
    }
}

/// (b) A package runner handed an assistant's package, in every form a runner takes it.
#[test]
fn b_a_package_runner_handed_an_assistant() {
    for line in [
        "npx claude",
        "npx -y @anthropic-ai/claude-code -p x",
        "npx --yes @anthropic-ai/claude-code@latest -p x",
        "npx @openai/codex@0.1.2 exec x",
        "npx --package=@openai/codex codex exec x",
        "npx -p @openai/codex codex exec x",
        "npm exec @openai/codex -- exec x",
        "npm exec -- claude -p x",
        "npm x claude",
        "pnpm dlx @anthropic-ai/claude-code -p x",
        "pnpx claude",
        "yarn dlx @openai/codex exec x",
        "bunx @google/gemini-cli",
        "bun x @anthropic-ai/claude-code",
        "uvx --from aider-chat aider",
        "uvx aider-chat",
        "uvx aider-chat==0.80.0",
        "uv tool run aider-chat",
        "pipx run aider-chat",
        "pipx run --spec aider-chat aider",
        "npx @qwen-code/qwen-code",
        "npx opencode-ai run x",
        "npx @sourcegraph/amp",
        "npx @charmland/crush",
        "npx @github/copilot -p x",
    ] {
        assert_asks(SH, line);
    }
    for line in [
        "npx.cmd claude",
        "npx @anthropic-ai/claude-code -p x",
        "npm.cmd exec @openai/codex -- exec x",
    ] {
        assert_asks(PS, line);
    }
}

/// (c) `node`, `bun` or `deno` handed a script inside Claude Code's or Codex's package.
#[test]
fn c_a_script_inside_an_assistants_package() {
    for line in [
        "node /usr/lib/node_modules/@anthropic-ai/claude-code/cli.js -p x",
        "node /opt/homebrew/lib/node_modules/@openai/codex/bin/codex.js exec x",
        "nodejs /usr/lib/node_modules/@anthropic-ai/claude-code/cli.js",
        "bun ~/.bun/install/global/node_modules/@openai/codex/bin/codex.js exec x",
        "deno run -A /x/node_modules/@anthropic-ai/claude-code/cli.js",
    ] {
        assert_asks(SH, line);
    }
    for line in [
        "node C:\\Users\\me\\AppData\\Roaming\\npm\\node_modules\\@openai\\codex\\bin\\codex.js exec x",
        "node.exe C:\\Users\\me\\AppData\\Roaming\\npm\\node_modules\\@anthropic-ai\\claude-code\\cli.js -p x",
    ] {
        assert_asks(PS, line);
    }
}

/// (d) A token that turns off an assistant's permission checks, anywhere on the line,
/// whatever the program, in any letter case, quoted or split by quotes.
#[test]
fn d_a_permission_bypass_token_anywhere() {
    for token in BYPASS_TOKENS {
        for line in [
            format!("some-tool {token}"),
            format!("echo {token}"),
            format!("x --permission-mode {token}"),
            format!("x --flag={token}"),
            format!("x '{token}'"),
            format!("x {}", token.to_uppercase()),
            format!("cat file | x {token} > out.txt"),
        ] {
            assert_asks(SH, &line);
            assert_asks(PS, &line);
        }
    }
    for line in [
        "x --dangerously-skip-perm''issions",
        "x --dangerously-skip-permiss\\ions",
        "x -c sandbox_mode=danger-full-access",
        "x -c 'sandbox_mode=\"danger-full-access\"'",
        "x --settings '{\"permissions\":{\"defaultMode\":\"bypassPermissions\"}}'",
    ] {
        assert_asks(SH, line);
    }
    assert_asks(PS, "x --dangerously-skip-perm`issions");
    assert_eq!(bypass_token("claude --yolo"), Some("--yolo"));
    assert_eq!(bypass_token("claude -p hi"), None);
}

/// A line that cannot be broken down is read as text: the first word of each command in
/// it, and a script inside an assistant's package anywhere.
#[test]
fn lines_that_cannot_be_broken_down() {
    // A pipe between plain commands is still plain; the rest are not.
    assert_asks(SH, "echo task | claude -p");
    assert_asks(SH, "cat task.txt | codex exec -");
    for line in [
        "claude -p \"$TASK\"",
        "FOO=1 claude -p x > out.txt",
        "claude -p x 2>&1 | tee log.txt",
        "if true; then claude -p x; fi",
        "(cd /p && codex exec x)",
        "claude -p \"$(cat task.txt)\"",
        "echo $(claude -p x)",
        "echo `codex exec x`",
        "x=$(npx @openai/codex exec y)",
        "for f in *.py; do aider $f; done",
        "node ~/.npm-global/lib/node_modules/@anthropic-ai/claude-code/cli.js -p x",
    ] {
        assert!(
            matches!(parse(SH, line), Parsed::Complex { .. }),
            "{line:?} should not be plain"
        );
        assert_asks(SH, line);
    }
    for line in [
        "& claude -p $task",
        "& 'C:\\Program Files\\Claude\\claude.exe' -p x",
        "$t = 'x'; codex exec $t",
        "claude -p $task | Out-File log.txt",
    ] {
        assert_asks(PS, line);
    }
}

/// Ordinary commands that mention these names, or look a little like them, stay ordinary.
#[test]
fn ordinary_commands_are_not_seen() {
    for line in [
        "echo claude",
        "grep -r codex src",
        "cat CLAUDE.md",
        "ls .claude",
        "git log --grep claude",
        "which claude",
        "npx vitest run",
        "npx tsc --noEmit",
        "npm install",
        "npm exec vitest",
        "npm run claude",
        "node scripts/build.js",
        "node scripts/claude.js",
        "python3 -c print(1)",
        "code .",
        "qemu-img info disk.qcow2",
        "amplify status",
        "claudette run",
        "codex-review src",
        "gemini-tool",
        "cargo test",
        "pytest -q",
        "echo q",
        "cp notes.txt claude-notes.txt",
        "uvx ruff check",
        "pipx run black .",
        "env FOO=1 cargo test",
        "timeout 60 cargo test",
        "sh -c 'cargo test'",
        "echo full auto",
        "git commit -m 'yolo'",
    ] {
        assert_not_seen(SH, line);
    }
    for line in [
        "Get-Content CLAUDE.md",
        "Select-String claude notes.txt",
        "npx eslint .",
        "Start-Process notepad",
        "cmd /c dir",
    ] {
        assert_not_seen(PS, line);
    }
    // Still an ordinary verdict otherwise: a plain test run stays grantable.
    assert!(classify(SH, "npx vitest run").always_ask.is_none());
    assert!(classify(SH, "which claude").read_only);
}

/// Known limit, kept visible: a renamed copy or a link is not seen. The line that makes
/// the link names the program as an argument only, and the link runs under its own name.
#[test]
fn a_renamed_copy_or_link_is_a_known_gap() {
    assert_not_seen(SH, "ln -s /usr/local/bin/claude helper");
    assert_not_seen(SH, "./helper -p 'fix it'");
    assert_not_seen(SH, "cp /usr/local/bin/codex ./tool");
    // A bypass token still gives such a line away.
    assert_asks(SH, "./helper -p 'fix it' --dangerously-skip-permissions");
}

/// A long line of nested wrappers is checked in bounded time.
#[test]
fn deep_nesting_stays_bounded() {
    let line = "env ".repeat(3000) + "cargo test";
    let started = std::time::Instant::now();
    let _ = command_facts(SH, &line, &parse(SH, &line));
    let nested = format!("{}claude -p x", "env ".repeat(4));
    assert_asks(SH, &nested);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

/// The why names what was found, for the card.
#[test]
fn the_reason_names_what_was_found() {
    let why = |line: &str| find(line, &parse(SH, line)).unwrap_or_default();
    assert!(why("claude -p x").contains("claude is a coding assistant"), "{}", why("claude -p x"));
    assert!(why("npx @openai/codex exec").contains("@openai/codex"));
    assert!(why("x --yolo").contains("--yolo"));
    assert!(why("timeout 9 codex exec").contains("codex"));
}
