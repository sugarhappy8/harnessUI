# Handoff: feature G, "Visible, resumable task state" (harnessUI 0.0.4)

Stopped early on the owner's change of plan. Only the storage layer is built. Everything
else in section G is designed (below) but not written. The tree is as buildable as I found
it: I changed nothing in the app crate or the frontend.

## (a) Files created or changed

| File | State |
|---|---|
| `crates/store/src/tasks.rs` (new, mine) | Complete and tested. Types, persistence, the rules for recording changed files and checks, 10 unit tests. In commit `1b8a5d3` ("0.0.4 work in progress"); since then `cargo fmt` joined one `assert!` in a test, so `git status` shows it modified (formatting only, 1 line). |
| `crates/store/src/lib.rs` (shared) | Added `mod tasks;` and `pub use tasks::{ChangedFile, FileChange, MAX_CHANGED_FILES, MAX_CHECKS, StepStatus, Task, TaskCheck, TaskNote, TaskState, TaskStatus, TaskStep, UserMarks};`. Committed in `1b8a5d3`. |

Nothing else. In particular, untouched by me:
- `src-tauri/src/task.rs`: still the original no-op `after_call` (0 diff lines).
- `src-tauri/src/turns.rs`: no `[0.0.4 task]` line added in `run` yet. The `after_call`
  line is unchanged (now at line 869-870; `input.summary = …` is at line 1686).
- `src-tauri/src/toolset.rs`, `src-tauri/src/lib.rs`, `src-tauri/commands.list`,
  `src-tauri/capabilities/main.json`, `src/**`, `e2e/**`: nothing of mine.
- `crates/store/src/schema.rs`: the `[task]` block is as the lead wrote it; it fits.
- I had created an empty folder `src-tauri/src/task/` and removed it again.

## (b) Section G, item by item

DONE and tested
- **Data / store module.** One record per chat in `tasks`; `get_task`, `change_task`
  (atomic read-change-write), `delete_task`; `TaskState::record_file` and
  `TaskState::record_check` (created vs edited vs deleted, one entry per file or command,
  capped). Tests: round trip through a database file closed and reopened; create, alter,
  delete, and no write when nothing changed; 40 changes at once all kept; per-chat
  isolation and cascade on chat delete; a missing chat is `NotFound`; older and newer
  state shapes load, a wrong shape is `StoreError::Json` and can still be deleted; the
  changed-file table (17 rows); listed once, newest last; cap of 200 with a count of the
  rest; a check run again keeps its latest result, cap of 30; statuses as plain words.
  Seen failing first: 9 of the 10 against stubbed bodies (all but the plain-words test).

NOT STARTED (no code exists; the design is in (c))
- Server `task` with `update_task`, and its line in `toolset.rs::build_host`.
- `task::after_call`: recognising check commands, reading the outcome, recording facts.
  (The recording rules themselves are done in the store; the recogniser, the outcome
  parser and the glue are not.)
- The task note at the start of a turn, and the `[0.0.4 task]` line in `turns.rs::run`.
- Tauri commands, their three registrations (`generate_handler!`, `commands.list`,
  `capabilities/main.json`), the `task-changed` event.
- `src/components/workbench/TaskTab.tsx` (still the lead's placeholder), files under
  `src/components/task/`, `ipc.ts` / `types.ts` entries, frontend tests.
- The e2e scenario `task-state` and its `PROBES` entry.

PARTLY done: nothing. There is no half-written code anywhere.

## (c) Design decisions

### Implemented: keep these (code and tests depend on them)

**JSON `state` column** (`store::TaskState`, snake_case, every field `#[serde(default)]`,
unknown fields ignored):

```json
{
  "goal": "string",
  "constraints": ["string"],
  "steps": [{"text": "string", "status": "todo|doing|done"}],
  "changed_files": [{"path": "full path", "kind": "created|edited|deleted", "at": 0}],
  "files_not_listed": 0,
  "checks": [{"command": "string", "passed": true, "at": 0, "summary": "string", "exit_code": 0}],
  "next_action": "string",
  "notes": [{"text": "string", "at": 0}],
  "user": {"goal": false, "next_action": false, "constraints": ["text"], "steps": ["text"],
           "changed": ["goal", "constraints", "steps", "next action"], "paused": false}
}
```

- `status` (`active|paused|done`, `store::TaskStatus`) and `updated_at` are table columns,
  not in the JSON. `store::Task { chat_id, status, updated_at, state }`.
- `updated_at` is set by the caller, never by the store (so tests are deterministic and
  bookkeeping changes need not move "last updated").
- `user` (`UserMarks`) is how "what the user wrote" is kept apart from what the model
  wrote: `goal` / `next_action` flags, and for constraints and steps the texts the user
  wrote. Authorship is by text, so when the model re-sends a list a user-written item
  stays the user's exactly as long as its text is unchanged (prune the lists to the texts
  still present). `changed` and `paused` mean "since the model was last sent the state".
  Only user commands may write `user`.
