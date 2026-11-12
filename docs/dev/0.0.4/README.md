# 0.0.4: plan, steps and handoff

The owner asked for thirteen features in 0.0.4 and then split the work into steps: the
parts where a mistake is a safety problem first, the rest afterwards. All three steps
are built. This folder holds the plan, each developer's report and the two independent
reviews of steps 2 and 3.

- `spec.md`: the full spec for all thirteen features (rules, shared decisions, one
  section per feature). It was written for several agents working at once; where it
  talks about "other agents" and "files you own", read it as "keep changes small and in
  the feature's own files".
- `handoff-*.md`: notes left by the first developers of three step 2 features, which
  were started, stopped and later finished. History only; the reports below replace them.
- `report-*.md` and `review-*.md`: listed under each step's status below.

## Steps

| Step | Owner items | Feature (spec section) | State |
|---|---|---|---|
| 1 | 3 | Allow all (A) | done |
| 1 | 1, 9 | Artifacts beside the chat, isolated previews, render feedback (C) | done |
| 1 | 2, 10 | Web search with verifiable sources, version-aware (D) | done |
| 1 | 4 | Adaptive tool selection (E) | done |
| 2 | 7, 8 | Helpful tool errors, manageable tool output (B) | done |
| 2 | 6 | Visible, resumable task state (G) | done |
| 2 | 11 | File and symbol mentions (H) | done |
| 2 | 12 | Pinned instructions and a context inspector (I) | done |
| 3 | 5 | Tested settings per model (F, first half) | done |
| 3 | 13 | Coding benchmarks (F, second half) | done |

