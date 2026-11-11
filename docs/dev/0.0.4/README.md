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
| 1 | 3 | Allow all (A) | see "Step 1 status" below |
| 1 | 1, 9 | Artifacts beside the chat, isolated previews, render feedback (C) | see below |
| 1 | 2, 10 | Web search with verifiable sources, version-aware (D) | see below |
| 1 | 4 | Adaptive tool selection (E) | see below |
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

STEP1_STATUS_PLACEHOLDER

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
