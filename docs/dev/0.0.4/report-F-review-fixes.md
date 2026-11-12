REPORT: feature F, the review fixes (A to D), the window checks, the TODO, and the real-model script

Summary: the review's escapes are now refused, the texts no longer promise what is not true, and verification can no longer be faked, hang or ignore Stop. All checks pass, the `capability-check` scenario passes, and the new real-model script `e2e/m9_model_check.py` passes against the 0.6B test model. Code the model writes still runs unconfined when the benchmark runs it; that is listed plainly under "Still not confined". I did not touch `turns.rs`: the one `[0.0.4 profile]` line is still directly after `input.thinking = …` in `turns::run`.

PER FINDING

1 / A. The texts now tell the truth.
- **Benchmark confirmation** (`src/components/evals/shared.tsx`, `benchConfirm(host)`), exactly:
  - "The app gives the model a fixed set of small coding tasks, one at a time, each in a temporary folder that is removed afterwards."
  - "The model writes code, and the app runs it on this computer with your rights: the task's tests, the model's scripts, and the app's own checks of the result."
  - "The app lets the model run only the task's tests and scripts and commands that look at files, and refuses everything else. Nothing is asked while it runs."
  - "The app cannot see inside code the model writes. Such code could read or change files elsewhere on this computer, or go online."
  - "Run the benchmark only with a model you trust."
  - "Tasks that need Python or Node.js are skipped when they are not installed."
  - Last line, local model: "It can take a long time. The results stay on this computer."
  - Last line, endpoint: "It can take a long time. The tasks and the model's replies go to <host>. The results stay on this computer."
- **Model-check confirmation** (`checkConfirm(host)`), exactly:
  - "The app gives the model three short tasks and tries each with a few settings."
  - "The model gets only tools to read and change files, and only in a temporary folder made for the check. It cannot run commands, and the app runs nothing the model writes. The folder is removed afterwards."
  - "Nothing is asked while it runs: anything that would need your approval is refused."
  - "The settings that work best are saved for this model and used in your chats, unless you chose a setting yourself."
  - Last line, local model: "It can take a few minutes. Nothing leaves this computer."
  - Last line, endpoint: "It can take a few minutes. The tasks and the model's replies go to <host>."
  - I confirmed the check's promise holds: the probes are judged only by reading files and the final answer (`probes::judge`), and nothing is executed.
- The host is the endpoint URL's hostname.
- **Audit previews** (`commands.rs`): both now say the same as the dialogs.
  - Check: "…gets only the file tools, in a temporary folder made for the check, and the app runs nothing the model writes…".
  - Benchmark: "…writes code and the app runs it on this computer with the user's rights… cannot see inside code the model writes: such code could read or change files elsewhere or go online…".
  - Both end with "The model runs on this computer." or "The tasks and the model's replies go to <host>." (`where_it_goes`, unit tested).
- **Tests:** frontend tests assert the exact lists for local and endpoint. A rendered dialog for an endpoint shows the host and not "Nothing leaves". A rendered dialog for the loaded local model shows "Nothing leaves this computer.". The e2e scenario asserts "go to 127.0.0.1." and the new benchmark sentences.

1b, 1c, 6 / B. An allow-list for benchmark commands (`sandbox.rs`, `only_allowed_commands`). It runs on top of every existing check. A command line runs only if the strict parser (`policy::command::parse`) breaks it into simple commands of literal words joined by `|`, `&&`, `||` or `;`, and every command is one of:
- Python (`python3`, `python`, `py`), optionally with `-B` or `-u`, then either `-m unittest` / `-m pytest` (any further arguments), or a `.py` file that resolves inside the workspace (further arguments plain).
- Node.js (`node`, `nodejs`) with a `.js`, `.cjs` or `.mjs` file inside the workspace as its first argument and no flag before it. Or `node --test`, whose arguments are only `--test`/`--test-…` flags and script files inside.
- Looking at files: `ls`, `cat`, `head`, `tail`, `wc`, `grep`, `diff`, `pwd`, `echo`, `dir`, `type`, `findstr`, `Get-ChildItem`, `Get-Content`, `Select-String`, `Get-Location`, `Write-Output`.

Always refused:
- a program given by path;
- any word with `*`, `?` or `[`, quoted or not;
- anything the strict parser cannot break down: every redirection, including `2>&1`; here-documents; background `&`; variables; substitutions;
- `ln`, `sed`, `awk`, `tar`, `cp`, `mv`, `make`, `sh`, `git` and every other program;
- any path argument outside the workspace (the existing check).

Every terminal refusal ends with one sentence, `COMMANDS_ALLOWED`, which is also appended to the run's system prompt when the terminal is offered:

"In this run the terminal runs only the task's tests and scripts and commands that look at files: python3 (or python) with a .py file of the project folder, python3 -m unittest, python3 -m pytest, node with a .js file, node --test, and ls, cat, head, tail, wc, grep, diff, pwd and echo, joined with |, && or ; and written without wildcards or redirections."