- `checks[].summary` comes from command output. It is for the tab only: never put it in
  the note (see "Security" below). `exit_code` is the app's own fact and may go in.

**Store API** (`impl Store`, all async):
- `get_task(chat_id) -> Option<Task>`
- `change_task(chat_id, |slot: &mut Option<Task>| -> R) -> (Option<Task>, R)`: runs in a
  transaction on the database thread. Set the slot to create, alter it in place, set
  `None` to delete. Writes only if the task differs afterwards. Always stored under the
  `chat_id` argument. Use it for every write (tool call, `after_call`, user edit, turn
  start) so concurrent writers never lose each other's change.
- `delete_task(chat_id) -> bool`: does not parse the state, so it works on a broken one.

**Recording rules** (`TaskState`):
- `record_file(path, kind, at)`: one entry per exact path, moved to the end. The entry says
  how the file differs from when the task started: created then edited = created; created
  then deleted = not listed; edited or deleted then written again = edited; edited then
  deleted = deleted. Cap `MAX_CHANGED_FILES` = 200, oldest dropped and counted in
  `files_not_listed`.
- `record_check(check)`: one entry per command text, latest result, moved to the end. Cap
  `MAX_CHECKS` = 30.

### Designed, not written: my recommendation, free to change

**Commands** (four): `get_task(chat_id) -> Option<TaskDto>`,
`edit_task(chat_id, edit: {goal?, constraints?, steps?, nextAction?}) -> TaskDto` (the
user's correction; may create the record), `set_task_status(chat_id, status) -> TaskDto`
(`paused`, `active`, `done`: one command for Pause, Resume/Reopen, Mark done),
`clear_task(chat_id)`. Emit `task-changed` (payload: chat id) to `crate::MAIN_WINDOW` on
every stored change so the tab refreshes. DTO in `task.rs`, camelCase, with `byUser`
flags derived from `user`.

**Module layout**: `src-tauri/src/task.rs` plus `task/checks.rs` (recogniser, outcome),
`task/note.rs` (rendering), `task/edits.rs` (cleaning, the model's update, the user's
edit). Write the logic as functions over `&store::Store` or plain values so it tests with
`Store::open_in_memory()` and no `AppHandle`; keep the Tauri parts thin.

**`update_task`**: schema as in spec part 2, `additionalProperties: false`, only `text`
required in a step. `Scope::AppData`, `Risk::Normal`, `outbound: false`. The chat comes
from `ctx.chat.chat_id`, never from arguments. Rules I settled on:
- Text is cleaned on the way in: whitespace runs to one space, `policy::sanitize::visible`,
  capped (goal 400, a step or constraint 200, next action 300, note 400 characters;
  30 steps, 12 constraints, 20 notes). Over-long input is shortened and the result says
  so, rather than refused.
- The model cannot set changed files, checks, `user`, or `paused`.
- While the task is paused the model's `status` is ignored (the result says why).
- A done task that gets a different goal starts fresh: steps, next action, notes, changed
  files and checks cleared, the user's constraints kept, status active. Any other
  substantive change to a done task makes it active unless the call says `done`.
- Refuse the call while a helper is at work (`crate::subagent::running_in(chat).is_some()`):
  a helper shares the chat's tools and would overwrite the main task.
- Result: "Task saved." plus the same compact lines as the note, without the header.

**`after_call`** (signature can stay as it is):
- Use `action.server` / `action.tool` / `action.payload`, not `name` / `args`: with tool
  selection a call may arrive wrapped in `call_tool`, and the action is the target's.
- Files: `action.server == "files"`, tool `write_file|edit_file|delete_file`,
  `!out.is_error`. Path = `payload.canonical`. `write_file` is "created" when
  `payload.expectedSha256` is null (fallback: `payload.exists == false`).
- Create the record on the first fact even if the model never called `update_task`, so
  the tab is useful with small models. Send no note for such a record (see below).
- Checks: command = `payload.command`. Recognise it, then read the outcome: exit code
  from "Exit code N" near the start of the text; `passed = code == 0 && !is_error`; no
  code and `is_error` means it did not finish (stopped, could not start): record nothing,
  except a timeout, which is a failed check.
- Recogniser (table-driven test with bash and PowerShell rows): split on `;` and newlines
  and judge only the LAST statement, because that is what the exit status reports; `||`
  hides the status (`npm test || true`): not a check; a check piped into something
  (`| tail`, `| tee`) is not recorded, the status is the last stage's; trailing `&`: not
  a check; in an `&&` chain any plain check counts. Match by program position only (first
  word after `VAR=x` assignments and wrappers such as `time`, `npx`, `pnpm exec`,
  `uv run`, `python -m`, `bash -c "…"`, PowerShell `&`), never by words anywhere, so
  `echo "npm test"` and `git commit -m "fix tests"` miss. Strip `.exe/.cmd/.bat`.
  `--help`, `--version`, `--watch` are not checks. The bare word `test` is the shell
  builtin. Script and target names (`npm run test:unit`, `make lint`, `./build.sh`,
  `gradlew assembleDebug`) are judged by their words, split on punctuation and camelCase.

**Task note** (one string in `input.app_notes`, multi-line; the runner prefixes
`[App note] `). Proposed text:

```
Task state (the app's record of the work so far, not a request). Text in quotes is what you recorded earlier, unless it says the user wrote it.
status: active
since you last saw this: the user paused the task and changed: goal, next action
goal (the user wrote it): "Put the switch in the header"
constraints: "No new dependencies"
steps: 1 [done] "Read the settings page"; 2 [doing] "Add the switch"; 3 [todo] "Run the tests"
changed files (in "/work/src"): created "Switch.tsx"; edited "Settings.tsx"
last checks: passed "npm test"; failed "npm run lint" (exit code 1)
next action (the user wrote it): "Move the switch into the header"
```

- This sample is the design, not output from code: no renderer exists yet.
- No note when there is no task, when the task is done, or when the record holds only
  app-tracked facts (no goal, steps, constraints or next action).
- Cap: 300 tokens by `agent::estimate_tokens` (3 characters per token), so 900
  characters; count a non-ASCII character as a whole token so CJK text stays bounded.
  Budget in priority order: goal, next action, steps (from the first unfinished one, with
  "12, 5 done" and "+N more"), files (newest first, up to 20, relative to their common
  folder), last 4 checks, constraints. Test with a very large task.
- Quote every model-written or path text: one line (`policy::sanitize::one_line`), `"`
  replaced by `'`, the `[App note]` marker defanged. Model text can then never start a
  line or close its quotes.
- Turn start (the one call from `turns.rs::run`): build the note, then make a paused task
  active, then clear `user.changed` and `user.paused`. So "paused" means on hold with
  nothing running. Offer a read-only variant for the Context inspector (feature I).

**Tab**: header with a status chip (Active / Paused / Done) and "Pause", "Resume",
"Mark done", "Clear task" (Radix AlertDialog to confirm). Pause =
`set_task_status(paused)` then, only if `generation.chatId === chatId`,
`useApp.getState().stop()`. Resume = `set_task_status(active)` then
`useApp.getState().send("Continue the task. Start with the next action.")`; disable it
while any generation runs. Say in the tab that only changes made with the file tools are
listed (commands that change files are not tracked).

### Security points to keep
- The note carries only app facts, the model's own text (quoted) and the user's text
  (marked). Never text from tool output: a new turn starts as "trusted" (D9), so a line of
  test output in the note would be untrusted content with the app's voice.
- Only the user's commands set `user` marks, `paused`, and changed files/checks come only
  from `after_call`.

## (d) Things that save time

- **How app notes reach the model.** `agent::turn::TurnInput.app_notes: Vec<String>`.
  `crates/agent/src/turn/runner.rs::view()` appends them to the turn's own user message as
  one extra text part, each as `[App note] ` + `sanitizer.clean(note.trim())`, joined by
  newlines. Nothing in `turns.rs::run` sets `app_notes` today. A helper never gets them
  (`subagent::run` clears them, and the helper's input is cloned before the summary line).
- **Cost of that mechanism.** The note is not stored, so in the next turn the earlier
  user message is sent without it and the server's cached prompt is valid only up to that
  message: the whole previous turn is read again. This is why I would send no note for
  done or facts-only records. A real fix needs the lead (`crates/agent`).
- **D9 side effect.** The loop sets `untrusted = true` after every tool execution
  (`runner.rs`, right after `execute`), `update_task` included. In Ask mode a granted
  outbound call (`run_command`) therefore asks again after an `update_task`. Not fixable
  from this feature's files; tell the lead.
- **Stop path.** Frontend `useApp.getState().stop()` -> `ipc.stopGeneration()` ->
  `chat::stop_generation` (cancels the generation, releases computer use, cancels cards).
  One generation runs app-wide; `state.generation.running_in(chat)` on the Rust side,
  `generation.chatId` in the store.
- **Send path.** `useApp.getState().send(text)` uses the active chat and the last message
  as parent, and puts failures in the store's `error` banner.
- **Store patterns.** `self.call(move |conn| …)` runs on the one database thread;
  `conn.transaction()` works there; `ensure_chat`; `sql_enum!` gives serde and SQL
  conversions. Tests: `Store::open_in_memory()`, or `Store::open(path)` with `tempfile`.
- **Tool results.** Terminal: text starts `Exit code N (x s)`, `is_error` on any failure,
  `ui: {command, cwd}`. `outputs::after_call` runs before `task::after_call` and may add a
  `[Shortened: …]` line to the text; skip bracketed marker lines and prefer `full_text`.
  Agent B is rewording terminal and file results; agent A adds fields to `policy::Action`,
  so a struct literal for the action will need those fields.
- **Built-in server pattern.** `src-tauri/src/subagent.rs::SubagentsServer`. Take an
  `AppHandle`, use `app.try_state::<AppState>()` in `call` (the server is registered
  before the state exists). `subagent::defang_app_notes` is private: copy the rule.
- **App crate tests.** No dev-dependencies (no `tempfile`): use `std::env::temp_dir()`.
  A test with the real `tools::FilesServer` / `TerminalServer` would catch payload drift.
- **e2e.** `click_button(label)` takes the first visible button containing the label:
  avoid "Save", "Continue", "Stop", "Use", "Send" in new button labels. Open the panel
  with the header button (`aria-label` "Panel"), then the "Task" tab. After
  `window.location.reload()` the chat must be opened again and the panel is closed.
  Pass tools explicitly to `new_chat`, adding `update_task` only if it is in
  `c.all_tools` (tool selection may make it automatic). Make the reply slow
  (`"delay": 0.05, "chunk": 8`) so Pause has something to stop.
- **Tooling.** `rustfmt` is not installed for toolchain 1.99: format with plain
  `cargo fmt --all`. Right now that command fails on another agent's crate
  (`crates/search` names `mod extract;` with no file yet); `cargo fmt -p store` works.

## (e) Checks run (all from `/home/user/harnissUI`, `CARGO_INCREMENTAL=0`)

| Command | Result |
|---|---|
| `cargo +1.99 test -p store tasks` against stubs | 9 failed, 1 passed (the expected red run) |
| `cargo +1.99 test -p store tasks` after implementing | 10 passed, 0 failed |
| `cargo fmt --all` | exit 1: `failed to resolve mod extract: crates/search/src/extract.rs does not exist` (not my file). No file newly showed as modified in `git status` afterwards; I did not check whether it reformatted files other agents had already changed. |
| `cargo fmt -p store -- --check` | exit 0 |
| `rustfmt --edition 2024 --check crates/store/src/tasks.rs` | formatted |
| `cargo +1.99 clippy -p store --all-targets -- -D warnings` | exit 0, no warnings |
| `cargo +1.99 test -p store` | 79 passed (unit, mine are 10 of them), 4 passed (`tests/store_api.rs`), 0 failed |

Not run: anything for the app crate (I changed nothing in it), `npm run typecheck` /
`lint` / `test` (no frontend changes), the e2e scenario (not written, and not to be run).
