REPORT: feature F (tested settings per model, coding benchmarks)

Summary: everything in the brief is built and wired. The app tests pass (404), the frontend checks pass, and the end-to-end scenario `capability-check` passes in the real app. My code and files are clippy-clean. The final clippy run is blocked by another developer's unfinished file (`crates/agent/src/turn/runner/preview.rs`); details in section 4.

1. FILES

New (all mine):
- `src-tauri/src/evals.rs`: module root. The `#![allow(dead_code)]` is gone.
- `src-tauri/src/evals/`:
  - `sandbox.rs` and `sandbox/tests.rs`: the draft rule, kept, plus one exception and new tests.
  - `workspace.rs`, `harness.rs` and `harness/tests.rs`.
  - `profile.rs`, `probes.rs` and `probes/tests.rs`.
  - `tasks.rs`, `verify.rs`, `bench.rs` and `bench/tests.rs`.
  - `commands.rs`, `testing.rs` (helpers added).
- `bench/tasks/<11 tasks>/`, each with `task.json`, `files/`, `verify/`, `solution/` and sometimes `wrong/`.
- `src/components/evals/`: `types.ts`, `shared.tsx`, `ModelCheck.tsx`, `Benchmarks.tsx`, `evals.test.tsx`.

Shared files, exactly what I added:
- `src-tauri/src/lib.rs`: `mod evals;` and 8 entries in `generate_handler!`. The commands are `get_model_profile`, `forget_model_profile`, `start_capability_check`, `cancel_eval`, `list_bench_tasks`, `start_benchmark`, `list_bench_runs`, `get_bench_run`.
- `src-tauri/commands.list`: the same 8 names. `src-tauri/capabilities/main.json`: the 8 `allow-…` grants.
- `src-tauri/src/turns.rs`, two additions:
  - In `ChatTools`, a field `thinking_chosen: bool` (serde default, not stored when false), marked `[0.0.4 profile]`.
  - In `run`, right after `input.thinking = !chat_prefs.thinking_off;`, the one marked line: `crate::evals::profile::apply_to_turn(&state, &req.model_ref, &chat_prefs, &mut input).await; // [0.0.4 profile]`. I re-read the file after your notice: there is no shared assembly function yet, so the line stays where `input.thinking` is set today. Whoever restructures can move it as is.
- `src-tauri/src/mcp_cmd.rs::set_chat_thinking`: the closure also sets `prefs.thinking_chosen = true`, marked.
- `src-tauri/src/chat.rs::get_chat`: `dto.thinking = crate::evals::profile::chat_thinking(&state, &chat).await;`, marked. Before that it builds the dto with `chat_dto` as it did.
- `src-tauri/src/toolset.rs`: `pub fn captured_env_now()`, a 4-line read-only accessor for `CAPTURED`, marked `[0.0.4 evals]`. The benchmark terminal and toolchain detection need it on macOS.
- `src/lib/ipc.ts`: one block of 9 wrappers, including `onEvalProgress`.
- `src/components/SettingsDialog.tsx`: two imports; `<ModelCheck />` after `<Runtime />` and `<Benchmarks />` after `<Diagnostics />`.
- `e2e/scripted_agent.py`: `p_capability_check` and one `PROBES` entry.

No new dependencies. No new model tool: the harness uses the existing file and terminal tools, so `OWN_DATA_TOOLS` is unaffected.

2. WHAT THE USER SEES, AND THE DESIGN

Settings → Local models, section "Tested settings":
- A model picker shows the loaded local model and the endpoints' models.
- "Check this model" opens a confirmation. The button is disabled with a reason when the model can't be checked, for example "Load this model to check it.".
- After a check: the date, a sentence on what was chosen, and a table. Each row is one setting; the columns are passed or failed for each of the three probes, how tool calls arrived ("native", "from text" or "both"), and tool failures.
- "Forget these settings" removes the profile. "End the check" stops a running check.

