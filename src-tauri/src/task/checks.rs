//! Which commands are checks (a test run, a build, a lint, a type check), and how one ended.
//!
//! A command counts only when its result says how the check went:
//!
//! - Only the last statement of a command line is judged (`a; b` reports how `b` ended).
//! - A check whose result is hidden is not one: `npm test || true`, a check piped into
//!   something else (`cargo test | tail`: the result is `tail`'s), a check sent to the
//!   background.
//! - A check is recognised by the program that runs, never by words somewhere in the line,
//!   so `echo "npm test"` and `git commit -m "fix tests"` are not checks.
//!
//! How it ended is read from two things the terminal tool always gives: whether the result
//! is an error, and the first line `Exit code N (…)`. Nothing else in the result's wording
//! is relied on.

use mcp_host::ToolOutput;

/// Characters of a check's one-line summary kept for the Task tab.
const SUMMARY_CHARS: usize = 160;

/// How deep a command inside a command (`bash -c "…"`) is followed.
const MAX_NESTING: usize = 3;

// ---- Reading a command line ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    /// `&&`
    And,
    /// `||`
    Or,
    /// `|`
    Pipe,
    /// `;` or a line break
    End,
    /// A lone `&` after a command.
    Background,
}

/// Splits a command line into words and operators. Quotes group a word; a backslash is an
/// ordinary character outside quotes (it separates folders on Windows).
fn tokens(command: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;
    // Whether the statement in progress has a word yet (a leading `&` is PowerShell's call
    // operator, not "run in the background").
    let mut statement_has_word = false;
    macro_rules! flush {
        () => {
            if in_word {
                out.push(Token::Word(std::mem::take(&mut word)));
                in_word = false;
                statement_has_word = true;
            }
        };
    }
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '\'' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    word.push(chars[i]);
                    i += 1;
                }
            }
            '"' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && chars.get(i + 1) == Some(&'"') {
                        i += 1;
                    }
                    word.push(chars[i]);
                    i += 1;
                }
            }
            '\n' | ';' => {
                flush!();
                out.push(Token::End);
                statement_has_word = false;
            }
            '&' if next == Some('&') => {
                flush!();
                out.push(Token::And);
                i += 1;
            }
            '|' if next == Some('|') => {
                flush!();
                out.push(Token::Or);
                i += 1;
            }
            '|' => {
                flush!();
                out.push(Token::Pipe);
                // `|&` pipes both streams.
                if next == Some('&') {
                    i += 1;
                }
            }
            // `2>&1` and `&>file` are redirections.
            '&' if in_word && word.ends_with('>') => word.push(c),
            '&' if next == Some('>') => {
                in_word = true;
                word.push(c);
            }
            '&' => {
                flush!();
                if statement_has_word {
                    out.push(Token::Background);
                }
            }
            // Grouping: `(cd app && npm test)`.
            '(' | ')' if !in_word => {}
            c if c.is_whitespace() => flush!(),
            c => {
                in_word = true;
                word.push(c);
            }
        }
        i += 1;
    }
    flush!();
    out
}

/// The words of a name, lower case: split at anything that is not a letter or a digit, and
/// where a small letter is followed by a capital (`assembleDebug`).
fn name_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut last_lower = false;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            last_lower = false;
            continue;
        }
        if c.is_uppercase() && last_lower && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        last_lower = c.is_lowercase() || c.is_ascii_digit();
        word.extend(c.to_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Words in the name of a script, a make target or a build task that say it is a check.
const CHECK_WORDS: &[&str] = &[
    "test",
    "tests",
    "build",
    "lint",
    "check",
    "typecheck",
    "tsc",
    "compile",
    "verify",
    "spec",
    "specs",
    "e2e",
    "ci",
    "clippy",
    "assemble",
    "validate",
    "coverage",
];

/// Whether the name of a script, target or task says it is a check (`test:unit`,
/// `build.sh`, `assembleDebug`), and not one that keeps running (`test:watch`).
fn named_like_a_check(name: &str) -> bool {
    let words = name_words(name);
    words.iter().any(|w| CHECK_WORDS.contains(&w.as_str())) && !words.iter().any(|w| w == "watch")
}

/// The program a word names: the file name without its folder, lower case, without the
/// endings Windows adds to programs.
fn program_of(word: &str) -> String {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let mut name = base.to_lowercase();
    for ending in [".exe", ".cmd", ".bat"] {
        if let Some(stripped) = name.strip_suffix(ending) {
            name = stripped.to_owned();
            break;
        }
    }
    name
}

/// Endings of script files; a script is judged by its name.
const SCRIPT_ENDINGS: &[&str] = &[
    ".sh", ".bash", ".zsh", ".ps1", ".py", ".js", ".mjs", ".cjs", ".ts", ".rb", ".bat", ".cmd",
];

fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
        }
        None => false,
    }
}

