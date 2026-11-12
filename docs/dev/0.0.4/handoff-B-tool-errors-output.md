# Handoff: feature B (helpful tool errors, manageable tool output)

Stopped early at the owner's request. Two building blocks are finished and tested; nothing is
wired into a tool or the app yet. The tree builds and passes for the crates touched.

## (a) Files created or changed

| File | State |
|---|---|
| `crates/store/src/outputs.rs` (new, 500 lines) | Complete: the store module for `tool_outputs`, with 7 unit tests. |
| `crates/store/src/lib.rs` | Added `mod outputs;` and `pub use outputs::{MAX_LINE_CHARS, MAX_OUTPUT_BYTES, MAX_OUTPUTS_PER_CHAT, NewToolOutput, OutputLine, StoredToolOutput, ToolOutputInfo, output_lines};`. |
| `crates/tools/src/errors.rs` (new, 987 lines) | Complete as a library of pure helpers with 14 unit tests. Not called by any tool yet. |
| `crates/tools/src/lib.rs` | Added `pub mod errors;`. |

Not touched at all (still as the lead left them): `crates/tools/src/files.rs`, `terminal.rs`,
`git.rs`, `skills.rs`, `src-tauri/src/outputs.rs` (still the no-op `after_call`), the
`[0.0.4 outputs]` call line in `turns.rs`, `toolset.rs`, `src-tauri/src/lib.rs`,
`commands.list`, `capabilities/main.json`, `src/lib/ipc.ts`, `ToolCallCard.tsx`,
`ToolCallCard.test.tsx`, `e2e/scripted_agent.py`, `crates/store/src/schema.rs` (the
`tool_outputs` block is used exactly as it is).

### `crates/store/src/outputs.rs`
- `Store::add_tool_output(NewToolOutput { chat_id, call_id, tool, content }) -> ToolOutputInfo`:
  caps the text, inserts, and deletes the chat's outputs beyond the 200 newest, in one
  transaction. Unknown chat: `StoreError::NotFound`.
- `Store::get_tool_output(chat: ChatId, id: i64) -> Option<StoredToolOutput>`: the SQL is
  `WHERE id = ?1 AND chat_id = ?2`, so another chat's id reads as "not found". This is the
  per-chat rule; `read_output` and `get_tool_output` should both go through it.
- `Store::list_tool_outputs(chat) -> Vec<ToolOutputInfo>`, newest first, no text (for the
  "unknown id: these ids exist" error).
- `output_lines(text) -> Vec<OutputLine { text, continued }>`: the one definition of a
  stored output's lines. Like `str::lines`, plus a line over 2,000 characters is split
  into pieces of 2,000 (`continued: true` on a piece that is followed by more of the same
  line). The `lines` column is this count.
- Limits: `MAX_OUTPUT_BYTES` 2 MiB (head and tail kept, cut on line breaks when one is
  within 4 KiB, never inside a character; marker line
  `[... N bytes in the middle of this output were not kept: it was larger than 2 MB ...]`),
  `MAX_OUTPUTS_PER_CHAT` 200, `MAX_LINE_CHARS` 2000. Output ids are plain `i64`.

### `crates/tools/src/errors.rs`
Pure functions over text, all public:
- `quote(line, max)`, `quote_line(line)` (120 chars, `policy::sanitize::visible`, tabs kept).
- `number_list(&[usize], max)`: "4, 9 and 17", "… and 4 more".
- `match_lines(text, needle)`: 1-based line of each non-overlapping match.
- `Pick { Only, All, Nth(n) }`, `Pick::from_args(occurrence, replace_all)` (both together is
  an error with a fixed text), `plan_edit(current, old_text, new_text, pick) ->
  Result<EditPlan { updated, replaced, total, line }, EditProblem>` with
  `EditProblem::{NotFound, Ambiguous { lines }, OutOfRange { asked, lines }, NoChange}`.
  Meant to be called by BOTH `describe` and `call` of `edit_file`, so preview and write
  cannot disagree.
- `Known { Yes, Stale, No }`, `EditContext { path, current, old_text, known }`,
  `edit_problem_message(&problem, &ctx) -> String`: the exact error texts (tests pin them).
- `near_miss(text, old_text) -> Option<NearMiss>`: line endings (either direction),
  whitespace/indentation, letter case, or both, with the line number.
- `closest_lines(text, old_text, max)`: best matches for the longest line of `old_text`
  (shared letter pairs, at least half alike, at most 20,000 lines scanned).