Settings → Diagnostics, section "Coding benchmarks":
- It says which tasks will be skipped, for example "Python not found: 4 tasks will be skipped".
- "Run the benchmark" opens a confirmation. "End the benchmark" stops a run.
- A list of saved runs. A table per run: task, result, broken edits, tool failures, blocked, steps, tokens, time, and a totals row.
- "Compare with" shows two runs side by side.

What the user reads before a run starts (exact text). The dialog shows "Model: <name>", Cancel, and a start button that ignores clicks for its first 0.7 s.
- Title "Check this model?", button "Start the check":
  - "The app gives the model three short tasks and tries each with a few settings."
  - "The model will read and change files in a temporary folder made for the check, and nowhere else. The folder is removed afterwards."
  - "Nothing is asked while it runs: anything that would need your approval is refused."
  - "The settings that work best are saved for this model and used in your chats, unless you chose a setting yourself."
  - "It can take a few minutes. Nothing leaves this computer."
- Title "Run the benchmark?", button "Start the benchmark":
  - "The app gives the model a fixed set of small coding tasks, one at a time."
  - "For each task the model will read and change files and run commands in a temporary folder made for that task. The folder is removed afterwards."
  - "Nothing is asked while it runs: commands that delete, go online or cannot be understood are refused."
  - "Tasks that need Python or Node.js are skipped when they are not installed."
  - "It can take a long time. The results stay on this computer."
- No button label contains Allow once, Deny, Save, Use, Continue, Send or Stop; a test enforces this.

Harness (`harness.rs`):
- It runs the real `agent::turn::run_turn`. Its tool host is private to the run (design decision 1): `FilesServer`, plus `TerminalServer` for benchmarks only.
- Only the rule's tools are offered to the model. Every call is described by its own tool and decided by `sandbox::Gate`, which calls `policy::evaluate`. The approver is `NobodyToAsk`: it shows no card and always answers no.
- `execute` asks the rule again just before the tool runs.
- It records per call: tool, status, blocked, recovered-from-text, detail and time. Per run: steps, prompt and completion tokens, final answer and status (`completed`, `stopped`, `timeLimit`, `stepCap`, `mistakes`, `loop`, `failed`).
- Cancel goes through a child token, and a timer enforces the time limit. Failed edits are counted from the finished call's status, never from error wording, so feature B's read-only `edit_file` lookup is counted correctly. A test proves that lookup runs inside the workspace and is refused outside it.

Approver rule table, as implemented (`sandbox.rs`):

| The call | Decision |
|---|---|
| A tool that is not part of the run (anything but `list_dir`, `read_file`, `search_files`, `write_file`, `edit_file`, plus `run_command` in benchmarks; also `delete_file`) | blocked |
| A hard limit: path outside the run's folder (also via `..` or a link), the app's own folders | blocked |
| Anything `policy::evaluate` would not allow by itself in Allow all, with the turn treated as untrusted from the first call (Ask, Denied, Refused, or a grant-only scope) | blocked |
| Any `keep_asking` (delete or empty a file, destroy, sensitive, publish, hidden, protected app, marked destructive) | blocked |
| `network` (a network program, or a command that cannot be fully read) | blocked |
| `outbound`, except a terminal command, which every command has; commands are judged by the other rows | blocked |
| A file action whose resolved `canonical` path is not inside the folder; a command whose working folder is not inside | blocked |
| A command word naming a place outside (paths read from the working folder, links followed, `..` summed over the line), `~`, or a variable | blocked |
| Package installers and fetchers (`pip`, `npx`, `npm install`, `cargo install`, `python -m pip`, `docker`, …) | blocked |
| `node --test` alone, with only `--test`/`--test-…` flags | read as a plain command, then judged by the rows above |
| Everything else: reads, writes and edits inside the folder, plain commands such as `python3 -m unittest`, `python3 test_x.py`, `node test.js`, `node --test` | runs |

The gate only returns `AllowedByMode` or `Denied`, never `Ask`.