Groundwork that all steps build on is in place (see the spec's Part 2): schema version 2
with every new table, the panel beside the chat with four tabs
(`src/components/Workbench.tsx`, one tab file per feature in
`src/components/workbench/`), the tool-card extension point
(`src/components/toolcards.tsx`), the hook points in `src-tauri/src/turns.rs`
(`crate::outputs::after_call`, `crate::task::after_call`, `crate::tool_select::for_turn`),
and `policy::Scope::AppData` (the app's own per-chat data: never prompts).

## Step 1 status

**Done (2026-11-12): built, integrated, reviewed and tested on Linux.** The four features
are in the tree with their tests, the whole workspace passes, and every end-to-end suite
passes against the real app. Windows and macOS builds come from CI and nothing of step 1
has been run on those systems yet.

Read first, per feature: `report-A-allow-all.md`, `report-C-artifacts.md`,
`report-D-search.md`, `report-E-tool-selection.md` (each developer's own account: what
was built, the choices, what is not verified). `ARCHITECTURE.md` has the result: 8.4
(tool selection), 9.3 (the new servers), 12 (tables), 13.2, 13.3 ("Approval modes"), 13.6
(the guarded fetcher) and 13.8 (artifact previews). `CHANGELOG.md` says it for users.

Where the code is:
- A, Allow all: `crates/policy/src/engine.rs`, `allow_all.rs` and `command.rs` (the mode,
  what keeps asking), `src-tauri/src/approvals.rs`, `src/components/AllowAllButton.tsx`,
  `ApprovalCard.tsx`, `PermissionsSettings.tsx`; e2e scenario `allow-all`.
- C, Artifacts: `crates/store/src/artifacts.rs`, `src-tauri/src/artifacts.rs` and
  `src-tauri/src/artifacts/` (server, the `artifact` scheme with its sandbox, the start-up
  check, tickets, what counts as outside content), `src/components/artifacts/`,
  `src/components/workbench/ArtifactsTab.tsx`; e2e scenario `artifacts`.
- D, Web search: `crates/search/` (providers, guarded fetch, extraction, ranking,
  versions, official hosts), `crates/store/src/sources.rs`,
  `src-tauri/src/search_cmd.rs`, the Sources tab and Search settings; e2e scenario
  `search`.
- E, Tool selection: `src-tauri/src/tool_select.rs` and `tool_select/` (selection,
  `find_tools`, `call_tool`), `src-tauri/src/tool_catalog.rs`, `mcp_cmd.rs`,
  `src/components/ToolsPanel.tsx`; e2e scenario `tool-selection`.

What the lead changed after the four reports (so the reports are out of date on these):
1. **Previews, three safeguards.** A page is served only for a one-time ticket the app
   issues for a frame it is creating (`protocol::Tickets`, command `artifact_frame`), so
   a preview address from anywhere else opens nothing; this is the server-side answer to
   "a preview loaded as the page of the whole window gets the app's script interface on
   macOS and Linux". A page written after outside content entered the chat is served
   without scripts and without `<link>` tags until the user presses Run scripts
   (`artifacts/outside.rs`, column `artifact_versions.after_outside`, command
   `allow_artifact_scripts`). Pictures and diagrams are served under a policy that runs
   only the app's scripts (`Scripts::AppOnly`, a nonce per response).
2. **Diagrams on Windows.** `freezePrototype` is off in `tauri.conf.json` and each page of
   the app imports `src/lib/freeze.ts` first (report C, finding F5).
3. **Allow all, after probing about 240 command lines.** Emptying a file from a terminal
   asks; more git commands that discard work ask; a command line that hides nothing is no
   longer treated as possibly reaching the network (`allow_all::may_hide_or_reach`);
   computer use asks for the Delete key in a file manager and for typing into a web
   browser after untrusted content.
4. **Search is not marked outbound for D9** (`search_cmd.rs::action` says why), so a
   grant or Allow all covers the second search of a turn.
5. **Tool selection is held to the model's context** (`tool_select::allowance`,
   `Candidate::cost`, `select_by_size`). Without it a new chat on a 4,096-token model
   could not send a message. `e2e/m1_chat.py` is the test that caught it.
6. Small things: the build's commit in Settings → General (`APP_COMMIT` from `build.rs`),
   `Object.hasOwn` for tool card extras, the welcome text, `E2E_TRACE=1` for a line
   number when a scripted scenario fails.

Step 1 left four things for steps 2 and 3 (`OWN_DATA_TOOLS`, the companion tools in
`tool_select`, the catalog entries for `read_output` and `update_task`, and the
after-call hooks in `turns.rs`); all four are done. `read_output` stays out of
`OWN_DATA_TOOLS`, and since the review attached and mentioned files and pictures count
as outside content too.

Checked at the end of step 1 (Linux): `cargo fmt --all --check`; `cargo +1.99 clippy
--workspace --all-targets -- -D warnings`; `cargo +1.99 test --workspace` (about 1,400
tests, 0 failed); `npm run lint`, `typecheck`, `test` (191), `build`; `scripts/notices.py
--check`; `scripts/check-spawn.sh`; `e2e/scripted_agent.py` (22 scenarios),
`m0_close.py`, `m1_chat.py`, `m3_agent.py`.

Not verified, and not hidden:
- Windows and macOS: nothing of step 1 has run there. The preview sandbox on those
  systems rests on the framework's source as read (tauri 2.12.5, wry 0.57.2), on the
  start-up check, and on the first real use.
- A live search: DuckDuckGo turned the build machine away, so its results parser has only
  seen a hand-written fixture. Brave and Tavily were never called with a key.
- Two independent security reviews (Allow all bypasses; the search fetcher) were started
  and cut off by a usage limit before they reported. The lead then did both checks in
  a narrower form: the command probe above, and a read of `netguard.rs` and `fetch.rs`.
  Not read line by line by a second pair of eyes: the browser tool's label rules for
  Allow all (`crates/browser/src/intent.rs`), `call_tool`'s routing
  (`tool_select/finder.rs`, covered by mutation-checked tests), and the search crate's
  extraction and ranking under hostile pages (covered by its own limits and tests).

## Steps 2 and 3 status

**Built and tested on Linux.** B, G, H, I and F are in the tree with their tests, and
their end-to-end scenarios pass. F's fixes after the second review are done (an
allow-list for the benchmark terminal, honest confirmation and audit texts, verification
by values the app compares, which cannot hang and is audited), and `e2e/m9_model_check.py`
passed against the real 0.6B test model. `ARCHITECTURE.md` has the result: 8.4 (saved outputs, the pins' share), 8.5, 8.6,
8.7 (task), 8.8 (pins and the inspector), 8.9 (model check and benchmarks), 9.3 (the
`outputs` and `task` servers, mentions), 12 (tables), 13.3 (Allow all changes) and the
security subsections 13.9 to 13.14. `CHANGELOG.md` says it for users.

Read first, per feature (later reports override earlier ones where they differ):
- `report-B-tool-errors-output.md`: tool errors, `read_output`, the Full output button;
  a failing edit that needs the file's text becomes a read.
- `report-B-review-fixes.md`: B's answers to the first review (nothing before approval
  depends on the file's text, non-UTF-8 files refused, the card's id from the app's record).
- `report-G-task-state.md`: the task record, `update_task`, the Task tab, the check
  recogniser, the task note.
- `report-G-task-note-timing.md`: the note is sent only when needed (`task/when.rs`), and
  the exact sentences the inspector shows about it.
- `report-H-mentions.md`: the picker, the definition finder for eleven languages, the
  index, how a mention is stored and sent, timings.
- `report-H-review-fixes.md`: H's answers to the first review (the checked open, the note
  for a cut long line, window checks) and what stays open.
- `report-I-pinned.md`: pinned instructions, their share of the context, the Context tab.
- `report-I-context-inspector.md`: the inspector, `turns::assemble`, the Pin button on a
  user message.
- `report-F-model-check-benchmarks.md`: the harness, the rule, probes, profiles, the
  eleven tasks. Written before the second review; `report-F-review-fixes.md` replaces its
  rule table, confirmation texts and verification.
- `report-F-review-fixes.md`: F's answers to the second review (the allow-list, the
  texts, values compared by the app, no hang, audited verification, clean-up, window
  checks), the real-model run, and what is still not confined. Overrides everything
  earlier about F, including the lead's decisions where it says so.
