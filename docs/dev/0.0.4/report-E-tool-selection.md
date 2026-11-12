<!-- Saved by the lead from the developer's final message; the developer could not write this file itself. -->

# Report E: adaptive tool selection (owner item 4)

Built end to end, tests green, e2e scenario passes in the real app. Two things first:
- I could not write docs/dev/0.0.4/report-E-tool-selection.md: the Write tool refuses report files from a subagent. I did not work around it. This message is the report; save it there if you want the file.
- My first interim message wrongly listed the older e2e scenario `terminal-git` as passing. It failed (cause in section 4). I sent a correction, and I see you have since changed that scenario's message.

1. FILES

Created (mine):
- /home/user/harnissUI/src-tauri/src/tool_catalog.rs (catalog, example and argument helpers, the check, tests)
- /home/user/harnissUI/src-tauri/src/tool_select/finder.rs (`toolfinder` server, the `call_tool` unwrap `Routing`, `find_tools` search and answer)
- /home/user/harnissUI/src-tauri/src/tool_select/tests.rs
- /home/user/harnissUI/src-tauri/src/tool_select/finder/tests.rs
- /home/user/harnissUI/src-tauri/src/tool_select/finder/tests/through_the_loop.rs
- /home/user/harnissUI/src/components/CalledTool.tsx, /home/user/harnissUI/src/components/CalledTool.test.tsx

Changed (mine): src-tauri/src/tool_select.rs (rewritten), src-tauri/src/mcp_cmd.rs, src/components/ToolsPanel.tsx, src/components/ToolsPanel.test.tsx.

Shared files, exactly what I added:
- src-tauri/src/lib.rs: `mod tool_catalog;` and three handler entries: `tool_select::get_tool_selection`, `tool_select::set_tool_selection`, `tool_select::tool_catalog_problems`.
- src-tauri/commands.list and src-tauri/capabilities/main.json: the same three commands.
- src-tauri/src/toolset.rs: one line, `host.add_builtin(Arc::new(crate::tool_select::ToolFinderServer));`, with a comment.
- src-tauri/src/turns.rs, every edit tagged `[0.0.4 tool selection]`:
  1. `ChatTools`: fields `shown` and `found` (what `chats.tool_prefs` remembers).
  2. `AppApprover::ask`: the card's `tool` is `finder::real_tool(&req.tool, Some(&req.action))`.
  3. `impl AppTools`: a `routing()` helper.
  4. `AppTools::validate` and `AppTools::describe`: go through `Routing`.
  5. `AppTools::execute` at the marked point: route. `find_tools` is answered there; `call_tool` becomes the target and then runs the unchanged call path and both after-call hooks under the target's name.
  6. `audit_entry`: tool and arguments from `finder::audited`.
  7. `run`: the user's tool switches are loaded for hidden tools too, and `set_terminal_enabled` also looks at hidden tools.
  Points 1, 2, 3, 6 and 7 are outside the marked points. They are needed. Without 2 and 6 the card and the audit row say `call_tool`. Without 7 a hidden external tool loses the user's "always ask" switch when run through `call_tool`, and `load_skill` tells the model the terminal is off when it is only hidden.
- src/lib/types.ts: `ToolDto.companion`, `ToolDto.shown`, interface `ToolSelection`.
- src/lib/ipc.ts: `getToolSelection`, `setToolSelection`.
- src/components/toolcards.tsx: one import and one entry, `call_tool`.
- e2e/scripted_agent.py: `p_tool_selection` and its entry in `PROBES`.

No new dependency. No TODO left in the code.

2. WHAT IT DOES NOW, AND THE CHOICES

For the user: the Tools panel still decides which tools are on. Each tool that is on carries a small mark, "shown" or "on request". At the top is a switch, "Choose tools for each task automatically", with one line of explanation; it is on by default. The header count is the number of tools that are on, companions not counted. The context estimate counts what a request carries. The companions appear in one line ("Always included with the tools above: …"), not as switches. New group labels: Artifacts, Web search.