Design decisions 1 to 7: all kept. Decision 2, the strict command rule, is unchanged. Two corrections:
- The `node --test` row above. Feature A's classifier marks an interpreter called without a file as reading its code from standard input, so `node --test` came out with `network: true` and was blocked. The test runner reads no code from standard input. The exception covers only one plain `node`/`nodejs` command whose flags are all `--test` or `--test-…`; tests show that anything added to it is still blocked. Feature A may want to fix this in `crates/policy/src/allow_all.rs` itself.
- Decision 7 is done with a different set of 11 tasks; the list is below.

Profile format. Key: `modelProfile:` followed by the first 16 hex digits of a SHA-256 of the identity: `local\n<file name>\n<size>\n<backend>` or `remote\n<base URL>\n<model>`. Example value:

```json
{"version":1,"identity":{"kind":"remote","url":"http://127.0.0.1:5000/v1","model":"fake-model"},
 "label":"fake-model","checkedAt":1762911441000,"chosen":"Temperature 0.2",
 "defaults":{"temperature":0.2},
 "results":[{"label":"Default","settings":{"thinkingOff":false,"temperature":null},"passed":0,"toolFailures":1,
   "probes":[{"probe":"toolCalling","passed":false,"arrival":"native","toolFailures":0,"failedEdits":0,"blocked":0,
              "steps":2,"tokens":1020,"timeMs":35,"status":"completed","note":"The reply did not contain the word from the file."}, ...]}, ...]}
```

- A local model gets a profile identity only while it is the loaded model, on the current backend.
- Variants: Default and "Temperature 0.2"; when the model can think, also "Thinking off" and "Thinking off, temperature 0.2".
- Best variant: most probes passed, then fewest tool failures, then list order, so the defaults win a tie. If nothing passes, `chosen` and `defaults` are null and nothing changes for that model.
- `apply` works on `RequestDefaults {thinking, temperature}` only, so a profile cannot reach approval modes, grants, folders or tool switches.
- Thinking comes from the profile only when the user has not chosen. `user_chose_thinking = thinking_chosen || thinking_off`, so an old chat with thinking off keeps it, and an untouched old chat behaves exactly as before until a profile exists. Temperature comes from the profile only when the request has none.
- `set_setting`'s allow-list does not accept profile keys (a test reads the allow-list). `forget_model_profile` accepts only keys of the form `modelProfile:` plus 16 hex digits.

Probes (file tools only, no terminal):
- Tool calling: read `note.txt` and reply with its code word, which is random for every run. Pass requires a successful `read_file` and the word in the answer.
- Editing: `color = blue` becomes `color = green`. Verified by exact file content plus a successful `edit_file`.
- Error recovery: `limit = 10` appears twice; only the `[server]` one must change. Verified by file content, never by error wording.

Native versus text tool calls: recorded per probe and per task (`arrival`) and shown in the table. `crates/agent` is untouched.

Benchmarks: the 11 tasks, what each verifies, and the toolchain it needs. "Run" means a shipped script run by the app; `unchanged` means the file is exactly as shipped.

| Task | Verification | Needs |
|---|---|---|
| `py-fix-failing-test` | run hidden tests on `mean`/`median`; `test_stats.py` unchanged | Python |
| `py-add-feature` | run tests of `TodoList.complete`/`pending`, including KeyError | Python |
| `py-fix-from-error` | run `summary()` on fresh data and on the real file ("3 orders, total 20.00"); `orders.json` unchanged | Python |
| `py-rename-across-files` | run: `get_user` works and `get_usr` is gone from the module; no `.py` file contains `get_usr` | Python |
| `js-fix-failing-test` | run hidden `slugify` cases; `test.js` unchanged | Node.js |
| `js-add-feature` | run `Cart.total(discount)` cases | Node.js |
| `js-fix-from-error` | run `displayName` cases ("(no name)"); `users.json` unchanged | Node.js |
| `config-json-settings` | JSON pointers: `/server/port` = 8080, `/logLevel` = "debug", others kept; JSON still parses | none |
| `config-ini-section` | INI: `[database]` timeout 30 and pool 4, host kept; `[cache]` unchanged | none |
| `docs-readme-install` | README parts in order: intro, `## Install`, code block, `pip install tidy-notes`, `## Usage`, … | none |
| `docs-changelog-release` | `package.json` `/version` = "1.3.0" (others kept); CHANGELOG parts in order | none |

