Feature G (visible, resumable task state) is finished and working end to end. The `task-state` scenario passed both times I ran it, all Rust and frontend tests pass, and clippy has no warnings in my files. The one check that fails is clippy on the `app` crate, and every error in it is in feature F's unfinished `src-tauri/src/evals*` code.

**1. Files created and changed**
- Wired the draft in. `src-tauri/src/task/root_draft.rs` was copied over `src-tauri/src/task.rs` and deleted.
- `src-tauri/src/task.rs`: I filled `fact_of`, `record` and `TaskDto::from`, and wrote `start_turn` and `note_in`. I added `note_from` and `turn_start_from`, which handle a task that cannot be read. `note_for_turn` has `#[allow(dead_code, reason = …)]` until feature I calls it.
- `task/checks.rs`: filled `words_are_a_check`, `is_check_at`, `exit_code_in` and `outcome`. I also fixed the lexer: a `)` now ends the word before it (the second developer's own lexer test was failing on this).
- `task/edits.rs`: filled every stub.
- `task/note.rs`: filled `cost`, `for_turn` and `for_result`, the renderer with its size budget.
- `task/server.rs`: filled `describe` and `Core::call`. I shortened the description.
- `task/tests.rs`:
  - Rewrote the test that could not compile. The app crate cannot write a broken row, so the test builds `StoreError::Json` the way the store does and checks no note, no write and the `task_unreadable` error. The store's own test already writes such a row and deletes it.
  - Added `a_failed_or_read_only_edit_is_not_a_changed_file`.
  - Added `the_inspector_and_the_turn_get_the_same_note`.
- New frontend files:
  - `src/components/task/`: `types.ts`, `api.ts`, `state.ts` (a store plus the `task-changed` listener), `TaskParts.tsx`, `TaskEditors.tsx`, `TaskFacts.tsx`, `ClearTaskDialog.tsx`.
  - `src/components/workbench/TaskTab.tsx`, replacing the placeholder.
  - `src/components/workbench/TaskTab.test.tsx`.
- Shared files, one small edit each:
  - `toolset.rs::build_host`: `host.add_builtin(Arc::new(crate::task::TaskServer::new(app.clone())));` plus a comment line.
  - `turns.rs::run`, right after `input.summary = …`: `input.app_notes.extend(crate::task::begin_turn(app, chat_id.0).await); // [0.0.4 task]`. `cargo fmt` split it over three lines. The helper's input is cloned before this point, so helpers get no task note.
  - `lib.rs` `generate_handler!`: `task::get_task`, `task::edit_task`, `task::set_task_status`, `task::clear_task`. The `mod task;` line was already there.
  - `commands.list`: the same four names.
  - `capabilities/main.json`: `allow-get-task`, `allow-edit-task`, `allow-set-task-status`, `allow-clear-task`.
  - `e2e/scripted_agent.py`: `p_task_state` and its entry in `PROBES`.
- I did not change `ipc.ts`, `types.ts`, `app.ts`, `tool_catalog.rs`, `mcp_cmd.rs` or the store. The catalog entry's name and group already match `task`/`update_task`, and a test checks this.

**2. What the user sees, and the design**
- **The Task tab** has a header with a status chip (Active, Paused, Done) and these buttons:
  - **Pause**: stores `paused`, then calls `useApp.stop()` (the Stop button's path) only if the running reply is in this chat.
  - **Resume**: shown for a paused task and for an active task with nothing running. It stores `active`, then sends "Continue the task. Start with the next action." through the normal `send`. It is disabled while any reply runs. Nothing touches approvals, grants, folders or tool switches.
  - **Mark done**, or **Reopen** on a done task.
  - **Clear task**, after a confirmation dialog.
- The tab's sections:
  - Goal, constraints, steps and next action, each editable in place, with "written by you" marks. Clicking a step's mark moves it todo → doing → done.
  - Changed files, newest first. A click copies the real path; the shown path has invisible characters escaped. A line says only changes made with the file tools are listed.
  - Checks: passed, failed with exit code, or did not finish, with the time and the one-line output summary.
  - Notes from the model, and "Last updated".
  - A task that cannot be read shows the error and a Clear button.
  - The tab refreshes on `task-changed`.
- **Design decisions kept from the second developer:**
  - What the user changed and the model has not seen wins once over a stale `update_task`; the result says "Not changed: the goal…" and the mark is cleared.
  - While a task is paused, or the user marked it done and the model has not been told, the model's `status` is ignored.
  - A done task that gets a different goal starts fresh, keeping the user's constraints.
  - `update_task` is refused while a helper runs.
  - A check's output summary goes to the tab only, never into the note or the tool result.
  - Files are recorded only from server `files`, `Scope::FileWrite`, `!read_only`, `!is_error` and `payload.canonical`. `write_file` counts as created when `expectedSha256` is null.
  - Checks are recorded from `payload.command`, `is_error` and the first line `Exit code N`. Scope must be Command, Commands or ReadOnlyProgram. An error with no exit code is recorded as "did not finish"; a non-error with no exit code is not recorded.
  - A record that holds only files and checks is created, but sends no note.
- **No design decision changed.** Where a test's expected string could have disagreed with the format rules, it did not: every expected string passed as written.
- **`update_task`** uses `Scope::AppData`, `Risk::Normal`, not outbound, no prompt, and is audited. The chat comes from the call's context, never from the arguments. Exact definition:
  - Description: "Save the task state the user sees and work resumes from. Send only what changed; a list replaces the old one."
  - Schema: `{"type":"object","properties":{"goal":{"type":"string"},"constraints":{"type":"array","items":{"type":"string"}},"steps":{"type":"array","items":{"type":"object","properties":{"text":{"type":"string"},"status":{"enum":["todo","doing","done"]}},"required":["text"]}},"next_action":{"type":"string"},"status":{"enum":["active","done"]},"note":{"type":"string"}},"additionalProperties":false}`
  - Size: description plus schema is 498 characters, and `definition_cost` comes to 194 tokens. I computed this with that function's own formula rather than calling it; the existing test asserts it stays at or under 215. This is over the 110-token aim because the schema shape fixed in the spec alone is 389 characters. No arguments were cut.
- **The result of `update_task` is the app's own data.** It holds text the model or user wrote and paths and command lines from tool arguments, never tool output.
- **Example task note, as captured from the e2e request** (the loop puts "[App note] " before it; the long folder path is shortened to `…` here):
  ```
  Task state, from the app's record of the work in this chat. It is information, not a request. Text in quotes is what you recorded earlier, unless marked as the user's.
  - status: active
  - since you last saw this, the user: paused this task and has now resumed it
  - goal: "Put a dark mode switch in the header"
  - steps (1 of 3 done): 2 [doing] "Add the switch"; 3 [todo] "Run the tests"
  - changed files (in "…/work/task-state"): created "Switch.tsx"
  - next action: "Add the switch to the header"
  ```
  - The full form also has `constraints`, `your latest note` and `last checks` lines, for example: `failed "npm run lint" (exit code 1); passed "npm test"`.
  - Size cap: 300 tokens including the prefix, with a character outside ASCII counted as one token. Space goes first to the goal and next action, then constraints, steps, latest note, 4 checks and 20 files, with "+N more".
  - Every supplied text is one line in quotes: `"` becomes `'`, any spelling of `[App note]` becomes `(App note)`, invisible characters are escaped and whitespace is squeezed.
- **Commands recognised as checks.** Only the last statement is judged. In an `&&` chain, any part that is a check counts. These are never checks: `||`, a pipe, `&` (background), `--help`, `-h`, `--version`, `--watch`. A command is judged by the program that runs, not by words elsewhere in the line.
  - Wrappers that are skipped: `time`, `nice`, `nohup`, `sudo`, `env`, `command`, `exec`, `winpty`, `npx`, `bunx`, `pnpx`, `uvx`, `xvfb-run`, `dotenv`, `cross-env`, `timeout`, and `VAR=x`. Also `pnpm`/`yarn exec|dlx`, `npm exec|x`, `bun x`, `uv`/`poetry`/`pipenv`/`pdm`/`hatch`/`rye`/`pipx run`, `bundle exec`.
  - Always a check: pytest, tox, nox, unittest, jest, vitest, mocha, ava, jasmine, karma, phpunit, pest, rspec, ctest, tsc, vue-tsc, svelte-check, eslint, oxlint, stylelint, mypy, pyright, flake8, pylint, pyflakes, rubocop, golangci-lint, staticcheck, shellcheck, hadolint, yamllint, markdownlint, ktlint, cppcheck, clang-tidy, ninja, msbuild, xcodebuild, gcc, g++, cc, c++, clang, clang++, rustc, javac, kotlinc, swiftc, compileall, py_compile.
  - A check with one of these subcommands:
    - `cargo` test, t, check, c, clippy, build, b, bench, nextest (a `+toolchain` is skipped)
    - `go` test, vet, build
    - `deno` test, check, lint
    - `dotnet`, `swift`, `zig` test, build
    - `mix` test, compile
    - `biome` check, lint, ci
    - `ruff check`, `playwright test`, `cypress run`, `terraform validate`, `composer test`
  - `cmake --build`.
  - Formatters with `--check`: prettier, black, isort, rustfmt, `ruff format`, `cargo fmt`.
  - Script runners: `npm test` and `npm t`, `npm run <name>`, and for pnpm, yarn and bun also `<runner> <name>`. The name must contain a check word: test(s), build, lint, check, typecheck, tsc, compile, verify, spec(s), e2e, ci, clippy, assemble, validate, coverage, and not watch.
  - Build tools judged by their targets: make, gmake, nmake and mingw32-make (a bare run counts), just, rake, gradle, gradlew, mvn and mvnw (plus package and install for Maven), ant.
  - Shells: `bash`/`sh`/`zsh`/`dash -c`, `pwsh`/`powershell -Command`, `cmd /c` are followed up to 3 levels deep. A script a shell runs is judged by its name.
  - Interpreters: `python -m <tool from the always list>`, `node --test`, or a script whose name, or whose first argument, is a check word (`python manage.py test`).
  - Scripts run directly (`./build.sh`, `.\Run-Tests.ps1`) are judged by their name. `.exe`, `.cmd` and `.bat` are stripped.
- **Answers to the lead:**
  - **Q1:** done. The test `the_inspector_and_the_turn_get_the_same_note` compares `note_in` (the body of `note_for_turn`) with `start_turn` (the body of `begin_turn`) across five stored states. The texts are identical.
  - **Q2:** described above.
  - **Q3:** not needed.
  - **Q4:** confirmed, notes are not stored. `crates/agent/src/turn/runner.rs::view()` adds the app notes to a clone of the turn's user message on each request; the stored user message is saved by the send path before the turn and never has them. The e2e checks this: the Resume request, which follows an earlier turn that also carried a note, contains "Task state, from the app" exactly once.
  - **Feature B's change:** an `edit_file` that fails or is read-only records no changed file, and a unit test checks it.

**3. Tests, and what I saw fail first**
- The red run after wiring in the draft: 49 failed and 8 passed out of 57 in `cargo +1.99 test -p app task::`. The lexer test was among the failures (fixed as above).
- Filled in the order checks, edits, note, server, root. One more failure turned up along the way: `the_definition_is_short`, at 564 characters against its 560 limit, fixed by shortening the description. The final state is 57 of 57.
- Added afterwards, and passed on their first run because the code already existed: the read-only edit test, the same-note test, and the 10 tests in `TaskTab.test.tsx`. The frontend tests were written after the tab, not test first.
- The e2e scenario `task-state`:
  1. A scripted `update_task` sets the goal and steps; the result says "Task saved."
  2. A scripted `write_file`, with its card answered, appears in the tab with the real path.
  3. Pause during a slow streamed reply stops it, and the tab and `get_task` show paused.
  4. After a reload the tab still shows the task, paused.
  5. Resume sends the continue message. The request carries the task note once, with "paused this task and has now resumed it", the goal, the file and the next action.
  6. An `edit_task` made through the command refreshes the open tab by the event.

**4. Checks run**
| Check | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo +1.99 clippy -p store --all-targets -- -D warnings` | clean |
| `cargo +1.99 clippy -p app --all-targets -- -D warnings` | fails, every error in feature F's `src-tauri/src/evals*` (dead code, type_complexity, needless_lifetimes); none in `task*` or my shared lines |
| `cargo +1.99 test -p app` | 387 passed, 0 failed (59 are `task::`) |
| `cargo +1.99 test -p store` | 112 + 4 passed |
| `npm run lint` | clean |
| `npm run typecheck` | clean |
| `npm test` | 34 files, 234 tests passed |
| `task-state` e2e (`flock … PROBES=task-state … xvfb-run -a python3 -I e2e/scripted_agent.py`) | PASS on both runs, 0 console lines |

The full e2e suite was not run.

**5. For ARCHITECTURE.md**
Each chat has one task record (`tasks` table; status and updated_at as columns, the rest as a JSON state), changed only through `Store::change_task`, which reads, changes and writes in one transaction, so a tool call and the user's edit arriving together both take effect. Three writers, kept apart:
- **The model** writes goal, constraints, steps, next action, status (active or done) and notes through the built-in `task` server's `update_task`. It is `Scope::AppData`: no prompt, but still decided by the policy engine and written to the audit log. The chat is always the calling chat. Unknown arguments such as `changed_files`, `checks` or `user` have no way in.
- **The app** records changed files and checks in `task::after_call`, only from its own facts: the policy action's server, scope, read-only flag and canonical path; the approved command line; `is_error`; and the terminal's first line `Exit code N`. Never from what a result says.
- **The user** corrects fields and pauses, resumes, marks done or clears through four main-window-only commands. What the user wrote is marked as theirs, and what they changed but the model has not yet seen wins once over a stale model update.

At the start of every real turn `begin_turn` renders one app note (the same renderer as the read-only `note_for_turn` used by the Context inspector), then makes a paused task active and clears the "since you last saw this" marks. The note is phrased as state and capped at 300 tokens. Every model-written, user-written, path or command text in it is one sanitised line in quotes that cannot close its quotes, start a line or forge an `[App note]`. Command output, the check summaries, never enters the note or the tool result; it is shown only in the Task tab, as plain text. Notes are added to each request and never stored, so at most one task note is ever in a request. Resume is an ordinary user message through the normal send path, and Pause uses the normal stop; neither touches approvals, grants, approved folders or tool switches.

**6. Unverified, and notes for the lead**
- Tested on Linux only. The Windows and PowerShell command forms are covered by unit tests only.
- The full e2e suite was not run, only `task-state`.
- No TODOs remain in my files.
- **D9 side effect (outside my files):** `runner.rs` sets `untrusted = true` after every tool call, including `update_task`. In Ask mode, a `run_command` that was already granted therefore asks again after an `update_task` call.
- **Prompt-cache cost:** because the note is not stored, the server's cached prompt is only valid up to the previous user message, so each new turn reprocesses the whole previous turn. Fixing that needs a change in `crates/agent`.
- **Window of a broken build:** about 25 minutes, between wiring the draft and filling the stubs. During that window the `app` crate compiled with warnings only.