The deny-style extras (installers, `~`, variables, `..` summing) are kept, and all their old tests still pass through the allow-list.

New tests, all with the real describers:
- `the_review_escapes_are_blocked`: every line of findings 1b, 1c and 6, with real outside paths.
- `look_alikes_are_blocked`: 36 lines, including your whole list.
- `commands_off_the_allow_list_are_blocked_with_the_list`: the formerly "ordinary" commands that are now refused; each refusal must carry the sentence.
- `every_shipped_task_command_runs`: every command named in a task prompt runs.
- `ordinary_commands_run`: rewritten for the allow-list, including `cat notes.txt | node --test`.
- A harness test that the system prompt carries the sentence.

I saw the three blocked tables fail with the allow-list disabled, then pass with it.

2, 1d / C. Verification can no longer be faked.
- Each task's script now runs in its own process. It only loads the project's code and prints raw results for fixed inputs as `RESULT <json>`, decides nothing, and contains no expected values.
- The app compares those results with `expect` in `task.json`, which is embedded in the binary. Numbers compare as numbers; on failure it reports the first difference, for example "mean12 is {"value":1.0}, expected {"value":1.5}". All 7 script tasks (4 Python, 3 Node.js) are converted.
- Test `tampering_with_the_checking_process_does_not_pass` covers the review's three cheats: the `assertEqual` patch, printing `VERIFIED <__main__.NONCE>` (plus a fake `RESULT {}`), and the Node `assert.strictEqual` patch. All now fail.
- I ran the review's `assertEqual` cheat by hand against the old verifier from git HEAD: it printed `VERIFIED n1`. I did not run the new test against the old scripts.
- Every task still passes when solved and fails when untouched or answered with `wrong/` (`every_task_passes_when_solved_and_fails_when_not`).

3 / C. Verification can no longer hang, and Stop reaches it.
- `run_program` waits on exit, timeout or the cancel token, whichever comes first.
- After that it kills the process group (also after a normal exit) and waits at most 2 s for each output stream before abandoning the reader.
- `verify` and `unparsable` take the run's cancel token. A task cancelled during verification is reported as "stopped".
- Test `a_child_left_holding_the_pipe_does_not_hang_the_check`: a child in another session holding the pipe. It returns in under 6 s; with the bound widened to 100 s I saw it fail after 40 s.
- Test `cancel_ends_a_running_check`: ends in under 2 s.
- Test `a_panicking_run_releases_the_slot`: `RUNNING` was already a drop guard; it is now tested on panic. The slot tests are serialised with a mutex because they share the static.

C. Verification is now audited.
- Each script run writes an audit row under the run's row: server `evals`, tool `verify`. The preview is "Runs the check of <task> on the model's code: <program> -I -B <script>", and the result is `ok` or `failed` with the difference.
- Decision `allowed_without_prompt`, decided by `app`.
- Tested in `every_script_check_gets_an_audit_row`, and seen in the real run.

5 / D. The red evals test is fixed.
- The `is_node_test_runner` exception is removed; the old test is replaced by the allow-list tests.

8 / D. Clean-up.
- On Unix, folders get mode 0o700 before removal and files are left alone. Deleting a file needs only its folder's permission, so nothing becomes writable for others and a hard-linked outside file keeps its mode.
- On Windows only, the read-only flag is cleared on files.
- Test `a_locked_folder_is_removed_and_nothing_becomes_world_writable`: a mode-000 folder tree with a hard link to an outside 0644 file. It checks the folder becomes 0o700, the tree is removed, and the outside file keeps 0644. I did not see it fail first, and since we run as root, removal alone proves little; the mode assertions are what check the behaviour.
- `remove_leftovers()` runs at the start of each check and benchmark. It removes only real folders (never links) directly in the temp folder, named exactly `harness-run-` or `harness-verify-` plus 32 lower-case hex digits, and older than 1 hour. Tested with a temp parent: our names removed; near-miss names, a file and a link with our name kept, and the link's target survives.

7. Not changed, as you allowed: `cd src && cat ../notes.txt` is still refused. `cd` is not on the allow-list anyway.

Lead's addition: window checks.
- `get_model_profile`, `list_bench_tasks`, `list_bench_runs` and `get_bench_run` now check the window first.
- Test `every_command_checks_the_window_first` reads `commands.rs` and requires all 8 commands to start with `only_main_window(window.label())?;`.

TODO(0.0.5): written in `probes.rs::variants`, a switchable "tool calls read from text" variant saved in the profile, which needs an agent-loop mode first.

WHERE I DIFFER FROM YOUR DECISIONS (please correct the docs)
1. The verdict parent is the app itself, not a parent script. There is no nonce any more, since a pass depends only on values. Expected values live in `task.json`, embedded in the binary.
2. `2>&1` and every other redirection are refused, not only those to or from a file: the strict parser allows none. The terminal tool captures stderr anyway.
3. Python accepts only `-B` and `-u` before `-m` or the file, so `py -3 x.py` is refused.
4. PowerShell commands are matched by name only. `type`, `dir`, `cat` and `ls` count by name; aliases such as `gci`, `gc` and `sls` are not listed.
5. Left-over removal skips folders younger than 1 hour, so it cannot delete a run in progress (tests run in parallel).
6. The allow-list sentence is added to every refused terminal call, including those the policy engine refuses first.

