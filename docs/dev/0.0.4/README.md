# 0.0.4: plan, steps and handoff

The owner asked for thirteen features in 0.0.4 and then split the work into steps: the
parts where a mistake is a safety problem first, the rest afterwards by another model.
This folder is what the next developer needs to continue.

- `spec.md`: the full spec for all thirteen features (rules, shared decisions, one
  section per feature). It was written for several agents working at once; where it
  talks about "other agents" and "files you own", read it as "keep changes small and in
  the feature's own files".
- `handoff-*.md`: notes left by the developers of three step 2 features that were
  started and then stopped: what exists, what is left, and decisions to keep.

## Steps

| Step | Owner items | Feature (spec section) | State |
|---|---|---|---|
| 1 | 3 | Allow all (A) | done |
| 1 | 1, 9 | Artifacts beside the chat, isolated previews, render feedback (C) | done |
| 1 | 2, 10 | Web search with verifiable sources, version-aware (D) | done |
| 1 | 4 | Adaptive tool selection (E) | done |
| 2 | 7, 8 | Helpful tool errors, manageable tool output (B) | started: `handoff-B-tool-errors-output.md` |
| 2 | 6 | Visible, resumable task state (G) | started: `handoff-G-task-state.md` |
| 2 | 11 | File and symbol mentions (H) | started: `handoff-H-mentions.md` |
| 2 | 12 | Pinned instructions and a context inspector (I) | not started |
| 3 | 5 | Tested settings per model (F, first half) | not started |
| 3 | 13 | Coding benchmarks (F, second half) | not started |

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

For whoever builds steps 2 and 3, the places where step 1 expects you to do something:
- `artifacts/outside.rs`, `OWN_DATA_TOOLS`: the tools whose results are the app's own data
  and do not count as outside content. `update_task` is already there. `read_output` must
  stay out (it returns another tool's result).
- `tool_select`: `read_output` and `update_task` are companions and always shown. Each
  costs room in a small context: run `e2e/m1_chat.py` (a 0.6B model at 4,096 tokens)
  after adding them, and look at `select_by_size` if a turn is refused again.
- `tool_catalog.rs` has entries for `read_output` and `update_task` checked against the
  agreed shapes only; `tool_catalog_problems` (first assertion of the `tool-selection`
  scenario) checks them against the real servers once those are registered.
- The after-call hooks in `turns.rs` (`outputs::after_call`, `task::after_call`) are still
  the no-op stubs.

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

## For steps 2 and 3

Suggested order: B, G, H, I, then F. I depends on G (the inspector shows the task note)
and needs `turns.rs::run` split so one function assembles what a turn sends; do it when
nothing else is changing that function. F builds on Allow all's rules (its sandbox
policy answers cards by rule) and on B's error accounting.

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

Two findings from the stopped work that the next developer must keep:
1. **Tool errors (B).** An `edit_file` error is produced in `describe`, before the
   policy gate, and reaches the model as an app note outside the untrusted-data
   markers. Quoting file lines in such an error would let a failing edit read lines of
   any file in an approved folder with no approval. Quote lines only for a file the
   model has already read in this chat; otherwise give line numbers and the `read_file`
   call to make. (Details in `handoff-B-tool-errors-output.md`.)
2. **Mentions (H).** Store a mention as a copy of the excerpt in the app's attachments
   folder, never with the user's file path in `attachments.bytes_path`:
   `Store::delete_chat` returns unreferenced `bytes_path`s for deletion.
   (Details in `handoff-H-mentions.md`.)

Commits: work-in-progress commits on this branch carry `[skip ci]` in the message so CI
does not run on unfinished states; the commit that completes a step must not. The
version in `tauri.conf.json`, `Cargo.toml` and `package.json` is raised to 0.0.4 by
whoever completes the release, together with `CHANGELOG.md`.
