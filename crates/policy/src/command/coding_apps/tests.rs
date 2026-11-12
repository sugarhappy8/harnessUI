//! Every row of the coding-assistant rule (0.0.6), in both directions and in the three
//! approval modes: what asks with Allow once / Deny in Ask first and Allow all, runs without
//! a card in Full access (D12) and counts as going online, and what stays an ordinary
//! command.

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

/// Full access (0.0.5, D12): the user chose that nothing asks, with or without Allow all.
fn full_access(allow_all: bool, untrusted: bool) -> TurnContext {
    TurnContext {
        full_access: true,
        ..turn(allow_all, untrusted)
    }
}

/// The line starts a coding assistant (or turns off one's checks): Ask first shows Allow
/// once / Deny whatever was granted, Allow all asks too, before and after untrusted
/// content, Full access runs it without a card (the terminal reaches everything there,
/// D12), and the line counts as going online.
fn assert_asks(shell: Shell, line: &str) {
    let verdict = classify(shell, line);
    let facts = command_facts(shell, line, &parse(shell, line));
    assert!(facts.coding_app.is_some(), "{shell:?} {line:?}: not seen");
    assert!(
        verdict.always_ask.is_some(),
        "{shell:?} {line:?}: not always-ask"
    );
    assert!(
        verdict.keep_asking.is_some(),
        "{shell:?} {line:?}: no keep-asking"
    );
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
        for allow_all in [false, true] {
            assert_eq!(
                evaluate(&action, &full_access(allow_all, untrusted), &Everything),
                Decision::AllowedByFullAccess,
                "Full access, {shell:?} {line:?}"
            );
        }
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

/// (a) After a program that changes how another program runs (processor, priority,
/// limits, terminal, sandbox, namespace, tracing, clock) and then runs it, and after
/// `find -exec` / `fd -x`, which hand each file to a program (review of 0.0.6, round 1).
#[test]
fn a_after_programs_that_run_another_program() {
    for line in [
        "taskset -c 0 claude -p hi --allowedTools Bash",
        "numactl --cpunodebind=0 claude -p x",
        "chrt -f 10 codex exec x",
        "prlimit --nofile=100 claude -p x",
        "unbuffer claude -p x",
        "rlwrap claude",
        "tmux new-session -d 'claude -p x'",
        "tmux new -d codex exec x",
        "screen -dmS x claude -p x",
        "firejail claude -p x",
        "bwrap --bind / / claude -p x",
        "systemd-run --user --pty claude -p x",
        "arch -arm64 claude -p x",
        "dbus-run-session claude -p x",
        "xvfb-run claude -p x",
        "unshare -r claude -p x",
        "nsenter -t 1 -m claude -p x",
        "runuser -u me -- claude -p x",
        "sg staff 'claude -p x'",
        "strace -f claude -p x",
        "ltrace claude -p x",
        "faketime '2020-01-01' claude -p x",
        "corepack pnpm dlx @anthropic-ai/claude-code -p x",
        "corepack yarn claude",
        "find . -name x -exec claude -p {} +",
        "find . -execdir codex exec x +",
        "find . -exec claude -p x ;",
        "find . -exec claude -p x \\;",
        "find . -type f -exec echo {} \\; -exec claude -p {} \\;",
        "find . -type f -exec echo {} ';' -exec claude -p {} ';'",
        "find . -ok codex exec x ;",
        "fd -x claude -p",
        "fd . -X codex exec",
        "fd --exec claude -p",
        "fdfind -x claude -p",
        // More runners (review of 0.0.6, round 2): Python and Ruby environments, version
        // and environment managers, npm tools and other wrappers.
        "uv run claude -p x",
        "uv run --with x claude -p x",
        "poetry run claude -p x",
        "pipenv run claude -p x",
        "pdm run claude -p x",
        "hatch run claude -p x",
        "conda run claude -p x",
        "conda run -n env claude -p x",
        "mamba run -n env codex exec x",
        "micromamba run -n env claude -p x",
        "pixi run claude -p x",
        "bundle exec claude -p x",
        "rye run claude -p x",
        "mise exec -- claude -p x",
        "mise x -- claude -p x",
        "rtx exec -- claude -p x",
        "asdf exec claude -p x",
        "nvm exec 20 claude -p x",
        "nvm run 20 codex exec x",
        "volta run claude -p x",
        "fnm exec -- claude -p x",
        "direnv exec . claude -p x",
        "nix-shell -p nodejs --run 'claude -p x'",
        "nix-shell --command 'codex exec x'",
        "nix develop -c claude -p x",
        "nix develop --command codex exec x",
        "nix shell nixpkgs#nodejs -c claude -p x",
        "nix run nixpkgs#claude-code",
        "nix run github:x/y#codex -- exec x",
        "devbox run claude -p x",
        "distrobox enter -- claude -p x",
        "toolbox run claude -p x",
        "cross-env FOO=1 claude -p x",
        "cross-env-shell 'claude -p x'",
        "dotenv -- claude -p x",
        "dotenvx run -- claude -p x",
        "env-cmd -f .env claude -p x",
        "nodemon --exec 'claude -p x'",
        "concurrently 'claude -p x'",
        "concurrently 'npm:watch' 'codex exec x'",
        "watchexec -- claude -p x",
        "valgrind claude -p x",
        "gdb --args claude -p x",
        "lldb -- claude -p x",
        "torsocks claude -p x",
        "proxychains4 claude -p x",
        "proxychains -q claude -p x",
        "fakeroot claude -p x",
        "proot claude -p x",
        "ssh-agent claude -p x",
        "dbus-launch claude -p x",
        "setarch x86_64 claude -p x",
        "chronic claude -p x",
        "ifne claude -p x",
        "cpulimit -l 50 -- claude -p x",
        "entr claude -p x",
        "lckdo /tmp/l claude -p x",
        "vglrun claude -p x",
        "prime-run claude -p x",
        "gamemoderun claude -p x",
        "nq claude -p x",
        "ts claude -p x",
        "tsp claude -p x",
        "pueue add -- claude -p x",
        "expect -c 'spawn claude -p x; interact'",
        "hyperfine 'claude -p x'",
        "sandbox-exec -f p.sb claude -p x",
        "gosu me claude -p x",
        "su-exec me claude -p x",
        "setpriv --reuid=1000 claude -p x",
        "systemd-nspawn -D /x claude -p x",
        "perf stat claude -p x",
        "xterm -e claude -p x",
        "gnome-terminal -- claude -p x",
        "winpty claude -p x",
        "zellij run -- claude -p x",
        "dtach -c /tmp/s claude -p x",
        // A runner after a wrapper.
        "env nix run nixpkgs#claude-code",
        "sudo -u me kubectl exec pod -- codex exec x",
        "timeout 60 uv run claude -p x",
    ] {
        assert_asks(SH, line);
    }
    for line in [
        "conhost claude -p x",
        "conhost.exe claude -p x",
        "wt claude -p x",
        "wt.exe -w 0 nt claude -p x",
        "wt new-tab codex exec x",
    ] {
        assert_asks(PS, line);
    }
    for line in [
        "uv run pytest",
        "uv run --with rich python app.py",
        "poetry run python app.py --model claude",
        "bundle exec rake",
        "conda run -n env python x.py",
        "nvm exec 20 node app.js",
        "mise exec -- cargo test",
        "nix develop -c cargo build",
        "nix run nixpkgs#hello",
        "cross-env NODE_ENV=production webpack",
        "hyperfine 'cargo build'",
        "concurrently 'npm:watch' 'npm:serve'",
        "nodemon --exec 'cargo run'",
        "valgrind ./target/debug/app",
        "gdb --args ./app --name claude",
        "wt -w 0 nt pwsh",
    ] {
        assert_not_seen(SH, line);
    }
    for line in [
        "taskset -c 0 cargo test",
        "tmux new-session -d 'cargo test'",
        "strace -o claude.log ls",
        "find . -name '*.rs' -exec wc -l {} +",
        "find . -name claude",
        "fd -x wc -l",
        "fd claude",
        "corepack enable",
    ] {
        assert_not_seen(SH, line);
    }
}

/// (a) A name the POSIX shell runs as written once its escapes and quotes are taken away
/// (`cod\ex` is `codex`, `$'claude'` is `claude`): every reading of a line that cannot be
/// broken down is checked (review of 0.0.6, round 1).
#[test]
fn a_a_name_with_escapes_or_quotes_inside() {
    for line in [
        "cod\\ex exec hi",
        "c\\laude -p hi --allowedTools Bash",
        "co\\dex exec x",
        "\\claude -p x",
        "cl''aude -p x",
        "\"claude\" -p x",
        "$'claude' -p x",
        "$\"codex\" exec x",
        "echo task | cl\\aude -p",
    ] {
        assert_asks(SH, line);
    }
    // A Windows path keeps its backslashes and is still read as one.
    assert_asks(PS, "& 'C:\\Users\\me\\.local\\bin\\claude.exe' -p x");
    assert_not_seen(SH, "echo c\\laude");
    assert_not_seen(SH, "cat no\\tes.txt > out.txt");
}

/// (a) A name the shell joins from two lines (a backslash at the end of a line), a name
/// written as a pattern the shell expands (`cl?ude`, `clau*`, `cla[u]de`, `{claude,}`), and
/// cmd's caret escape (`c^laude`) (review of 0.0.6, round 2).
#[test]
fn a_a_name_split_over_lines_or_written_as_a_pattern() {
    for line in [
        "c\\\nlaude -p x --allowedTools Bash",
        "co\\\ndex exec x",
        "ls; c\\\nlaude -p x",
        "env c\\\nlaude -p x",
        "claude \\\n  -p x",
        "echo task | cl\\\naude -p",
        "\"c\\\nlaude\" -p x",
        "/usr/local/bin/cl?ude -p x",
        "~/.local/bin/clau* -p x",
        "cla[u]de -p x",
        "c[!x]aude -p x",
        "cod[a-z]x exec x",
        "{claude,} -p x",
        "{c,}laude -p x",
        "c{la,}ude -p x",
        "{c..c}laude -p x",
        "co{d..d}ex exec x",
        "/usr/local/bin/(x|claude) -p y",
        "~/.local/bin/(codex|nothing) exec x",
        "cl[[:alpha:]]ude -p x",
        "env cl?ude -p x",
        "timeout 9 ~/.local/bin/clau* -p x",
        "sudo /usr/local/bin/co?ex exec x",
        "x --dangerously-skip-\\\npermissions",
        "x --yo\\\nlo",
    ] {
        assert_asks(SH, line);
    }
    for line in [
        "cmd /c c^laude -p x",
        "cmd.exe /c c^laude -p x",
        "cmd /c \"c^laude -p x\"",
        "cmd /c co^dex exec x",
        "c`laude -p x",
    ] {
        assert_asks(PS, line);
    }
    for line in [
        "cargo test \\\n  --package foo",
        "echo cl?ude",
        "ls cla*",
        "ls *",
        "timeout 5 ls *",
        "nohup tar czf a.tgz *",
        "./*.sh",
        "cat {a,b}.txt",
        "echo {claude,codex}",
        "cp x.txt{,.bak}",
        // Braces expand to `echo claude -p x`: echo runs.
        "{echo,claude} -p x",
        // `coddx codex codfx exec x`: coddx runs.
        "cod{d..f}x exec x",
    ] {
        assert_not_seen(SH, line);
    }
    // A line continued on the next line is still a plain command that a grant can cover.
    assert!(matches!(
        parse(SH, "cargo test \\\n  --package foo"),
        Parsed::Plain(ref c) if c.len() == 1 && c[0].args == ["test", "--package", "foo"]
    ));
    assert!(
        classify(SH, "cargo test \\\n  --package foo")
            .always_ask
            .is_none()
    );
}

/// (a) Code handed to an interpreter (`python3 -c`, `node -e`, `perl -e`, …) that runs a
/// command line given as a string, and the program a locator finds used as a program
/// (`$(which claude)`, `& (Get-Command claude)`, .NET's `Process.Start`) (review of 0.0.6,
/// round 1).
#[test]
fn a_inline_code_and_located_programs() {
    for line in [
        "python3 -c \"import os; os.system('claude -p x')\"",
        "python -c 'import subprocess; subprocess.run([\"codex\", \"exec\", \"x\"])'",
        "python3.12 -c \"__import__('os').system('claude -p x')\"",
        "node -e \"require('child_process').execSync('codex exec x')\"",
        "node --eval \"require('child_process').spawnSync('claude', ['-p', 'x'])\"",
        "perl -e 'system(\"claude -p x\")'",
        "ruby -e 'system(\"codex\", \"exec\", \"x\")'",
        "osascript -e 'do shell script \"claude -p x\"'",
        "$(which claude) -p x",
        "`which codex` exec x",
        "$(command -v claude) -p x",
        "$(type -p codex) exec x",
        // Quoted, as shellcheck recommends (review of 0.0.6, round 2).
        "\"$(which claude)\" -p x",
        "\"`which codex`\" exec x",
        "\"$(command -v claude)\" -p x",
        "env \"$(which claude)\" -p x",
        "$(echo claude) -p x",
        "$(printf %s codex) exec x",
        "$(ls ~/.local/bin/cl*) -p x",
        "$(find / -name claude -type f) -p x",
        "\"$(realpath ~/.local/bin/claude)\" -p x",
    ] {
        assert_asks(SH, line);
    }
    for line in [
        "Start-Process -FilePath (Get-Command claude).Source -ArgumentList '-p x'",
        "Start-Process (gcm codex).Source",
        "saps -FilePath (Get-Command claude).Path",
        "& (Join-Path $env:APPDATA 'npm\\claude.cmd') -p x",
        "& (Resolve-Path ~\\.local\\bin\\cl*.exe) -p x",
        "& (Get-Command claude) -p x",
        "&(Get-Command claude) -p x",
        "& (gcm codex) exec x",
        "& (Get-Command claude).Source -p x",
        "[Diagnostics.Process]::Start('claude','-p x')",
        "[System.Diagnostics.Process]::Start(\"codex\", \"exec x\")",
        "python -c \"import os; os.system('claude -p x')\"",
    ] {
        assert_asks(PS, line);
    }
    for line in [
        "python3 -c 'print(1)'",
        "python3 -c \"print('hello world')\"",
        "node -e \"console.log('done')\"",
        "perl -e 'print \"ok\\n\"'",
        "echo $(which claude)",
        "ls -l $(which claude)",
        "if which claude > /dev/null; then echo found; fi",
        "$(which cargo) test",
        "cargo test 2>&1 | grep -E 'a,q'",
        "sed -n '1,q' notes.txt > out.txt",
    ] {
        assert_not_seen(SH, line);
    }
    for line in [
        "Get-Command claude",
        "(Get-Command claude).Source",
        "Write-Output (Get-Command claude).Source",
        "Test-Path (gcm claude).Path",
        "[Diagnostics.Process]::Start('notepad', 'claude.txt')",
    ] {
        assert_not_seen(PS, line);
    }
}

/// (a) A name kept in a variable or an alias on the same line and then run
/// (`c=claude; $c -p x`, `$c = 'claude'; & $c`, `Set-Alias c claude; c`) (review of 0.0.6,
/// round 2).
#[test]
fn a_a_name_kept_in_a_variable_or_alias() {
    for line in [
        "c=claude; $c -p x",
        "c=claude; ${c} -p x",
        "export C=/usr/local/bin/claude; \"$C\" -p x",
        "declare -x c=codex; $c exec x",
        "c=claude && env $c -p x",
        "alias c=claude; c -p x",
        "alias c='claude --model x'; c -p y",
        "alias c='claude --model x'\nc -p x",
        "p=~/.local/bin/clau*; $p -p x",
    ] {
        assert_asks(SH, line);
    }
    for line in [
        "$c='claude'; & $c -p x",
        "$c = 'codex'; & $c exec x",
        "$env:c = 'claude'; & $env:c -p x",
        "Set-Alias c claude; c -p x",
        "Set-Alias -Name c -Value 'C:\\Users\\me\\.local\\bin\\claude.exe'; c -p x",
        "sal -Name c -Value codex; c exec x",
        "New-Variable -Name c -Value claude; & $c -p x",
        "cmd /c \"set c=claude & %c% -p x\"",
    ] {
        assert_asks(PS, line);
    }
    for line in [
        "c=cargo; $c test",
        "x=claude; echo $x",
        "alias ll='ls -l'; ll",
        "c=claude; ls $c",
        "NAME=claude make build",
    ] {
        assert_not_seen(SH, line);
    }
    for line in [
        "$c = 'claude'",
        "Set-Alias c claude",
        "$c = 'claude'; Write-Output $c",
    ] {
        assert_not_seen(PS, line);
    }
}

/// (a) A command line fed to a shell on its standard input (`echo 'claude -p x' | sh`,
/// `… | at now`) (review of 0.0.6, round 2).
#[test]
fn a_a_command_line_fed_to_a_shell() {
    for line in [
        "echo 'claude -p x' | sh",
        "echo 'claude -p x' | bash",
        "echo claude -p x | sh",
        "printf '%s\\n' 'codex exec x' | bash -s",
        "echo 'claude -p x' | zsh -",
        "echo \"claude -p $X\" | sh",
        "cat notes.txt; echo 'claude -p x' | dash",
        "echo 'claude -p x' | at now",
        "at now <<< 'claude -p x'",
        "batch <<< 'codex exec x'",
        "sh <<< 'claude -p x'",
    ] {
        assert_asks(SH, line);
    }
    for line in [
        "'claude -p x' | cmd",
        "'codex exec x' | powershell -",
        "echo 'claude -p x' | pwsh -Command -",
    ] {
        assert_asks(PS, line);
    }
    for line in [
        "echo 'ls -la' | sh",
        "echo claude | grep c",
        "echo 'claude -p x' > notes.txt",
        "cat install.sh | sh",
        "echo 'claude -p x' | sh -c 'cat'",
        "at -f job.sh now",
    ] {
        assert_not_seen(SH, line);
    }
}

/// (a) Any program, not only the listed ones, that runs what follows a word such as
/// `run`, `exec` or `--` (`kubectl exec pod -- claude`, `docker run img claude`), with the
/// value of a long option left out (`--model claude`) (review of 0.0.6, round 2).
#[test]
fn a_after_a_run_word_of_any_program() {
    for line in [
        "kubectl exec pod -- claude -p x",
        "kubectl -n ns exec -it pod -- codex exec x",
        "docker run --rm -it img claude -p x",
        "docker exec -it box codex exec x",
        "podman run img claude -p x",
        "devcontainer exec --workspace-folder . claude -p x",
        "someenv run claude -p x",
        "sometool exec -- codex exec x",
        "flatpak run org.x.Y --command=sh -- claude",
        "pm2 start claude -- -p x",
        "tmux-like split-pane claude",
        "vagrant ssh -c 'claude -p x'",
        "anything --exec 'codex exec x'",
        "anything --run claude",
    ] {
        assert_asks(SH, line);
    }
    for line in [
        "docker run --rm img cargo test",
        "docker run -e MODEL=claude img",
        "npm run dev -- --provider claude",
        "poetry run python app.py --model claude",
        "cargo run -- --name codex",
        "git checkout -- claude",
        "git diff -- codex",
        "grep -c claude notes.txt",
        "grep -r exec claude",
        "echo run claude",
        "kubectl exec pod -- ls",
        "make run",
        "psql -c 'select 1'",
    ] {
        assert_not_seen(SH, line);
    }
}

/// (a) After a run word, an option of the program's own that takes no value (`--frozen`,
/// `--no-project`, `--verbose`) does not hide the program that follows it, nor does an
/// option with a value before the run word (`uv --directory . run claude`); only the value
/// of an option known to take a label (`--model claude`, `--name codex`) is left out, and
/// the value of any other option counts (`tb run --agent claude-code`) (review of 0.0.6,
/// round 3).
#[test]
fn a_after_a_run_word_past_options_of_its_own() {
    for line in [
        "uv run --no-project claude -p 'fix it'",
        "uv run --no-project claude -p '…'",
        "uv run --frozen claude",
        "conda run --no-capture-output claude",
        "bundle exec --keep-file-descriptors claude",
        "poetry run --verbose claude",
        "pixi run --frozen claude",
        "devbox run --pure claude",
        "mise exec --yes claude",
        "uv --directory . run claude",
        "uv --directory . --cache-dir /tmp/c --python 3.12 run claude -p x",
        "uv run --frozen --no-sync --isolated codex exec x",
        "docker run --rm --init img claude -p x",
        "pixi run --frozen --environment dev claude -p x",
        "poetry run --verbose python app.py; poetry run --quiet claude",
        "tb run --agent claude-code -t x",
        "sometool run --provider codex",
    ] {
        assert_asks(SH, line);
    }
    for line in [
        "poetry run --verbose python app.py --model claude",
        "uv run --frozen python app.py --model codex",
        "docker run --name claude img cargo test",
        "uv --directory . run pytest",
        "cargo run --release -- --name codex",
        "gdb --args ./app --model codex",
        "conda run --name claude python x.py",
    ] {
        assert_not_seen(SH, line);
    }
}

/// (a) A pattern in the place of the program that a wrapper or a run word runs, after the
/// wrapper's own values (`timeout 9`, `nice -n 5`, `sudo -u me`) or after the run word
/// (`uv run c*`), is read as any pattern the shell can expand to an assistant's name, as
/// in the program's own place: one fixed letter is enough (`~/.local/bin/c*`). Where the
/// word before may have been an option's value, at least one letter must be fixed, so
/// `tests/*` stays an argument (review of 0.0.6, round 3).
#[test]
fn a_a_pattern_in_the_program_place_after_a_wrapper_or_run_word() {
    for line in [
        "timeout 9 ~/.local/bin/c* -p x",
        "nice -n 5 ~/.local/bin/c* -p x",
        "uv run ~/.local/bin/c* -p x",
        "uv run c* -p x",
        "timeout 9s c* -p x",
        "sudo -u me ~/.local/bin/c* -p x",
        "env -i PATH=/usr/bin c* -p x",
        "runuser -u me -- c* -p x",
        "timeout 9 pixi run c* -p x",
        "nice -n 5 uv run ~/.local/bin/c* -p x",
        "uv run --frozen c* -p x",
        "uv run --with rich ~/.local/bin/c* -p x",
        "conda run -n env c* -p x",
        "nvm exec 20 c* -p x",
        "kubectl exec pod -- c* -p x",
        "mise exec -- ~/.local/bin/c* -p x",
        "docker run --rm -it img c* -p x",
    ] {
        assert_asks(SH, line);
    }
    for line in [
        "timeout 5 ls *",
        "nice -n 5 ls *",
        "sudo -u me ls *",
        "nohup tar czf a.tgz *",
        "uv run pytest tests/*",
        "uv run --frozen pytest tests/*",
        "conda run -n env python *.py",
        "nvm exec 20 node *",
        "kubectl exec pod -- ls *",
        "timeout 9 bundle exec rake *",
    ] {
        assert_not_seen(SH, line);
    }
}

/// Reading a line many ways stays linear in parentheses and backticks: a line of 16 KB or
/// 64 KB full of `(` or `` ` `` that never close nearby, or of locators that never close,
/// is read in bounded time, and comes out ordinary or past the limit of work (review of
/// 0.0.6, round 3). What is timed is this rule ([`find`]) on what the classifier made of
/// the line. The limits are generous for slow runners; a debug build takes a small part of
/// them.
#[test]
fn parentheses_and_backticks_stay_linear() {
    let lines = |size: usize| {
        [
            format!("{})", "a(".repeat(size / 2)),
            format!("echo {}", "(a".repeat(size / 2)),
            format!("{}`", "a`b".repeat(size / 3)),
            format!("echo `{}", "a `x ".repeat(size / 5)),
            format!("{}claude)", "$(which ".repeat(size / 8)),
            "(which (".repeat(size / 8),
            "& (gcm ".repeat(size / 7),
            format!("{} -p x", "$(".repeat(size / 2)),
        ]
    };
    for (size, limit) in [(16 * 1024, 5), (64 * 1024, 15)] {
        for line in lines(size) {
            assert!(line.len() >= size - 8, "{}", line.len());
            for shell in [SH, PS] {
                let parsed = parse(shell, &line);
                let started = std::time::Instant::now();
                let found = find(&line, &parsed);
                let took = started.elapsed();
                assert!(
                    took < std::time::Duration::from_secs(limit),
                    "{shell:?}, {} bytes starting {:?}: {took:?}",
                    line.len(),
                    &line[..24]
                );
                if let Some(why) = found
                    && why != TOO_MUCH
                {
                    // Only a line that names an assistant may be seen for it.
                    assert!(
                        line.contains("claude"),
                        "{shell:?} {:?}: {why}",
                        &line[..24]
                    );
                }
            }
        }
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
        // An option with a value before the subcommand (review of 0.0.6, round 1).
        "npm --prefix /x exec -- claude -p x",
        "npm -w pkg exec codex",
        "npm --registry https://r exec claude",
        "pnpm --dir x dlx @openai/codex",
        "yarn --cwd x dlx @openai/codex",
        // A package manager handed the program of a package by name.
        "yarn claude",
        "pnpm claude -p x",
        "bun codex exec x",
        "yarn run claude",
        "bun run codex exec x",
        "yarn --cwd x claude -p x",
        "pnpm -C x codex exec x",
    ] {
        assert_asks(SH, line);
    }
    for line in [
        "yarn test",
        "yarn add claude-notes",
        "pnpm install",
        "pnpm --dir x build",
        "bun run build",
        "bun test",
        "bun claude.ts",
        "npm --prefix /x run test",
        "npm --prefix /x exec -- vitest",
    ] {
        assert_not_seen(SH, line);
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
    // Nested shells, escapes, inline code and substitutions, each read several ways.
    let piece = "sh -c \"python3 -c 'x(\\\"a,b\\\")' $(which x) `y` c\\d; ";
    let line = piece.repeat(2000);
    let _ = command_facts(SH, &line, &parse(SH, &line));
    let _ = command_facts(PS, &line, &parse(PS, &line));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

/// A long ordinary line, such as a file written with a here-document, is read to the end
/// in every reading (braces, quotes, joined lines) within the limit of work, and stays
/// ordinary; a line nested past the limit counts, since it was not read to the end.
#[test]
fn long_ordinary_lines_are_read_to_the_end() {
    let code = "fn f(a: u8, b: u8) -> u8 { let x = [a, b]; g(\"{a,b}\", 'c', x) } \\\n".repeat(200);
    let line = format!("cat > src/lib.rs <<'EOF'\n{code}EOF\ncargo test");
    assert!(line.len() < 16 * 1024 && line.len() > 10_000);
    let started = std::time::Instant::now();
    assert_not_seen(SH, &line);
    assert_not_seen(PS, &line);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    // Shells inside shells, each with quotes, escapes, braces and joined lines, many times
    // over: past the limit, the line counts.
    let mut nested = "echo {a,b} c\\\nd 'x' \"y\" \\z; ".repeat(40);
    for _ in 0..7 {
        nested = format!(
            "sh -c \"{} ; {}\"",
            nested.replace('"', "\\\""),
            nested.replace('"', "\\\"")
        );
        if nested.len() > 15 * 1024 {
            break;
        }
    }
    let started = std::time::Instant::now();
    let facts = command_facts(SH, &nested, &parse(SH, &nested));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    if let Some(why) = facts.coding_app {
        assert_eq!(why, TOO_MUCH);
    }
}

/// The why names what was found, for the card.
#[test]
fn the_reason_names_what_was_found() {
    let why = |line: &str| find(line, &parse(SH, line)).unwrap_or_default();
    assert!(
        why("claude -p x").contains("claude is a coding assistant"),
        "{}",
        why("claude -p x")
    );
    assert!(why("npx @openai/codex exec").contains("@openai/codex"));
    assert!(why("x --yolo").contains("--yolo"));
    assert!(why("timeout 9 codex exec").contains("codex"));
}

/// Sensitive places (0.0.6): a command that writes the settings of Claude Code or Codex (a
/// project's `.claude/` and `.mcp.json`, the user's `~/.claude/`, `~/.claude.json` and
/// `~/.codex/`) keeps asking in Allow all, the same rule as `.git`, is decided in Ask first
/// exactly as the same line for `.git` is, and runs in Full access; names that only look
/// alike, and plain reads, do not keep asking.
#[test]
fn writing_their_settings_keeps_asking() {
    use crate::engine::KeepAsking;
    let kind =
        |shell: Shell, line: &str| command_facts(shell, line, &parse(shell, line)).keep_asking;
    for (shell, line) in [
        (SH, "cp hooks.json .claude/settings.json"),
        (SH, "echo '{}' > .claude/settings.local.json"),
        (SH, "tee .mcp.json"),
        (SH, "mv servers.json sub/.mcp.json"),
        (SH, "cp x ~/.claude.json"),
        (SH, "cp x /home/u/.claude/settings.json"),
        (SH, "echo 'model = \"x\"' >> ~/.codex/config.toml"),
        (SH, "mkdir -p .codex"),
        (PS, "Set-Content -Path .claude\\settings.json -Value x"),
        (PS, "Copy-Item x C:\\Users\\me\\.codex\\config.toml"),
        (PS, "Copy-Item x C:\\Users\\me\\.claude.json"),
    ] {
        let found = kind(shell, line);
        assert!(
            matches!(found, Some((KeepAsking::Sensitive, _))),
            "{shell:?} {line:?}: {found:?}"
        );
        let action = command_action(shell, line);
        assert!(
            matches!(
                evaluate(&action, &turn(true, false), &Everything),
                Decision::Ask {
                    reason: AskReason::StillAsksInAllowAll,
                    ..
                }
            ),
            "{line:?} did not ask in Allow all"
        );
        // Ask first: the same decision as the same line writing into `.git`, whatever was
        // granted (a grant for the command's own scope included).
        let as_git = line
            .replace(".claude.json", ".git/config")
            .replace(".claude", ".git")
            .replace(".codex", ".git")
            .replace(".mcp.json", ".git/config");
        let git = command_action(shell, &as_git);
        assert!(
            matches!(
                command_facts(shell, &as_git, &parse(shell, &as_git)).keep_asking,
                Some((KeepAsking::Sensitive, _))
            ),
            "{as_git:?}"
        );
        let granted: std::collections::BTreeSet<Scope> = [action.scope.clone()].into();
        assert_eq!(
            evaluate(&action, &turn(false, false), &granted),
            evaluate(
                &git,
                &turn(false, false),
                &[git.scope.clone()]
                    .into_iter()
                    .collect::<std::collections::BTreeSet<_>>()
            ),
            "{line:?} vs {as_git:?} in Ask first"
        );
        assert_eq!(
            evaluate(&action, &full_access(false, false), &Everything),
            Decision::AllowedByFullAccess,
            "{line:?} in Full access"
        );
    }
    for (shell, line) in [
        (SH, "cat .claude/settings.json"),
        (SH, "ls .codex"),
        (SH, "cp notes.md claude/notes.md"),
        (SH, "cp a .claudeignore"),
        (SH, "cp a docs/claude.json"),
        (SH, "cp a mcp.json"),
        (SH, "cp a .mcp.json.example"),
        (SH, "cp a CLAUDE.md"),
        (PS, "Get-Content .claude\\settings.json"),
    ] {
        assert_eq!(kind(shell, line), None, "{shell:?} {line:?}");
    }
}

/// Sensitive places for the file tools (0.0.6), as the files describer builds the action
/// (`files::always_ask_reason` makes it always-ask, `files::keep_asking` keeps it asking in
/// Allow all): writing the settings of Claude Code or Codex asks with Allow once / Deny in
/// Ask first whatever was granted, still asks in Allow all, and runs in Full access, exactly
/// like a write into `.git`; a look-alike name stays grantable.
#[test]
fn the_file_tools_keep_asking_for_their_settings_in_every_mode() {
    use crate::files::{WriteKind, always_ask_reason, keep_asking};
    use crate::path::Flavor;
    let write = |path: &str| {
        let always = always_ask_reason(path, WriteKind::Edit, Flavor::Posix);
        Action {
            server: "files".into(),
            tool: "write_file".into(),
            summary: path.into(),
            preview: path.into(),
            scope: Scope::FileWrite {
                folder: "/home/u/proj".into(),
            },
            risk: if always.is_some() {
                Risk::AlwaysAsk
            } else {
                Risk::Normal
            },
            read_only: false,
            outbound: false,
            keep_asking: keep_asking(path, WriteKind::Edit, Flavor::Posix),
            network: false,
            hard_limit: None,
            payload: serde_json::Value::Null,
        }
    };
    let git = write("/home/u/proj/.git/config");
    for path in [
        "/home/u/proj/.claude/settings.json",
        "/home/u/proj/.mcp.json",
        "/home/u/proj/.codex/config.toml",
        "/home/u/.claude/settings.json",
        "/home/u/.claude.json",
        "/home/u/.codex/config.toml",
    ] {
        let action = write(path);
        assert_eq!(action.risk, Risk::AlwaysAsk, "{path}");
        for untrusted in [false, true] {
            assert_eq!(
                evaluate(&action, &turn(false, untrusted), &Everything),
                Decision::Ask {
                    card: CardKind::OnceOnly,
                    reason: AskReason::AlwaysAsk
                },
                "Ask first, {path}"
            );
            assert!(
                matches!(
                    evaluate(&action, &turn(true, untrusted), &Everything),
                    Decision::Ask {
                        reason: AskReason::StillAsksInAllowAll,
                        ..
                    }
                ),
                "Allow all, {path}"
            );
            for allow_all in [false, true] {
                assert_eq!(
                    evaluate(&action, &full_access(allow_all, untrusted), &Everything),
                    Decision::AllowedByFullAccess,
                    "Full access, {path}"
                );
            }
            for mode in [
                turn(false, untrusted),
                turn(true, untrusted),
                full_access(false, untrusted),
            ] {
                assert_eq!(
                    evaluate(&action, &mode, &Everything),
                    evaluate(&git, &mode, &Everything),
                    "{path} is not decided like .git in {mode:?}"
                );
            }
        }
    }
    let plain = write("/home/u/proj/claude/notes.md");
    assert_eq!(plain.risk, Risk::Normal);
    assert_eq!(
        evaluate(&plain, &turn(false, false), &Everything),
        Decision::AllowedByGrant
    );
}