fn is_option(word: &str) -> bool {
    word.starts_with('-')
}

/// Programs that only start another program: what follows them is judged.
const WRAPPERS: &[&str] = &[
    "time",
    "nice",
    "nohup",
    "sudo",
    "env",
    "command",
    "exec",
    "winpty",
    "npx",
    "bunx",
    "pnpx",
    "uvx",
    "xvfb-run",
    "dotenv",
    "cross-env",
    "timeout",
];

/// Program and first argument that together only start another program.
const WRAPPER_PAIRS: &[(&str, &str)] = &[
    ("pnpm", "exec"),
    ("pnpm", "dlx"),
    ("yarn", "exec"),
    ("yarn", "dlx"),
    ("npm", "exec"),
    ("npm", "x"),
    ("bun", "x"),
    ("uv", "run"),
    ("poetry", "run"),
    ("pipenv", "run"),
    ("pdm", "run"),
    ("hatch", "run"),
    ("rye", "run"),
    ("pipx", "run"),
    ("bundle", "exec"),
];

/// Programs whose every run is a check.
const ALWAYS: &[&str] = &[
    // Tests.
    "pytest",
    "py.test",
    "tox",
    "nox",
    "unittest",
    "jest",
    "vitest",
    "mocha",
    "ava",
    "jasmine",
    "karma",
    "phpunit",
    "pest",
    "rspec",
    "ctest",
    // Lints and type checks.
    "tsc",
    "vue-tsc",
    "svelte-check",
    "eslint",
    "oxlint",
    "stylelint",
    "mypy",
    "pyright",
    "flake8",
    "pylint",
    "pyflakes",
    "rubocop",
    "golangci-lint",
    "staticcheck",
    "shellcheck",
    "hadolint",
    "yamllint",
    "markdownlint",
    "ktlint",
    "cppcheck",
    "clang-tidy",
    // Builds.
    "ninja",
    "msbuild",
    "xcodebuild",
    "gcc",
    "g++",
    "cc",
    "c++",
    "clang",
    "clang++",
    "rustc",
    "javac",
    "kotlinc",
    "swiftc",
    "compileall",
    "py_compile",
];

/// Programs that are a check with one of these as their first plain argument.
const WITH_SUBCOMMAND: &[(&str, &[&str])] = &[
    (
        "cargo",
        &[
            "test", "t", "check", "c", "clippy", "build", "b", "bench", "nextest",
        ],
    ),
    ("go", &["test", "vet", "build"]),
    ("deno", &["test", "check", "lint"]),
    ("dotnet", &["test", "build"]),
    ("swift", &["test", "build"]),
    ("zig", &["test", "build"]),
    ("mix", &["test", "compile"]),
    ("biome", &["check", "lint", "ci"]),
    ("ruff", &["check"]),
    ("playwright", &["test"]),
    ("cypress", &["run"]),
    ("terraform", &["validate"]),
    ("composer", &["test"]),
];

/// Formatters: a check only when asked to report, not to rewrite.
const FORMATTERS: &[&str] = &["prettier", "black", "isort", "rustfmt", "ruff", "cargo"];

/// Package script runners: `npm test`, `npm run <script>`, and for the others also
/// `<runner> <script>`.
const SCRIPT_RUNNERS: &[&str] = &["npm", "pnpm", "yarn", "bun"];

/// Build tools whose targets or tasks are judged by their names. The flag says whether a
/// run without a target is a build.
const TARGET_RUNNERS: &[(&str, bool)] = &[
    ("make", true),
    ("gmake", true),
    ("nmake", true),
    ("mingw32-make", true),
    ("just", false),
    ("rake", false),
    ("gradle", false),
    ("gradlew", false),
    ("mvn", false),
    ("mvnw", false),
    ("ant", false),
];

/// Words that are a check as a Maven phase but would be too wide as a general word.
const MAVEN_PHASES: &[&str] = &["package", "install"];

const INTERPRETERS: &[&str] = &["python", "python3", "py", "node", "ruby", "php", "deno", "bun"];

const SHELLS: &[&str] = &["bash", "sh", "zsh", "dash", "pwsh", "powershell", "cmd"];

/// Whether one command (words between `&&`) is a check.
fn words_are_a_check(words: &[String], depth: usize) -> bool {
    let _ = (words, depth);
    false
}