- Tasks reach the app embedded with `include_bytes!` (the listing in `tasks.rs`). A test checks the listing matches the folder exactly. `solution/` and `wrong/` are embedded only under `cfg(test)`.
- The `{python}`/`{node}` placeholders in prompts are replaced with the program found. A toolchain counts as found only if `--version` runs, which matters for the Windows Store stub.
- Script checks: the script is written to a separate `harness-verify-…` temp folder after the model finished. It runs through `runtime::spawn` with `cwd` set to the workspace, a minimal environment and a 60 s limit; Python runs with `-I -B`, Node scripts are `.cjs`. A random nonce goes on stdin and is read before any of the model's code loads. Pass means exit 0 and a last line of `VERIFIED <nonce>`.
- Broken edits = failed edit/write calls + files left not parsing (JSON built in; `.py` through `ast.parse` under `-I`; `.js` through `node --check`).
- Metrics per task and in total: completed, broken edits, tool failures, blocked, steps, tokens, time.
- Results are saved to `<app local data>/benchmarks/<YYYYMMDD-HHMMSS-xxxx>.json`. Ids are validated, writes are atomic, and the newest 50 runs are kept.
- A benchmark uses the model's profile defaults when it has a profile.

Audit:
- Each run writes one row: `chat_id` none, server `evals`, tool `capability_check` or `benchmark`, `allow_once` by `user`, and a preview stating what the run does. The result is filled when the run ends (`ok` or `cancelled`, with a summary).
- Each call gets a row built with `turns::audit_entry`, with `chat_id` none, `parent_audit_id` pointing at the run row, and a preview prefixed "Model check: " or "Benchmark: ". `audit_log.chat_id` is already nullable with no foreign key, so the table is unchanged.
- One run at a time (a static guard). `cancel_eval` stops a run in about 0.3 s in tests. All commands check for the main window.
- Workspace: `std::env::temp_dir()/harness-run-<uuid>`, created with `create_dir`, removed on drop. That covers success, failure, panic and cancel, and covers read-only files. Removal never follows links. There is no way to point a run at a path.

3. TESTS

Rust, in the `evals` module: 64 tests.
- sandbox 17: the 15 from the draft plus `node --test` and the read-only edit lookup.
- workspace 5, harness 9, probes 10, profile 8, tasks 2, verify 1, bench 7, commands 3.

What they prove, besides the brief's mandatory approver cases (write inside runs; write outside, delete, network command, unreadable command, and `..` and links are refused):
- Every task passes when solved and fails when left untouched. The 6 tasks with a `wrong/` answer (py-fix-failing-test, py-rename-across-files, js-fix-from-error, config-json, config-ini, docs-readme) fail with it.
- Code that prints `VERIFIED …` itself fails.
- A JSON file left broken counts as a broken edit.
- A task whose toolchain is missing is skipped without asking the model anything.
- Stop ends a run in under 2 s; the time limit stops a run.
- The profile tests the lead asked for: an explicit choice beats the profile; no profile means today's behaviour; a profile changes only thinking and temperature (all other `TurnInput` fields compared).

Frontend: `evals.test.tsx`, 6 tests. Nothing starts before the user confirms; the confirmation contains the required facts; the start button ignores early clicks; no forbidden label appears; the results table, chosen settings, Forget, the skipped-task note, and the run and compare tables.

Seen failing before the code was right:
- The draft's `ordinary_commands_run` failed on `node --test` before the exception existed.
- `every_task_passes…` failed on js-add-feature for the same reason.
- `refused_calls…` failed because it expected `delete_file` to be blocked. In fact the agent loop refuses a tool that was not offered before any describer runs; that is safer, so I corrected the expectation.
- `node --test --require ./x.js`: I expected it blocked, but it runs a file inside the folder, so the rule is right.
- e2e: the first benchmark run through the UI hung after its last task. The audit writer's sender was still held, so closing the audit waited forever; fixed by dropping it.
- Most other tests were written with or right after the code; I did not see them fail first.