- `edit_distance(a, b, limit)`, `similar_names(wanted, candidates, max)`: same name in
  another case, then same name with another extension, then a few characters away.
- `looks_like_error(line)`, `error_lines(&lines, first_number, max) -> (picked, count)`.

## (b) Status against section B of the spec

DONE and tested (library level only):
- Store module, 2 MB cap, 200 newest per chat, cross-chat refusal at the query.
- The text of the `edit_file` errors (found more than once, not found with whitespace /
  line-ending / case diagnosis and closest lines, occurrence out of range, both arguments,
  no change), the edit matcher for `occurrence` and `replace_all`, similar-name ranking,
  error-line picking.

PARTLY done:
- `edit_file` `occurrence` / `replace_all`: the matcher and messages exist; `files.rs` is
  unchanged. Left to do: add `occurrence` (integer, minimum 1) to the schema; in `describe`
  build `Pick::from_args(n(args, "occurrence"), b(args, "replace_all"))`, call `plan_edit`,
  return `Described::Invalid(edit_problem_message(..))` on a problem, and use
  `plan.updated` for the diff and the summary (`Edit X (match 2 of 3, at line 9)`); in
  `call` use `plan_edit` again on the bytes that were hashed.
- File-not-found: `similar_names` exists; the code that lists a folder and its parent
  (inside approved folders only) and names the working folder is not written.

NOT STARTED:
- Every change in `files.rs`, `terminal.rs`, `git.rs`, `skills.rs`.
- The shortened view for long command output (head, error lines, tail).
- `src-tauri/src/outputs.rs`: `after_call`, the `outputs` server with `read_output`, the
  `get_tool_output` command, and all registration (handler, `commands.list`,
  `capabilities/main.json`, `toolset.rs`, `ipc.ts`).
- The "Full output" button in `ToolCallCard.tsx` and its tests.
- The e2e scenario `tool-errors-output`.

## (c) Decisions to keep

1. **An edit error must not become a way to read a file.** `describe` runs BEFORE the
   policy gate, and `Described::Invalid(text)` reaches the model as an `[App note]`
   OUTSIDE the untrusted-data delimiter (see (d)). So `edit_problem_message` quotes lines
   of a file only with `Known::Yes`; with `Known::No` / `Stale` it gives line numbers and
   the `read_file` call that shows them (test `an_unread_file_is_never_quoted`). The plan
   was for `FilesServer` to remember, per chat, the SHA-256 of each file a `read_file` or
   `write_file` call succeeded on (in memory, `Mutex<HashMap<(chat_id, path), hash>>`):
   `Yes` when the current hash equals it, `Stale` when it differs, `No` otherwise. This is
   also what makes "remind it to read the file first if it has not" possible. If the lead
   prefers the spec's literal form (always quote), pass `Known::Yes` everywhere, but then
   a failing `edit_file` reveals up to 13 lines of any file in an approved folder with no
   approval; I would not ship that.
2. The existing test `edit_requires_a_unique_match` needs the ambiguous and not-found
   cases to stay `Described::Invalid` containing "3 times" and "not found". The new texts
   keep both phrases.
3. Reference line (spec): `[Shortened: N of M lines shown. Full output is saved as output #ID. Read more with read_output {"id": ID, "lines": "A-B"} or {"id": ID, "grep": "text"}.]`
   with plain digits. Planned: A-B is a real range (the first lines not shown), M is
   `ToolOutputInfo::lines`, and the card finds the id with a regex on the last line.
4. Line numbers everywhere are `store::output_lines` numbers. `crates/tools` cannot depend
   on `store`; if the shaping helper in `tools` numbers lines, give it the same 2,000
   character rule and add a test in the app crate that the two agree.
5. Error-word pattern in `errors.rs`: the spec's list with `fail` for `failed` and with
   `fatal` and `no such file` added (catches `FAIL` lines of Jest and Go, and git). Use
   `errors::looks_like_error` in both places the spec names so they stay the same.
6. `get_tool_output` should take the chat id (and may take the call id): the lead's brief
   says it must refuse another chat's output, which an id alone cannot do.
7. Planned and not built: `after_call` takes the result cap as an extra argument (the
   owner of the entry point may change the call line), stores, appends the line, and sets
   `out.full_text = None` so the live card shows what the model saw, ending with the line.

## (d) What I learned about the code

