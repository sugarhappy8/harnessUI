Feature B is finished and working end to end. The e2e scenario `tool-errors-output` passes, and the lead's design change is in place: no message from `describe` ever holds file text. One check is not clean: `cargo +1.99 clippy -p app` fails, but only in feature F's `evals` files, which F is still editing.

## What the model sees for an ambiguous edit, step by step
1. The model calls `edit_file {path, old_text, new_text}` and `old_text` appears twice in the file.
2. `describe` works out the edit with `errors::plan_edit` and gets "found more than once". Explaining that needs the file's lines, so `describe` does not refuse. It returns a read of that file with exactly `read_file`'s policy facts for that path (the same `read_action` code; `tool` is still "edit_file"). The model sees nothing at this step.
3. The gate treats it as the read it is:
   - In Ask mode with no grant, the user gets a read card: "Read /…/calc.py to explain why an edit cannot be made". Its preview says: "The edit to /…/calc.py cannot be applied as written. To explain why, the app will look up the lines of this file that match or come closest, and give them to the model. Nothing will be changed."
   - With a read grant for that folder, or in Allow all, there is no card.
   - If the user denies, the model gets only the usual "[App note] The user denied this call."
4. `call` gets the read-only action. It never writes: it reads the file again, works out the edit again, and returns an error result (`is_error: true`). The loop wraps it as untrusted data, with no app note. This is exactly what the e2e scenario recorded:
```
<tool_output id="…" tool="edit_file" status="error">
old_text was found 2 times in /…/calc.py, so it is not clear which one to change. Matches:
line 2:     total = 2 * qty
line 7:     total = 2 * qty
Send the call again with one of these: occurrence (1 to 2, counted from the top) to change one match; replace_all: true to change all 2; or more of the surrounding lines in old_text, so that it matches only once.
</tool_output id="…">
```
   If the file changed in between so that the edit would now apply, it still writes nothing. It says: "X changed after the edit was checked, and the edit would now apply as written. Nothing was changed: send the edit_file call again to make the change."
5. The model's second call adds `"occurrence": 2`. `describe` now gives a write action, "Edit X (match 2 of 2, at line 7)", with the diff, so the user gets an edit card that asks exactly as before. After approval, `call` writes. The result is "Edited X: replaced match 2 of 2, at line 7."

## 1. Files created and changed
- **`crates/tools/src/errors.rs`**:
  - New `needs_file_text(&EditProblem)`: true only for "not found" and "found more than once".
  - `edit_problem_message` now always quotes lines; it is only ever returned from `call`.
  - `Known` now only adds the "read the file first" reminder.
- **`crates/tools/src/files.rs`**:
  - `describe` turns those two problems into a read-only "explain" action.
  - New `explain_action` and `explain_edit`.
  - In `call`, any write tool under a read-only action refuses. A clippy fix in an existing test.
  - The comments on the per-chat read registry now say it is not a security boundary.
- **`crates/tools/src/skills.rs`**: messages for:
  - an unknown skill (lists the enabled ones);
  - a file not in the skill (similar names among its files);
  - a folder given instead of a file;
  - an offset past the end;
  - a path outside the skill's folder (names the folder);
  - a missing SKILL.md.
- **`crates/tools/src/git.rs`**: the `git_push` refusal (no Git installed, remote not usable) no longer quotes the push URL from the repository config. It was a describe-time message carrying text from a file. It now says what to do instead.
- **`src-tauri/src/outputs.rs`**, rewritten:
  - `after_call` and `record`: store the full text, append the Shortened line, clear `full_text`, and always take the tool's note out of `ui`.
  - The `OutputsServer` with `read_output`, and the command `get_tool_output(chatId, id)`, main window only.
  - The `after_call` signature is unchanged, so `turns.rs` was not touched.
- **`src-tauri/src/outputs/tests.rs`**: new, 12 tests.
- **Shared files, one additive edit each**:
  - `toolset.rs`: `host.add_builtin(Arc::new(crate::outputs::OutputsServer::new(app.clone())));` plus a comment line.
  - `lib.rs`: `outputs::get_tool_output,` in `generate_handler!`.
  - `commands.list`: `get_tool_output`.
  - `capabilities/main.json`: `"allow-get-tool-output"`.
  - `src/lib/ipc.ts`: the `SavedToolOutput` interface and `getToolOutput(chatId, id)`.
  - `e2e/scripted_agent.py`: `p_tool_errors_output` and one `PROBES` entry.
