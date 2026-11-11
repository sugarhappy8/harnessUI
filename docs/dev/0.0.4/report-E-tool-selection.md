# Report E: adaptive tool selection (interim, 2026-11-11 19:55 UTC, saved by the lead)

State: built end to end; own tests pass; the e2e scenario `tool-selection` passed in the
real app; a run of the whole scripted suite was in progress (18 older scenarios passed).

- Checks at the last full run: `cargo +1.99 test -p app` 147 passed, `tool_select` 41
  passed after the last change, clippy for the app crate clean, frontend lint, typecheck
  and `npm test` (104) passed.
- `call_tool` safety: `src-tauri/src/tool_select/finder.rs` (`Routing`) turns a
  `call_tool` call into its target before validate, describe and execute: the target's
  schema validation with the loop's argument repair, the target's own describer and the
  user's switches for it, card name and audit row from the target's action. Only tools
  in the turn's two lists (shown, hidden) are found. `call_tool`, `find_tools` and
  `run_subagent` are refused as targets. Unit tests in `tool_select/finder/tests.rs`;
  six whole-turn tests with the real loop and policy engine in
  `tool_select/finder/tests/through_the_loop.rs`; four mutation checks, all caught.
- Files: new `src-tauri/src/tool_catalog.rs`, `tool_select/finder.rs`,
  `tool_select/tests.rs`, `tool_select/finder/tests.rs`,
  `tool_select/finder/tests/through_the_loop.rs`, `src/components/CalledTool.tsx` and
  its test. Changed: `tool_select.rs`, `mcp_cmd.rs`, `ToolsPanel.tsx` and its test.
  Shared: `lib.rs` (mod line, 3 handlers), `commands.list`, `capabilities/main.json`
  (commands `get_tool_selection`, `set_tool_selection`, `tool_catalog_problems`),
  `toolset.rs` (1 line), `types.ts`, `ipc.ts`, `toolcards.tsx` (1 entry),
  `e2e/scripted_agent.py`, and `turns.rs`.
- `turns.rs` edits outside the marked points, all tagged `[0.0.4 tool selection]`:
  `ChatTools` gets `shown` and `found`; `AppApprover::ask` takes the card's tool name
  from the action; `audit_entry` logs the real tool and its arguments; `run` loads the
  user's tool switches for hidden tools too (otherwise a hidden external tool would lose
  "always ask" through `call_tool`) and counts a hidden terminal as on for the skills
  server.
- Re-run at integration: `cargo +1.99 test -p app tool_catalog`, and the e2e scenario
  `tool-selection` (its first assertion calls `tool_catalog_problems` in the running
  app, which checks every registered built-in tool against the catalog with the real
  schemas). `outputs`, `search` and `task` were not registered when this was checked;
  their entries are only checked against the argument shapes in the spec's Part 2.
- Deliberate choices: a chat whose tools all fit under the cap is shown all of them; the
  list is in `enabled_for_chat` order (it grows in place, not by appending at the end);
  with the automatic switch off, `find_tools` and `call_tool` are not offered;
  `run_subagent` and `load_skill` are always shown when on.
