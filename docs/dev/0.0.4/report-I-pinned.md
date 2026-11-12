Feature I part 1 (pinned instructions) is finished and the end-to-end scenario `pinned` passes. The earlier developer's claim held: `cargo +1.99 test -p agent` gave 151 unit and 47 turn_loop passing, and `-p store` gave 112 and 4. I kept their tested code and decisions 1 to 8. The only change is to their "known gap": a failed database read now stops the turn instead of sending it without pins (details in 2).

Most of my files were included in commit 0c6ab3d, which was made while I worked. Still uncommitted: `crates/agent/tests/turn_loop.rs`, `e2e/scripted_agent.py`, `src-tauri/capabilities/main.json`, `src-tauri/commands.list`, `src-tauri/src/lib.rs`, `src-tauri/src/turns.rs`.

**1. Files**
- New, mine:
  - `src-tauri/src/pinned/tests.rs`
  - `src/components/context/types.ts`, `api.ts`, `PinnedList.tsx`, `PinnedList.test.tsx`
  - `src/components/workbench/ContextTab.tsx` (placeholder replaced; it renders `<PinnedList>`)
- Changed, mine:
  - `src-tauri/src/pinned.rs` (the earlier developer's draft): compiles now. `for_turn(app, chat_id) -> AppResult<Vec<String>>` fails closed. `list_pins` also checks the window.
  - `crates/agent/tests/turn_loop.rs`: one new test appended.
  - `rustfmt` run on my files and theirs only (`crates/store/src/pinned.rs`, `crates/agent/src/prompt/tests.rs`, `turn_loop.rs`).
- Shared files, exactly what I added:
  - `src-tauri/src/lib.rs`: `mod pinned;` after `mod permissions;`, and six handler entries after `artifacts::artifact_canary,`: `pinned::list_pins`, `pinned::add_pin`, `pinned::edit_pin`, `pinned::remove_pin`, `pinned::move_pin`, `pinned::set_pin_scope`.
  - `src-tauri/commands.list`: the same six names appended.
  - `src-tauri/capabilities/main.json`: `allow-list-pins`, `allow-add-pin`, `allow-edit-pin`, `allow-remove-pin`, `allow-move-pin`, `allow-set-pin-scope`.
  - `src-tauri/src/turns.rs`, two marked lines and nothing else:
    - `input.pinned = crate::pinned::for_turn(app, chat_id).await?; // [0.0.4 pinned]`, just before `if subagents_on {`, so before the helper's copy.
    - In `failure_next_step`: `("pinned_do_not_fit", _) => crate::pinned::next_step(local), // [0.0.4 pinned]`.
  - `e2e/scripted_agent.py`: function `p_pinned` with `@probe("pinned")`, and `p_pinned` appended to `PROBES`.

**2. What the user sees, and the design**

In the panel beside the chat, the Context tab shows:
- The heading "Pinned instructions" with a counter ("2 of 20").
- One line: "Sent with every message, right after the system prompt. Stored as plain text: do not put passwords or keys in a pin."
- The list: pins for every chat first, then this chat's own. Each pin has:
  - a This chat / Every chat switch (buttons with `aria-pressed`);
  - Move up and Move down, disabled at the ends of the pin's own group;
  - Edit pin (edit in place with "Keep changes" and "Discard changes"; Escape discards, Ctrl+Enter keeps);
  - Unpin.
- An add box with "12 of 1,000 characters" while typing, a This chat / Every chat choice and a "Pin it" button.

The box counts code points. "Pin it" stays off when the text is empty or over 1,000 characters, and at 20 pins the box is disabled. The store's refusals (for example, another chat is full) appear as plain text, not as `role=status` or `role=alert`. Pin text is shown as text, never as markup.

The commands return the updated list. The event `pinned-changed` (payload: a chat id, or null for every-chat pins) makes the tab read the list again.

The prompt start with two pins, as the model receives it (exact text checked in the end-to-end run):
```
You are a helpful assistant.

<pinned_instructions>
The user pinned these instructions. Follow them in every reply:
1. Answer in German.
2. Use metric units in every answer.
</pinned_instructions>

<agent_instructions>...
```

When pins do not fit (20 pins of 1,000 characters on a 4,096-token model), the turn does not start and the message is:

"Your 20 pinned instructions need about 6,734 tokens; this model's window is 4,096, and pinned instructions may take up to 748 of it. Remove or shorten pins in the Context tab, or raise the context size."

The advice under it:
- Local model: "Remove or shorten pinned instructions in the Context tab, or load the model with a larger context."
- Remote endpoint: "Remove or shorten pinned instructions in the Context tab. If the server's model has a larger context, set it in Settings → Endpoints."

The known gap is closed: if the pins cannot be read, `for_turn` calls `computer_cmd::end_turn` itself and returns the error `pinned_unreadable` ("The app could not read this chat's pinned instructions, so nothing was sent."). The `?` in `turns.rs` then ends the turn before anything is sent, and the UI shows the error and clears the reply in progress. A `?` already exists at the same stage for approved folders, so this follows the existing pattern.

Subagents: a helper inherits the chat's pins. They are standing instructions from the user, like the system prompt, which helpers already get. It also keeps the cached prompt start shared between the chat and its helpers. `subagent::run` keeps `input.pinned`, and the `turns.rs` line sits before `let helper_input = input.clone();`. Both are tested.

No model tool was added, so there is nothing for `OWN_DATA_TOOLS`.

**3. Tests added**
- `turn_loop.rs`, `without_pins_the_tools_are_refused_or_accepted_as_before`: at 4,096 tokens with no pins, it sweeps tool lists across the 40% limit. Every turn is accepted or refused exactly as the rule from before pins decides, with the same "These 4 tools need about" message. It also checks that the default tools with no pins get through at 4,096. To show it bites, I briefly changed the runner to under-count the fixed part when there are no pins; the test failed, and I restored the file.
- `src-tauri/src/pinned/tests.rs`, 8 tests:
  - The commands are granted to the main window only (`main.json` has all six, overlay and indicator have none); the in-command window check; all six are in `commands.list`.
  - No tool can reach pins: nothing about pins in the tool catalog; a scan of all `.rs` files under `src-tauri/src` and `crates/` finds no call to the store's pin writers outside the two pin modules, and no other store file names the table.
  - The list's contents, order and JSON field names (camelCase).
  - Refusals reach the tab as the store's own sentence.
  - An unreadable store stops the turn.
  - `turns.rs` sets the pins with `?` before the helper's copy, and the `failure_next_step` arm exists.
  - A helper's request starts with the system prompt followed by the exact pinned block, and contains nothing of the chat's messages (run against a real scripted endpoint).
- `PinnedList.test.tsx`, 13 tests: listing, empty state, adding for this chat and for every chat, the character counter and limit, a refusal shown with the typed text kept, the 20-pin limit, edit keep and discard (buttons and keys), moving only within a group, switching scope, unpinning, refresh on the event, text never rendered as markup, and the Context tab mounting the list.
- I wrote these tests after the code, so none failed first. For the frontend I checked they bite: breaking the length check and the event filter made 3 tests fail, and I restored the file.

**4. Checks run**
- `cargo fmt --all --check`: my files are clean. It still reports `src-tauri/src/evals/sandbox/tests.rs` and `src-tauri/src/mentions.rs`, which belong to features F and H. An earlier run also listed several `crates/tools` files owned by B and H; I did not touch them.
- `cargo +1.99 clippy -p agent --all-targets -- -D warnings`: clean. Same for `-p store`: clean.
- `cargo +1.99 clippy -p app --all-targets -- -D warnings`: fails with 11 errors. All are in feature F's `src-tauri/src/evals/` (harness, profile, sandbox, verify, tasks) and in `outputs.rs` and `task.rs` (features B and G); none are in my files.
- `cargo +1.99 test -p agent`: 151 unit, 48 turn_loop and 30, 12, 1, 1, 1 in the other suites, all passing.
- `cargo +1.99 test -p store`: 112 and 4 passing.
- `cargo +1.99 test -p app`: 378 passed, 3 failed. All 3 are feature F's evals tests (`evals::harness`, `evals::sandbox`, `evals::bench`). `pinned::tests` passes 8 of 8.
  - In one earlier full run, `tool_select::finder::tests::call_tool_is_described_exactly_as_the_tool_it_names` also failed. It passed on its own, `tool_select` passed 46 of 46 on its own, and it passed in the next full run, so it looks flaky.
- `npm run lint`: clean. `npm run typecheck`: clean. `npm test`: 31 files, 204 tests passing.
- End-to-end, after `cargo +1.99 build -p app` (it only built on the 4th try, about 4 minutes in, while feature F was mid-edit): `flock /tmp/harness-e2e.lock env PROBES=pinned E2E_TRACE=1 E2E_OUT=<scratch> timeout 900 xvfb-run -a python3 -I e2e/scripted_agent.py` → PASS, 0 console lines. The scenario:
  1. Pins one instruction for this chat and one for every chat by typing in the Context tab.
  2. Checks the counter and the plain-text warning.
  3. Checks chat A's request carries the exact block, once, after the system prompt and before `<agent_instructions>`.
  4. Checks a second chat's request carries only the every-chat pin.
  5. After a window reload, checks both pins are still listed in chat A.
  6. Removes its pins at the end so later scenarios are not affected.
- Not run: `e2e/m1_chat.py` (the lead runs it) and `npm run build`. With no pins, the tool-size check is unchanged, which the new agent test covers.

**5. For ARCHITECTURE.md**

Pinned instructions are short texts the user keeps in front of the model, for one chat or for every chat, stored in `pinned_instructions` (a NULL `chat_id` means every chat; a chat's own pins are deleted with the chat). The store (`crates/store/src/pinned.rs`) enforces the limits whoever calls it: at most 20 pins apply to any chat (both kinds counted, so an every-chat pin is refused if any chat would go over), 1 to 1,000 characters each (a CHECK constraint backs this up), with control, bidirectional and invisible tag characters removed. Every change goes through a chat the pin applies to; another chat's pin is "not found". Pins are written only by the six Tauri commands in `src-tauri/src/pinned.rs`, which are granted to the main window and check the calling window again, and they take only text the user typed. There is no model tool for pins, and a source-scanning test keeps all other code away from the pin writers, so model output, tool results, files and web pages cannot become pins. A pin changes only the prompt: the app never interprets its text, and the store's pin calls write to no other table (tested), so a pin cannot change approval mode, grants, approved folders or tool switches (also tested in the agent loop). In `crates/agent`, `TurnInput::pinned` is rendered in the system message right after the chat's system prompt, as a `<pinned_instructions>` block whose tag is reserved (untrusted text cannot forge or close it) and whose text passes the prompt sanitiser. Because the block is part of the fixed part of every request, trimming and summarising never drop it. Pins have their own share of the context (25% of the budget), separate from the tools' 40%, and the tools' check leaves pins out so the tool list chosen for small models still fits. A turn whose pins exceed their share does not start and says how many tokens they take. A turn whose pins cannot be read does not start either. Without pins, the request is byte-for-byte what it was before. Helpers started by `run_subagent` inherit the chat's pins. Pins are stored as plain text, and the Context tab says not to put passwords or keys in one.

**6. Unverified and TODOs**
- Only built and run on Linux.
- Keyboard use of the tab is covered by unit tests, not tried by hand.
- The `pinned-changed` refresh between windows is unit-tested with a mocked event; the end-to-end run relies on the commands' return values.
- No `TODO(0.0.5)` markers left.
- Part 2 (the context inspector and the "Pin" action on a user message) is not started, as instructed. `PinnedList` keeps its state locally, so the inspector can sit beside it in `ContextTab` and refresh on `pinned-changed`.