- **`src/components/ToolCallCard.tsx`**: `savedOutputId()` and a "Full output" strip that opens the saved text in the card (scrollable, "Copy output", "Hide"). It takes the chat from `useApp().activeChatId`, as `ArtifactCardExtra` does.
- **`src/components/ToolCallCard.test.tsx`**: 4 new tests.
- **`src-tauri/src/tool_select/finder/tests.rs`** (tool selection's file):
  - One case had a failing edit refused by `describe`; that edit is now a read, so it reached the engine (8 actions instead of 7).
  - I kept the "describer refuses" case by using `occurrence: 9`, which is still refused.
  - I added the read case and set the count to 8.
- Not touched: `crates/store`, `turns.rs`, `tool_catalog.rs`, `mcp_cmd.rs`. Their name and server (`read_output`, `outputs`) match what I register.

## 2. What it does now, and the choices
- **Edit messages before the gate.** Messages that need no file text stay refusals from `describe`: bad arguments, `occurrence` with `replace_all`, `occurrence` out of range (numbers only), no change, path refused, folder or file mix-ups, missing file (no names), too large, binary, changed since read. A test checks that none holds a line of the file.
- **The read registry** stays only for the "read the file first" reminder and for refusing a `write_file` to a file that changed since this chat read it.
- **Similar file names:** I kept the second developer's rule. Names appear only in results of `read_file`, `list_dir` and `search_files`, which the gate let through as reads (`Scope::FileRead`, the same policy as `list_dir`), each folder checked against the sandbox. A failing edit or delete never names a file.
- **Shortened line**, exactly:
  `[Shortened: N of M lines shown. Full output is saved as output #ID. Read more with read_output {"id": ID, "lines": "A-B"} or {"id": ID, "grep": "text"}.]`
  - N comes from the tool's note, or else from counting the shown lines that match the start of the full text. M is the stored line count.
  - A-B is the first range not shown (at most 200 lines).
  - `full_text` is then cleared, so the live card and the reloaded card both show what the model saw, with the button.
- **`read_output` definition**, exactly:
  - description: "Read a shortened tool result saved as output #id: a line range, or the lines containing grep text."
  - schema: `{"type":"object","properties":{"id":{"type":"integer"},"lines":{"type":"string","description":"A-B"},"grep":{"type":"string"},"context":{"type":"integer","minimum":0}},"required":["id"],"additionalProperties":false}`
  - Under 330 characters together (a test checks it).
- **`read_output` behaviour:**
  - Policy: `Scope::AppData`, `Risk::Normal`, read-only, `outbound: false`, `network: false`.
  - Lines come back numbered as "line: text".
  - When more is left, the result names the next range to ask for.
  - With grep: matching lines marked ":" and context lines "-", default context 2, at most 20. When cut, it says where to read on.
  - Errors: an unknown id lists the chat's ids; a bad range says what to send; past the end says how many lines there are.
  - "No matches" is a plain result that gives the line count (the second developer's decision).
  - Its result carries outside content (another tool's output), not the app's own data. That fits it not being in `OWN_DATA_TOOLS`.
- **Promises to other features:** `is_error` and the `Exit code N (…)` first line of `run_command` are unchanged. Feature G: an explain-only `edit_file` returns `is_error: true` and `action.read_only`, so the task hook must not count it as a changed file.

## 3. Tests
- **New in `files.rs`:**
  - `an_edit_that_needs_the_file_to_explain_it_is_described_as_a_read`: ambiguous and missing text, including `.git/config`; policy facts equal `read_file`'s.
  - `no_message_from_describe_holds_a_line_of_the_file`: a marker line, 17 calls across `edit_file`, `write_file`, `delete_file` and `read_file`.
  - `the_read_only_edit_never_writes`: the file is byte-for-byte unchanged, also when changed in between so the edit would apply.
  - `text_that_is_not_found_is_explained_with_the_closest_lines`: exact texts as `call` results — closest lines, indentation, case, line endings, nothing alike.
- **Changed in `files.rs` because of the design change:**
  - `edit_requires_a_unique_match` now asserts on the `call` result and that the file is unchanged.
  - The ambiguous-edit test now runs without a prior read.
  - `an_edit_error_never_quotes_a_file_that_was_not_read` was replaced by the four tests above.
- **`errors.rs`:** `only_not_found_and_ambiguous_need_the_file_text` and `a_file_not_read_yet_gets_a_reminder_to_read_it` replace `an_unread_file_is_never_quoted`. One near-miss assertion is now an exact string.
- **`skills.rs`:** `skill_failures_say_what_to_do_next`.
- **`outputs` (12 tests):** exact Shortened line, two chats, tools and store count lines the same, ranges, grep, errors, AppData never asks, definition size.
- **Card:** 4 new tests.
- **Seen failing first:**
  - The two "Full output" card tests, with the button switched off temporarily.
  - The finder test (8 instead of 7), from the design change itself.
  - Two skills assertions and one files assertion failed on my own wrong expected strings; the code was right.
  - The outputs, errors and remaining files tests were written with the code and passed on their first run.
- **Old terminal test:** the three properties of the old `head_and_tail` (head kept, tail kept with a marker, short text left alone) are all still asserted. I added nothing.

## 4. Checks run (final round)
- `cargo fmt --all --check`: clean.
- `cargo +1.99 clippy -p tools --all-targets -- -D warnings`: clean.
- `cargo +1.99 clippy -p app --all-targets -- -D warnings`: fails, with 5 errors, all in feature F's files (`evals/harness.rs`, `tasks.rs`, `sandbox.rs`, `verify.rs`). None are in my files.
- `cargo +1.99 test -p tools`: 188 passed, 1 ignored; `allow_all.rs` 3 passed.
- `cargo +1.99 test -p policy`: 78 passed.
- `cargo +1.99 test -p app`: 387 passed, 0 failed. Earlier, three `evals` tests of F's were failing while F worked; they pass now.
- `npm run lint` and `npm run typecheck`: clean. `npm test`: 34 files, 234 tests passed.
- e2e `tool-errors-output`: PASS. It covers:
  - the read card for the ambiguous edit, then the match list as data, then the edit card for `occurrence: 2`, then the file changed;
  - a 6,000-line `read_file` shortened with the line (`723 of 6000`, output #1);
  - `get_tool_output` refusing another chat;
  - `read_output` with grep and no card;
  - "Full output" opening the text in the card.
- Not run: the full e2e suite.

## 5. For ARCHITECTURE.md
Tool failures explain themselves without opening a way around the gate. A message from `describe` reaches the model as an app note, which the model trusts, before any approval. So it never contains text from a file, names from a folder, or text from repository config; it only gives arguments, numbers, paths the model sent, and what to call next. When explaining a failure needs a file's text (an `edit_file` whose text is missing or found more than once), the call is described as a read of that file, with `read_file`'s own policy facts for that path. The gate asks or allows exactly as for that read. `call` answers with the matching or closest lines (cut to 120 characters, made visible) as an error result, which the loop wraps as untrusted data. Under that read-only action no file tool writes, even if the edit has become possible. Long results are cut to the chat's result budget by the tools: head, the error-looking lines with numbers, and tail for commands. The whole text goes to `tool_outputs` for that chat (2 MB cap, keeping head and tail; the 200 newest per chat). The model is told the output id and how to read more. `read_output` reads only the current chat's outputs (the chat is part of every query). It is `Scope::AppData`, read-only and offline, so it never prompts, and it is audited. Its result is outside content, delivered like any tool result. The tool card opens the same saved text through `get_tool_output(chatId, id)`, main window only.

## 6. Unverified and left to do
- Only built and run on Linux; not compiled for Windows or macOS.
- No `TODO(0.0.5)` added.
- Not done (both optional in the handover notes): de-duplicating identical saved outputs, and lowering the 2,000-character line rule to 1,000.
- The lead should re-run `cargo +1.99 clippy -p app` once F's `evals` files are finished.