For the model: a turn's list is decided once, when the turn starts (`tool_select::select`, a pure function):
- always, when on: the companions, `run_subagent`, `load_skill`;
- everything the chat was shown before (`tool_prefs.shown`), so the list never shrinks;
- then, while there is room under the cap (14): the five core file tools, what the chat used (from its history, newest first) or found, and the tools of the groups the last three user messages are about, best first, the matching groups taking turns.
The rest stays reachable with `find_tools` and `call_tool`.

Matching: a strong keyword counts 2, a weak one 1, a tool-name word that is not an everyday word 2; a group matches from 2. Skills match by enabled skill names and descriptions, external servers by their name, tool names and descriptions. Table test: 31 rows of messages and expected groups.

`find_tools`: searches shown and hidden tools, returns up to 8 (anything under half the best score is left out), each with purpose, arguments, examples (two for the best two matches, one for the others) and how to run it. Found tools wait in `tool_prefs.found` for the next turn.

Verbatim answer for `find_tools {"query": "commit"}` in the running app (files, terminal, git on; git hidden):

Tools that match "commit" (2):

git_commit [Git]: Stage the given paths, or all changes when none are given, and commit them.
  Arguments: message (string, required), paths (list of strings), repo (string)
  Example arguments: {"message": "Fix the date parser"}
  Example arguments: {"message": "Add tests for dates", "paths": ["tests/test_dates.py"]}
  Not in your tool list this turn. Run it with call_tool: name "git_commit", and its arguments in "arguments".

git_log [Git]: List recent commits.
  Arguments: limit (integer), path (string), repo (string)
  Example arguments: {"limit": 10}
  Example arguments: {"path": "README.md", "limit": 5}
  Not in your tool list this turn. Run it with call_tool: name "git_log", and its arguments in "arguments".