/// Whether the result of running `command` says how a test run, a build, a lint or a type
/// check went.
pub fn is_check(command: &str) -> bool {
    is_check_at(command, 0)
}

fn is_check_at(command: &str, depth: usize) -> bool {
    let _ = (command, depth);
    false
}

// ---- Reading a result ---------------------------------------------------------------------

/// How a command ended, as far as its result says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The exit code, when the result's first line gives one.
    pub exit_code: Option<i32>,
    /// The command ended with success.
    pub passed: bool,
    /// The last line of the output, for the Task tab. It is text from the command: never
    /// sent to the model outside the tool result it came in.
    pub summary: String,
}

/// The exit code in a result's first line, `Exit code N (…)`.
fn exit_code_in(text: &str) -> Option<i32> {
    let _ = text;
    None
}

/// Reads how a `run_command` call ended from its result.
pub fn outcome(out: &ToolOutput) -> Outcome {
    let _ = (out, SUMMARY_CHARS, exit_code_in(""));
    Outcome {
        exit_code: None,
        passed: false,
        summary: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Commands a model writes in bash and zsh.
    #[test]
    fn recognises_checks_in_posix_shells() {
        let table: &[(&str, bool)] = &[
            // The spec's own list.
            ("pytest", true),
            ("pytest -q tests/test_dates.py", true),
            ("cargo test", true),
            ("cargo check", true),
            ("cargo clippy --all-targets -- -D warnings", true),
            ("cargo build --release", true),
            ("cargo +1.99 test -p store", true),
            ("npm test", true),
            ("npm run build", true),
            ("npm run lint", true),
            ("tsc", true),
            ("tsc --noEmit -p tsconfig.json", true),
            ("go test ./...", true),
            ("go vet ./...", true),
            ("go build ./cmd/app", true),
            ("make", true),
            ("make test", true),
            // Scripts, targets and tasks are judged by their names.
            ("npm run test:unit", true),
            ("npm run typecheck", true),
            ("npm t", true),
            ("yarn lint", true),
            ("pnpm build", true),
            ("pnpm run check", true),
            ("bun test", true),
            ("make lint", true),
            ("make -j8 build", true),
            ("./build.sh", true),
            ("bash scripts/run-tests.sh", true),
            ("./gradlew assembleDebug", true),
            ("./gradlew connectedAndroidTest", true),
            ("mvn -q package", true),
            ("python manage.py test", true),
            ("python scripts/build.py", true),
            ("node scripts/lint.mjs", true),
            ("node --test", true),
            // Other common ones.
            ("ruff check .", true),
            ("ruff format --check .", true),
            ("black --check src", true),
            ("prettier --check .", true),
            ("cargo fmt --all --check", true),
            ("cargo fmt --all -- --check", true),
            ("eslint src --max-warnings 0", true),
            ("mypy src", true),
            ("vitest run", true),
            ("jest --ci", true),
            ("cmake --build build", true),
            ("dotnet test", true),
            ("gcc -Wall -o app main.c", true),
            ("playwright test", true),
            // Started through something else: the program that ends up running counts.
            ("npx tsc --noEmit", true),
            ("npx --yes vitest run", true),
            ("pnpm exec eslint .", true),
            ("uv run pytest", true),
            ("poetry run mypy .", true),
            ("python -m pytest -x", true),
            ("python3 -m unittest discover", true),
            ("CI=1 npm test", true),
            ("env CI=1 NODE_ENV=test npm test", true),
            ("time cargo test", true),
            ("timeout 300 cargo test", true),
            ("/usr/local/bin/pytest", true),
            ("bash -c \"cd app && npm test\"", true),
            ("bash -lc 'cargo test'", true),
            ("bundle exec rspec", true),
            // In a chain every part must succeed, so the result is the check's too.
            ("cd app && npm test", true),
            ("npm run build && npm test", true),
            ("(cd app && cargo test)", true),
            ("npm test 2>&1", true),
            ("npm test > out.txt 2>&1", true),
            ("cd app; npm test", true),
            ("export CI=1\nnpm test", true),
            // Not checks at all.
            ("ls -la", false),
            ("cat package.json", false),
            ("npm install", false),
            ("npm ci", false),
            ("npm run dev", false),
            ("npm start", false),
            ("npm run format", false),
            ("yarn add left-pad", false),
            ("pnpm install", false),
            ("cargo run", false),
            ("cargo fmt --all", false),
            ("cargo add serde", false),
            ("go run .", false),
            ("go mod tidy", false),
            ("make clean", false),
            ("make install", false),
            ("git status", false),
            ("python main.py", false),
            ("python -m http.server", false),
            ("node server.js", false),
            ("prettier --write .", false),
            ("black src", false),
            ("ruff format .", false),
            ("cmake -S . -B build", false),
            ("pip install -r requirements.txt", false),
            ("", false),
            ("   ", false),
            // The word is there, but another program runs.
            ("echo \"npm test\"", false),
            ("echo run cargo test next", false),
            ("git commit -m \"fix tests\"", false),
            ("grep -r test src", false),
            ("cat test.log", false),
            ("rm -rf build", false),
            ("mkdir build", false),
            ("cd build", false),
            ("test -f package.json", false),
            ("which pytest", false),
            ("latest --build", false),
            // The result would not be the check's.
            ("npm test || true", false),
            ("cargo test || echo failed", false),
            ("cargo test 2>&1 | tail -20", false),
            ("npm test | tee out.txt", false),
            ("pytest |& tee log.txt", false),
            ("npm test &", false),
            ("npm test; echo done", false),
            ("cargo build; ls target", false),
            // Not a run that ends with a verdict.
            ("npm test -- --watch", false),
            ("npm run test:watch", false),
            ("tsc --watch", false),
            ("vitest --watch", false),
            ("cargo watch -x test", false),
            ("pytest --help", false),
            ("tsc --version", false),
            ("cargo test --help", false),
        ];
        for (command, expected) in table {
            assert_eq!(is_check(command), *expected, "{command:?}");
        }
    }

    /// Commands a model writes in PowerShell (and a few for cmd).
    #[test]
    fn recognises_checks_in_powershell() {
        let table: &[(&str, bool)] = &[
            ("npm test", true),
            ("npm.cmd run build", true),
            ("cargo.exe test", true),
            ("cd C:\\work\\app; npm test", true),
            ("Set-Location C:\\work\\app; cargo build", true),
            ("$env:CI = \"1\"; npm test", true),
            ("$env:CI=\"1\"; npx vitest run", true),
            (".\\gradlew.bat build", true),
            ("C:\\work\\app\\gradlew.bat assembleDebug", true),
            ("& \"C:\\Program Files\\nodejs\\npm.cmd\" test", true),
            ("& .\\build.ps1", true),
            (".\\scripts\\Run-Tests.ps1", true),
            ("python -m pytest", true),
            ("py -m pytest", true),
            ("dotnet build MyApp.sln", true),
            ("msbuild MyApp.sln /p:Configuration=Release", true),
            ("powershell -Command \"npm test\"", true),
            ("pwsh -NoProfile -Command \"cargo check\"", true),
            ("cmd /c \"npm run lint\"", true),
            ("cd app && npm test", true),
            ("npm test 2>&1", true),
            // Not checks.
            ("Get-ChildItem", false),
            ("dir", false),
            ("npm install", false),
            ("Get-Content test.log", false),
            ("Write-Output \"npm test\"", false),
            ("Remove-Item -Recurse build", false),
            ("npm run dev", false),
            ("Test-Path package.json", false),
            ("Select-String -Path src -Pattern test", false),
            // The result would not be the check's.
            ("npm test | Out-String", false),
            ("npm test 2>&1 | Select-Object -Last 20", false),
            ("npm test; Write-Output done", false),
            ("Start-Process npm -ArgumentList test", false),
        ];
        for (command, expected) in table {
            assert_eq!(is_check(command), *expected, "{command:?}");
        }
    }

    #[test]
    fn names_are_split_into_words() {
        assert_eq!(name_words("test:unit"), ["test", "unit"]);
        assert_eq!(name_words("assembleDebug"), ["assemble", "debug"]);
        assert_eq!(name_words("run-tests.sh"), ["run", "tests", "sh"]);
        assert_eq!(name_words("Run-Tests"), ["run", "tests"]);
        assert_eq!(name_words("build_all2"), ["build", "all2"]);
        // A word inside another word is not that word.
        assert!(!named_like_a_check("latest"));
        assert!(!named_like_a_check("contest-entry"));
        assert!(!named_like_a_check("rebuild")); // one word, and not in the list
        assert!(named_like_a_check("typecheck"));
        assert!(named_like_a_check("type-check"));
        assert!(!named_like_a_check("test:watch"));
    }

    #[test]
    fn a_command_line_is_split_at_operators_outside_quotes() {
        use Token::{And, Background, End, Or, Pipe, Word};
        let w = |s: &str| Word(s.to_owned());
        assert_eq!(
            tokens("cd app && npm test"),
            [w("cd"), w("app"), And, w("npm"), w("test")]
        );
        assert_eq!(
            tokens("a || b | c; d &"),
            [w("a"), Or, w("b"), Pipe, w("c"), End, w("d"), Background]
        );
        // Quoted operators are text.
        assert_eq!(
            tokens("echo \"a && b | c; d\" 'e || f'"),
            [w("echo"), w("a && b | c; d"), w("e || f")]
        );
        // Redirections are not "background".
        assert_eq!(
            tokens("npm test 2>&1 &>log"),
            [w("npm"), w("test"), w("2>&1"), w("&>log")]
        );
        // PowerShell's call operator.
        assert_eq!(
            tokens("& \"C:\\Program Files\\x.cmd\" test"),
            [w("C:\\Program Files\\x.cmd"), w("test")]
        );
        assert_eq!(tokens("(cd a)"), [w("cd"), w("a")]);
    }

    fn result(text: &str, is_error: bool) -> ToolOutput {
        ToolOutput {
            content: vec![mcp_host::Content::Text { text: text.into() }],
            is_error,
            full_text: None,
            ui: None,
        }
    }

    #[test]
    fn a_result_says_how_the_command_ended() {
        // Success: exit code 0 and not an error.
        let ok = outcome(&result(
            "Exit code 0 (2.3 s)\nrunning 12 tests\n\ntest result: ok. 12 passed; 0 failed\n",
            false,
        ));
        assert_eq!(ok.exit_code, Some(0));
        assert!(ok.passed);
        assert_eq!(ok.summary, "test result: ok. 12 passed; 0 failed");

        // A failed run: the exit code is kept.
        let failed = outcome(&result(
            "Exit code 101 (0.4 s)\nerror: test failed, to rerun pass `--lib`",
            true,
        ));
        assert_eq!(failed.exit_code, Some(101));
        assert!(!failed.passed);
        assert_eq!(failed.summary, "error: test failed, to rerun pass `--lib`");

        // An error without an exit code: it did not run to its end, whatever the wording.
        // Its first line is then the tool's own word on what happened.
        for (text, summary) in [
            (
                "Timed out after 120 s; the command was ended\npartial output",
                "Timed out after 120 s; the command was ended",
            ),
            (
                "Some other wording for a command that was stopped",
                "Some other wording for a command that was stopped",
            ),
            ("", ""),
        ] {
            let cut = outcome(&result(text, true));
            assert_eq!(cut.exit_code, None, "{text:?}");
            assert!(!cut.passed, "{text:?}");
            assert_eq!(cut.summary, summary, "{text:?}");
        }

        // The error flag decides, also against a first line that says 0.
        let odd = outcome(&result("Exit code 0 (1.0 s)\nbut it failed", true));
        assert!(!odd.passed);
        // And an exit code that is not 0 is never a pass.
        let odd = outcome(&result("Exit code 2 (1.0 s)", false));
        assert!(!odd.passed);
        assert_eq!(odd.summary, "");
        // A negative code (Windows) is read.
        assert_eq!(
            outcome(&result("Exit code -1073741510 (3.0 s)", true)).exit_code,
            Some(-1_073_741_510)
        );
        // Only the first line is looked at for the code.
        assert_eq!(
            outcome(&result("something else\nExit code 0 (1 s)", false)).exit_code,
            None
        );
    }

    #[test]
    fn the_summary_is_one_short_line_of_the_output() {
        // Marker lines the app adds in brackets are skipped; the full text is preferred.
        let mut out = result(
            "Exit code 1 (9.0 s)\nfirst lines\n[Shortened: 400 more lines. Read them with read_output.]",
            true,
        );
        out.full_text = Some(
            "Exit code 1 (9.0 s)\nfirst lines\nmany more\n2 failed, 10 passed\n\n[output beyond 1 MiB was not kept]\n"
                .into(),
        );
        assert_eq!(outcome(&out).summary, "2 failed, 10 passed");
        out.full_text = None;
        assert_eq!(outcome(&out).summary, "first lines");

        // One line, nothing invisible, capped.
        let long = format!("Exit code 0 (1 s)\n{}\u{202E}\u{7}", "x".repeat(500));
        let summary = outcome(&result(&long, false)).summary;
        assert_eq!(summary.chars().count(), SUMMARY_CHARS);
        assert!(summary.ends_with('…'));
        let odd = outcome(&result("Exit code 0 (1 s)\nok\u{202E}done\u{7}", false)).summary;
        assert_eq!(odd, "ok\\u{202E}done\\u{0007}");
    }
}