- **Where results are capped.** Tools size text to `ctx.chat.result_cap_tokens.max(500) * 3`
  BYTES. The agent loop then caps again in `crates/agent/src/turn/runner.rs` (~line 1590):
  `truncate_head_tail(&text, cap, cpt)` (`crates/agent/src/context.rs`), allowed =
  cap × chars-per-token CHARACTERS, where chars-per-token is 3.0 for llama-server and a
  learned 1.5 to 6.0 for remote endpoints. It keeps head and tail and inserts
  `[... N characters (about T tokens) omitted ...]` in the middle. Consequence: a tool
  that fills its budget and then appends a note already overflows by the note, and gets
  cut a second time in the middle. Leave room (I planned 400 bytes) for the tool's own
  note plus the reference line. The reference line survives a second cut because it is in
  the tail.
- **Where `full_text` is set today:** `list_dir` (files.rs ~724), `run_command`
  (terminal.rs ~518) and `sized_output` in git.rs (~1152) only. `read_file`,
  `search_files` and `read_skill_file` page with offset/limit and do NOT set it, so the
  e2e step "a read_file of a long file is shortened with the reference line" needs
  `read_file` to set `full_text` when the budget cut it. External MCP results never set
  it (`crates/mcp-host/src/result.rs`); the runner sets it itself when it cuts, which is
  AFTER `AppTools::execute` returns, so `after_call` never sees that case unless it does
  the shortening itself.
- **What the card shows.** `turns.rs` (~1271) sends `ToolFinished.text =
  full_text.unwrap_or(result text)`: live, a shortened call shows the FULL text; after
  the message is stored and reloaded the card shows what the model saw. Hence decision 7.
- **Call id.** `after_call` gets `self.shared.current_call()`, set by `AppSink` on
  `ToolCallStarted`. A helper's (subagent's) tool host has its own empty `TurnShared`, so
  its calls arrive with call id `""`. `run_subagent` itself returns before the hooks.
- **Hash re-check.** `describe` puts `expectedSha256` (hash of the bytes the diff was
  computed from, `null` for a new file) in the action payload. `call` first re-checks the
  path (`recheck`, sandbox step 5) and the skill-folder guard, then compares
  `current_hash(&path)` with the payload and refuses on any difference. `edit_file` then
  reads the file a SECOND time and applies the edit to that second read; hash the bytes
  you apply the edit to instead.
- **Trust position of errors.** `Described::Invalid(m)` and hard limits
  (`Decision::Denied`) become `ResultRecord.notes`, rendered by `render_result` in
  `crates/agent/src/prompt.rs` as `[App note] …` after the delimiter. `ToolOutput::error`
  from `call` is result text, inside the delimiter. Put anything quoted from files or
  commands in the second kind wherever possible.
- `policy::Action` has no `Default`; the Allow all agent is adding fields to it. A new
  server's `describe` (for `read_output`: `Scope::AppData`, `Risk::Normal`,
  `read_only: true`, `outbound: false`) must be checked against the struct as it is then.
- The terminal merges stdout and stderr into one buffer in arrival order; "last lines of
  stderr" needs a second small tail buffer in the reader tasks.
- git runs with `LC_ALL=C`, so its messages can be matched in English. The libgit2
  fallback's own texts are in `git_fallback.rs` (not ours to edit): "is not inside a git
  repository", "nothing to commit, working tree clean", "HEAD is detached (no branch is
  checked out)", "Not possible to fast-forward", "needs a sign-in. Add an access token".
- The store crate has `#![warn(missing_docs)]`: every public item needs a doc comment.
- `rustfmt` is not installed for the 1.99 toolchain; `cargo fmt --all` uses stable.

## (e) Checks run (last run after `cargo fmt --all`)

- `cargo fmt --all`: formatted; it printed one error for `crates/search`
  ("failed to resolve mod `extract`": another agent's file not written yet).
  `rustfmt --check --edition 2024 crates/tools/src/errors.rs crates/store/src/outputs.rs`: clean.
- `cargo +1.99 clippy -p store -p tools --all-targets -- -D warnings`: clean.
- `cargo +1.99 test -p store -p tools`: store 79 passed (7 are `outputs::tests`) plus 4 in
  `tests/store_api.rs`; tools 108 passed (14 are `errors::tests`); 0 failed.
- Seen failing first: 4 store tests against a stub (`another_chat_cannot_read_an_output`,
  `a_huge_output_keeps_its_head_and_tail`, `capping_never_splits_a_character`,
  `a_chat_keeps_its_newest_outputs`). The `errors.rs` tests were written with the code and
  passed on their first run.
- Not run: clippy and tests for `app` (nothing of mine is in it), the frontend checks (no
  frontend file touched), the e2e scenario (as instructed).