CHECKS RUN
- `cargo fmt --all --check`: clean.
- `cargo +1.99 clippy -p app --all-targets -- -D warnings`: clean.
- `cargo +1.99 test -p app --lib evals::`: 75 passed, 0 failed.
- `cargo +1.99 test -p app`: 419 passed. That run was before the last window-check edit; after it I re-ran `evals::` (75 passed) and clippy (clean), not the full suite.
- `npm run lint`: clean. `npm run typecheck`: clean. `npm test`: 36 files, 247 passed. The 9 evals frontend tests passed again after the last frontend edit.
- e2e `capability-check`: PASS. Endpoint text, check, profile, audit, temperature in a chat turn, Forget, and the benchmark through the UI: 0 of 11 completed, saved, audit `ok`.
- e2e `m9_model_check.py` (real model): PASS. Run through `flock /tmp/harness-e2e.lock … timeout 5400 xvfb-run -a python3 -I e2e/m9_model_check.py`, after `cargo +1.99 build -p app`.

THE SCRIPT (`e2e/m9_model_check.py`)
- Loads `TEST_GGUF` (default Qwen3 0.6B Q8_0) in the real app.
- Starts the check from Settings → Local models through its confirmation, and waits up to 60 minutes for it to end by itself (`M9_CHECK_MINUTES`).
- Asserts:
  - every setting tried is in the table with passed/failed per probe;
  - no new temp folders are left;
  - the approved folder and the home folder (outside the app's own folders) are unchanged;
  - one run row with result `ok`, and call rows with no chat and the "Model check: " prefix.
- Prints what passed; it does not assert that probes pass.
- Then starts the benchmark through its confirmation (checking the new wording), waits until the third task starts, and clicks "End the benchmark".
- Asserts it stops within 5 s, and that the partial run is saved with `cancelled: true`: 2 finished tasks, the third "stopped". By design a partial run is kept and marked "ended early" in the list.
- Also asserts no folders are left and one benchmark run row with result `cancelled`.

WHAT THE REAL MODEL DID (CPU backend, the model can think: 4 settings)
- **First run** (before the fixes), 636 s:
  - Default: tool calling hit the 180 s limit after one step that only thought; editing passed; error recovery failed.
  - Temperature 0.2 passed tool calling and editing, so it was chosen.
- **Second run** (with the fixes), 231 s:

| Setting | Tool calling | Editing | Error recovery |
|---|---|---|---|
| Default (chosen) | passed | passed | failed |
| Thinking off | failed | passed | failed |
| Temperature 0.2 | passed | passed | failed |
| Thinking off, temperature 0.2 | failed | passed | failed |

- All calls arrived as native calls.
- **Surprises**
  - **The rule blocked a call twice, correctly.** In both "thinking off" variants the model asked for `read_file` with path `/note.txt`, a path at the filesystem root, which the rule refused as outside the folder. The refusal text comes from the file tools and says "the user can approve another folder in the Tools panel". That doesn't fit a headless run, but the wording belongs to feature B.
  - **Error recovery never passed with this model.** It repeated the same ambiguous edit without `occurrence`, or passed the folder or a made-up path as `path`; once it produced wrong content. I think this probe probably can't pass with a 0.6B model.
  - **The chosen setting changes between runs.** Small differences decide the tie-break, so the chosen setting is not stable for this model.
  - **Tool failures count test failures.** In the benchmark (both runs), `python3 -m unittest -q` ran under the allow-list. It counts as a tool failure because the terminal reports exit code 1 as an error; that is a property of the metric.
  - **The model did not solve the first two tasks.** On py-fix-failing-test it ran the tests and then stopped ("mean12 is 1.0, expected 1.5"). On py-add-feature it made no call.
- **No hang.** Stop took 0.3 s. The verify rows are in the audit.

DEFECTS FOUND BY THE REAL RUN
None in the harness. Earlier, the scripted e2e had found the benchmark audit-writer hang, which was fixed then (the bench setup is dropped before the audit closes).

STILL NOT CONFINED (for the release notes)
- When the benchmark runs code the model wrote, that code runs with the user's full rights and can read, change or delete any file the user can, and go online. That code is the task's tests (`unittest`, `pytest`, `node --test` import model-written files), the model's scripts (`python3 x.py`, `node x.js`), and the app's verification (which loads the model's module).
- The allow-list only limits the command lines the model types. It cannot see what a script does.
- A process started by such code in a new session survives the run; the app stops waiting for it after 2 s but does not kill it.
- Such code can also read the verification script and the app's files.
- Real confinement needs an OS sandbox (bubblewrap, sandbox-exec, AppContainer); none exists yet.
- The capability check is unaffected: file tools only, and nothing the model writes is run.
- Only tested on Linux. The PowerShell and Windows paths of the allow-list are compile-checked only.