4. CHECKS RUN
- `cargo +1.99 test -p app`: 404 passed, 0 failed, including the command-registration test. An earlier full run gave 387 passed.
- `cargo +1.99 test -p app --lib evals::`: 64 passed.
- `cargo +1.99 clippy -p app --all-targets -- -D warnings`: clean earlier today, including all of `src-tauri/src/evals*`. The final 5 tries failed only in another developer's file, `crates/agent/src/turn/runner/preview.rs:87` ("match can be simplified with unwrap_or_default"); one try also hit `crates/tools/src/symbols/resolve.rs:280`.
- `cargo fmt --all --check`: no diffs in my files. The remaining diffs are in others' files: `crates/agent/tests/turn_loop.rs`, `crates/store/src/pinned.rs`, `src-tauri/src/pinned/tests.rs`.
- `npm run lint`: clean. `npm run typecheck`: clean. `npm test`: 34 files, 234 tests, all passed.
- e2e `capability-check`: PASS. It checks:
  - the confirmation text;
  - 2 settings × 3 probes against the scripted endpoint, with "Temperature 0.2 (chosen)" in the table;
  - the saved profile, and `set_setting` refusing its key;
  - the audit (1 run row, 7 call rows, no chat);
  - temp folders removed;
  - a chat turn sending `temperature: 0.2`, and none after Forget;
  - a benchmark through Diagnostics: "0 of 11 completed", the run saved as JSON, listed and loaded, audit row `ok`.

5. FOR ARCHITECTURE.md

Tested settings and benchmarks (`src-tauri/src/evals/`) share one headless harness. It runs the real agent loop against the loaded local model or a configured endpoint, in a fresh folder under the system temp folder (`harness-run-<uuid>`) that is removed when the run ends, however it ends. The harness builds its own tool host with the file tools (plus the terminal for benchmarks), so the user's approved folders, grants, MCP servers and tool switches play no part. The run's only approved folder is its workspace, and the app's forbidden folders stay forbidden.

Every call goes through its tool's describer and `policy::evaluate`, under a fixed gate. It evaluates in Allow all with the turn treated as untrusted from its first call. It then also refuses anything `keep_asking`, `network`, a hard limit, non-terminal `outbound`, paths that resolve outside the workspace, command words that name places outside it (including `..`, links, `~` and variables), and package installers. Whatever is not allowed is refused and counted as blocked. The approver never shows a card and never allows anything.

Each run and each of its calls is written to the audit log (run row by the user, call rows linked to it, no chat). Verification is shipped data embedded in the binary. Scripts are copied to a separate folder after the model finished and run by the app through `runtime::spawn` with a time limit and a nonce passed on stdin. Pass/fail never comes from model text; the one exception is the tool-calling probe's answer, which must contain a word the model could only have read from the file. Built-in checks read the workspace.

Nothing runs unless the user starts it after a plain-words confirmation. One run at a time; Stop works within a second. Results stay local: profiles in the settings table under `modelProfile:<hash>` of (model file name, size, backend) or (endpoint URL, model), and benchmark JSON in `<app data>/benchmarks`. The generic `set_setting` command cannot write a profile. A profile only fills in request defaults (thinking, temperature) that the user has not chosen (`ChatTools.thinking_chosen`), through one call in `turns::run` and the effective value in `get_chat`. It cannot touch approvals, grants, folders or tool switches.

6. UNVERIFIED, AND TODOs
- Not run against a real model. Every run used the scripted endpoint; the llama-server in `/home/user/llama-dev` was not used.
- Only run on Linux. The Windows paths (Python program names, `percent_var`, PowerShell parsing in the rule) are compile-checked, and the macOS captured PATH is only exercised through the accessor.
- `TODO(0.0.5)`: a switchable "tool calls read from text" request mode as a profile variant. It needs an agent-crate mode that describes tools in the prompt; today only the arrival is recorded. This TODO is in this report but not yet written into the code.
- Not done: a check or benchmark does not wait for a chat reply that is generating on the same local server; both requests go to the server and queue there.
- A bench task's `timeMs` counts the model's turn only, not the verification time.