Size of the tool definitions (characters of the `tools` array in the request the fake endpoint received, divided by 3.5, the Tools panel's estimator; `outputs` and `task` were not registered yet):
- default tools of a new chat (files, artifacts, search, browser: 22 switches), message "hello": all sent, 22 tools, 11,328 characters, about 3,237 tokens; chosen, 7 tools, 3,785 characters, about 1,081 tokens.
- files, terminal and git: selection off, 14 tools, about 1,738 tokens; first turn with it on, 7 tools, about 1,081 tokens.
Not measured with a real tokenizer.

Choices beyond the spec's text. Overrule any of them:
1. A chat whose tools all fit under the cap is shown all of them.
2. List order is the order of `enabled_for_chat`, as the spec says. A later turn's list is the earlier one with tools added in their place in that fixed order (the earlier list is a subsequence of the later). It is not appended at the end. Changing to append order touches `select` and `remembered` only.
3. With the switch off, `find_tools` and `call_tool` are not offered (nothing is hidden). `read_output` and `update_task` still are.
4. `run_subagent` is always shown when on, because `call_tool` may not reach it. `load_skill` too, because the system message tells the model to call it by name. A loaded skill also brings `read_skill_file` and `run_command`.
5. The cap is hard except for what is always shown and what the chat was shown before. A found tool is promoted only while there is room.
6. `find_tools` names an off group only when the search is about it; when nothing is found it also lists the other off groups. It never lists their tools.
7. The answer never holds a whole call as one JSON object. The fallback parser refuses a call written as text when it repeats call-shaped text from a tool result; a test runs the real parser over the answer.
8. `call_tool` also runs a tool that is in the list (same gate).
9. A third command, `tool_catalog_problems`: checks the catalog against every built-in tool registered in the running app, because the unit tests cannot build servers that need the app.
10. Catalog words changed by the table test: "make" is not a terminal word, "fill" not a browser word, "report" and "document" count only as phrases ("a report"), "click" alone does not match the computer group.
11. `ToolsPanel`: marks are shown in both modes; with the switch off every tool that is on reads "shown".

3. TESTS

Rust, new: tool_catalog 7; tool_select selection 22; finder 14; whole turns 6; mcp_cmd 5 new.
Frontend, new: ToolsPanel.test.tsx 7; CalledTool.test.tsx 4.

Seen failing before the code existed (stubs):
- all 7 tool_catalog tests;
- 19 tool_select selection tests;
- all 14 finder tests;
- 6 of the 7 new ToolsPanel tests; CalledTool tests (module missing).
Seen failing against real code, then fixed: `what_a_chat_used_never_pushes_out_the_core_file_tools` (used tools were filling the places of the core file tools; order in `select` corrected).
Not seen failing first: `with_the_selection_off_every_tool_that_is_on_is_shown`, the 5 new mcp_cmd tests, the 6 whole-turn tests, the e2e scenario. For the whole-turn tests I ran four mutations of finder.rs, one at a time, and restored the file: card name not mapped (6 tests failed), an off tool looked up in the host (2), no refusal of `call_tool`/`find_tools`/`run_subagent` (2), audit row keeps the outer call (5).

What the safety tests show:
- `call_tool_is_described_exactly_as_the_tool_it_names`: for write, edit, delete, read, command, git status, a hard limit and a describer refusal, the described result equals the direct one, and `policy::evaluate` gives the same decision for every combination of recovered, untrusted, Allow all, with and without grants.
- `through_call_tool_the_gate_asks_and_refuses_as_for_a_direct_call`: write asks and is covered by a folder grant; recovered asks once-only; delete always asks, also in Allow all; outside folder denied; command asks after untrusted content.
- off tools, unknown names, recursion, target schema errors naming the field, loop-equal argument repair, close names only of tools that are on.
- whole turns (real loop, real tools, real engine, scripted model): the card and the audit row are the target's, a denied card runs nothing, a grant covers the write and not the delete, an off tool gets no card and no run, the tool list is identical in every request of a turn.
- `across_turns_the_list_only_grows_and_keeps_its_order`: the ordering and growth invariant over nine turns.

Existing tests changed in mcp_cmd.rs, each deliberate, nothing removed or weakened:
- `a_new_chat_starts_with_files_skills_and_the_browser_when_it_fits`: asserts added (artifacts and search on; companion servers not on by a default of their own).
- `defaults_apply_to_built_in_tools_only_and_settings_lists_show_nothing_on`: one assert added (an external server named "search" gets nothing).
- `dto()` helper: the two new fields.
- `DEFAULT_TOOLS` is now `defaultChatTools3`, pinned by the new test `the_last_choice_is_saved_under_a_new_name_since_the_defaults_changed`.
Existing ToolsPanel tests: the `tool()` helper got the two fields and the ipc mock the two commands. No assertion changed.

4. CHECKS RUN (final tree)

- `cargo fmt --all`, then `cargo fmt --all -- --check`: clean.
- `cargo +1.99 clippy -p app --all-targets -- -D warnings`: clean.
- `cargo +1.99 test -p app`: 232 passed, 0 failed.
- `npm run lint`: clean. `npm run typecheck`: clean. `npm test`: 22 files, 145 tests passed. `npm run build`: built.
- `scripts/check-spawn.sh`: passed.
- e2e `tool-selection`: PASS three times (alone, inside the whole suite, alone on the final code).
- Whole scripted suite, once: 19 PASS, 1 FAIL. The failure was `terminal-git` at "bad call": the script calls `git_push` after a message that is not about git, so the tool was on but not in the list, and the loop answered "No such tool: git_push; available: …" where the scenario expected "Invalid arguments". That run was on a build from before the last reorder in `select`; I did not re-run the other 19 after it.
- `python3 scripts/notices.py --check` says THIRD-PARTY-NOTICES.md is out of date. Not from my work.
Not run: clippy and tests of other crates (I changed none), Windows or macOS builds, any real model.

5. FOR ARCHITECTURE.md

Adaptive tool selection (0.0.4). The Tools panel decides which tools are on in a chat; that is the permission. Of those, each turn shows the model a small list: the companion tools, the core file tools, what the chat was shown before, what it used or found, and the tools of the groups the user's last three messages are about, 14 at most (setting `toolSelection`, `{auto, max}`). The choice is a pure function (`tool_select::select`) run once when the turn starts, so the list is byte-identical in every request of a turn. What was shown is remembered per chat (`tool_prefs.shown`), so across turns the list only grows, always in the fixed order of the chat's tools with `run_subagent` last. A chat whose tools all fit under the cap is shown all of them. With the setting off every tool that is on is shown and the two finder tools are not offered.

Two tools of the built-in server `toolfinder` keep the rest reachable. `find_tools {query}` searches the tools that are on, shown or not, and returns purpose, arguments and validated examples from the catalog (`tool_catalog.rs`; external tools use their own description and an example made from their schema). It reads the chat's tool list only (`Scope::AppData`, never asks) and may say that a group is switched off; it cannot switch it on. `call_tool {name, arguments}` runs a tool that is on but not in the list. The turn's tool host turns the call into the target before anything looks at it (`finder::Routing`): the name is looked up only in the two lists the turn started with, so a tool that is off is not found; the arguments are repaired and validated against the target's own schema; the target's own describer makes the action, with the user's switches for that tool; the policy engine decides on that action, so grants, always-ask, hard limits, the recovered-from-text rule, D9 and Allow all apply exactly as for a direct call; the approval card and the audit row take the tool's name from the action and show the real tool with its own arguments; the target's own code runs, followed by the same after-call hooks. An action that does not name the target is refused. `call_tool` cannot run itself, `find_tools` or `run_subagent`. A helper started by `run_subagent` gets the same shown and hidden lists without `run_subagent`.

Nothing the model does can switch a tool on: the selection only chooses among the tools that are on, and the only part of a chat's stored tool choices a turn can change is what it remembers as shown and found (`mcp_cmd::change_selection_memory`). The companion rule lives in `mcp_cmd::companions_follow`: `find_tools`, `call_tool`, `read_output` and `update_task` are on exactly when another tool is offered, and a saved choice never holds them. Changes to a chat's stored tool choices are serialised by one lock, so a turn remembering what it showed cannot undo a switch the user flips at the same moment.

6. NOT VERIFIED, AND WHAT TO RE-RUN

Re-run at integration:
1. `cargo +1.99 test -p app tool_catalog`.
2. The e2e scenario `tool-selection`. Its first assertion calls `tool_catalog_problems` in the running app and prints which built-in tool lacks an entry or which example its real schema rejects. When I ran it, `artifacts` and `search` were registered and passed against their real schemas. `outputs` (`read_output`) and `task` (`update_task`) were not registered; their entries are checked only against the Part 2 shapes in `entries_for_servers_built_elsewhere_fit_the_agreed_shapes`.
3. The scenario's last note prints tool counts and sizes for the default set; the numbers above will rise by the two missing companions.

Not verified:
- External MCP tools through `call_tool`. The app crate's tests cannot register an external server. That the user's "always ask" and "any arguments" switches are used for the target, and that a not-ready server is refused before a card, is in the code and read, not tested.
- The audit row of a `call_tool` call holds the inner arguments as the model wrote them; what ran is the repaired value (for example "5" became 5).
- The chat's tool card of such a call still says `call_tool`, with the target's name as its summary. The approval card and the audit row name the real tool, and the target's card extra is shown through the `call_tool` entry in toolcards.tsx.
- Windows and macOS: compiled on Linux only. No platform code was added.
- No real model was used. How well a 9B model uses `find_tools` and `call_tool` is unknown. Keywords are English only.
- The helper's shown and hidden split relies on the existing copy in `run_subagent`; the `subagent` e2e scenario passes, and I added no test of my own for it.

Things I noticed and did not change:
- src/components/ToolCallCard.tsx looks up `TOOL_CARD_EXTRAS[c.name]` with a name the model wrote. A call to a tool named `constructor` or `__proto__` resolves to a member of Object.prototype and would break the card. `Object.hasOwn` fixes it; my `calledToolExtra` already checks that.
- `find_tools` and `call_tool` together cost roughly 400 tokens by the same estimate. When a chat's tools all fit under the cap nothing is hidden and they are still offered. Leaving them out then would save that, at the price of the list changing the day something becomes hidden. I left it as the spec's companion rule has it.
- `rebuild_git` and `set_terminal_pass_through` re-register their server, which moves its tools to the end of the registry order and so changes the tool list order once. Older behaviour, not mine.