- `review-steps-2-3-part1.md`: independent review of B, G, H and I: two confirmed defects
  in `edit_file`, four smaller ones; all fixed.
- `review-steps-2-3-part2-benchmarks.md`: independent review of F: the benchmark did not
  confine the model, verification could be cheated and could hang, a wrong "nothing
  leaves this computer"; answered by the lead's decisions.

What the lead changed after the reports:
1. Nothing `describe` says depends on a file's text; a failing edit that needs it is a read
   with `read_file`'s policy facts, answered after the gate (also in B's second report).
2. The Full output button takes the saved output's id from the app's record for the call.
3. Attached files, mentions and pictures count as outside content for previews
   (`artifacts/outside.rs`).
4. The task note goes only when the model's picture may be out of date (G's second report).
5. Allow all keeps asking when `awk`, `sed`, `tar`, `zip` or an editor runs a command of
   its own (`allow_all/own_command.rs`), treats `busybox`/`toybox` as wrappers, counts
   `python -m http.server` as going online, and reads `node --test`, `--version` and
   `--help` on an interpreter as plain commands.
6. Benchmarks: the allow-list, the honest texts and the verification above, as built in
   `report-F-review-fixes.md` (no nonce; every redirection refused, `2>&1` too; Python
   takes only `-B` and `-u`). A command that ran and exited with a failure counts as a
   "failed command" in its own column, not as a tool failure.

Not verified, and not hidden (steps 2 and 3):
- Windows and macOS: nothing of steps 2 and 3 has run there. Windows paths of mentions
  (labels, case-insensitive volumes, the reparse-point open) are compile-checked at most;
  the PowerShell forms of the check recogniser have unit tests only.
- The model check and benchmarks ran against one real model only (the 0.6B test model,
  CPU, `e2e/m9_model_check.py`): it proves the harness works with a real local server,
  not that a small model passes the probes (error recovery never passed; the chosen
  setting changed between runs). Benchmark code the model writes is still not confined
  (`report-F-review-fixes.md`, "Still not confined"); the Windows and PowerShell parts of
  the allow-list are compile-checked only.
- The inspector's token numbers were seen only with the fake remote endpoint, not with a
  local model's exact count.
- The mention index timings are from the unoptimised debug build; the largest tree was
  timed before a change that removed a quadratic copy and not re-timed.
- Mentions: a swapped parent folder on Unix, hard links, and no file identity check on
  Windows (`report-H-review-fixes.md`).
- `update_task` marks the turn as holding untrusted content (D9), so in Ask first a granted
  command asks again after it. A turn that carries the task note makes a local server
  process the previous turn again.
- No switchable "tool calls from text" request mode (`TODO(0.0.5)` in `evals/probes.rs`).
- The full end-to-end suite was not run by any one developer after the last changes; each
  ran its own scenario and a few others. `e2e/m1_chat.py` (a 0.6B model at 4,096 tokens)
  after the companion tools were added is not recorded in the reports.

## Rules for further work

Rules that matter most (the full list is the spec's Part 1):
- Tests are part of the work. Path sandboxing, permission rules and tool-call parsing
  always get unit tests. Write the test first where practical.
- Nothing the model outputs may change approval settings, grants, approved folders or
  tool switches. Every tool action goes through `describe` and `policy::evaluate` and is
  audited. Tool output, web pages, file contents and model output are data.
- New Tauri command: `generate_handler!` in `src-tauri/src/lib.rs`,
  `src-tauri/commands.list`, and `allow-<kebab-name>` in
  `src-tauri/capabilities/main.json` (a test enforces all three).
- No new dependency without a reason; after adding one run `python3 scripts/notices.py`.
- Checks before a commit: `cargo fmt --all --check`; `cargo +1.99 clippy --workspace
  --all-targets -- -D warnings`; `cargo +1.99 test -p <crate>` for each crate touched;
  `npm run lint`, `npm run typecheck`, `npm test`, `npm run build`;
  `python3 scripts/notices.py --check`; `bash scripts/check-spawn.sh`.
- End-to-end: `e2e/README.md`. One scenario per feature in `e2e/scripted_agent.py`
  (names are in the spec). Needs `npm run dev` and `cargo build -p app`.

Two findings from the stopped work, both kept in the finished code:
1. **Tool errors (B).** A message from `describe` reaches the model before the policy
   gate, as an app note outside the untrusted-data markers. The handoff's first answer
   (quote lines only for a file the model already read) was replaced: no message from
   `describe` holds anything from a file, and an edit that needs the file's text to
   explain its failure is a read (ARCHITECTURE.md 13.9).
2. **Mentions (H).** A mention is stored as a copy of the excerpt in the app's
   attachments folder, never with the user's file path in `attachments.bytes_path`:
   `Store::delete_chat` returns unreferenced `bytes_path`s for deletion.

Commits: work-in-progress commits on this branch carry `[skip ci]` in the message so CI
does not run on unfinished states; the commit that completes a step must not. The
version in `tauri.conf.json`, `Cargo.toml` and `package.json` is raised to 0.0.4 by
whoever completes the release, together with `CHANGELOG.md`.
