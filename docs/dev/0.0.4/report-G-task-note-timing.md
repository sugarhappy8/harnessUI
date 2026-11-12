The task note is no longer sent with every turn: it now goes only when one of your four conditions holds. The `get_task` window check is also done. `cargo +1.99 test -p app task::` passes 67 of 67 and the `task-state` scenario passes. Clippy on `app` could not run: the `agent` crate does not build under clippy because of another developer's work in progress, still the case after about six minutes of retries.

**What changed** (all in my files; `turns.rs` is untouched and the signatures of `begin_turn` and `note_for_turn` are the same)
- **New `src-tauri/src/task/when.rs`** holds the decision and the per-chat memory.
  - The decision is a pure function over the task's user marks, the chat's memory and the summary id. Its order is: user changed, paused, resumed or marked done → first real turn since the app started → summary changed → 8 turns since the last note → otherwise nothing.
  - The memory is kept in RAM only, per chat: the summary id at the last note, and the turns since it.
  - A turn with no task still records memory for the chat. So a task the model creates in that turn sends no note until one of the four reasons holds.
  - The TODO you asked for is where the note is produced.
- **The summary marker** is read the way `turns.rs::run` reads it: `latest_summary_on_path` over the chat's `current_path`, using the summary's row id.
  - One difference: `run` uses the path to `reply_to`, which I cannot see behind `begin_turn(app, chat_id)`. After a regenerate on another branch, the marker can differ for one turn; the worst case is one extra note.
- **`task.rs`:**
  - `begin_turn` and `note_for_turn` now use the same decision and the same renderer.
  - New read-only `pub async fn note_status_for_turn(state: &AppState, chat_id: i64) -> Option<String>`.
  - `clear_task` forgets the chat's memory.
  - I did not hook chat deletion, because that is in `chat.rs`, which I don't own. A leftover entry is harmless: at worst one first-turn note is skipped for a task the model wrote in that same conversation.
- **`get_task`** now calls `only_main_window(window.label())?` first, like the other three commands.
- I ran `cargo fmt -p app` once, which does the same as the allowed `cargo fmt --all` for that crate.

**Tests**
- **`when.rs` (6 tests):**
  - Each reason alone sends the note: first turn; each user mark alone (changed, paused, done); summary changed, appeared or gone; 8 turns passed.
  - With none of them, nothing is sent.
  - A due note is still nothing for a done task or one that holds only files and checks.
  - The count runs from the last note: notes go at turns 8 and 16.
  - Each status sentence, and that the memory can be forgotten.
- **`tests.rs`:**
  - The same-note test now covers 5 task states × 4 memory states. It checks that the inspector's note and the real turn's note match, that the status sentence agrees, and that looking changes neither the memory nor the record. Both "sent" and "not sent" cases are covered.
  - New `an_ordinary_turn_carries_no_note`: a task the model wrote sends nothing for three turns. A pause and resume sends the note once, and a new summary sends it once.
  - New `every_task_command_checks_the_window`: reads `task.rs` and asserts that each of the four `#[tauri::command]` functions starts with the window check.
- **`task-state` scenario:**
  - New assertion: the ordinary follow-up turn ("go on with it") carries no task note.
  - The Resume turn carries it exactly once, and it includes "paused this task and has now resumed it".

**Results**
| Check | Result |
|---|---|
| `cargo +1.99 test -p app task::` | 67 passed, 0 failed |
| `task-state` scenario | PASS, on a binary built after all the changes, with 0 console lines |
| `cargo fmt --all --check` | no diff in my files; diffs only in `crates/agent/tests/turn_loop.rs`, `crates/store/src/pinned.rs`, `src-tauri/src/pinned/tests.rs` and `turns.rs:1642`, all other developers' work |
| `cargo +1.99 clippy -p app --all-targets -- -D warnings` | not run: stops in the `agent` crate (`crates/agent/src/turn/runner/preview.rs:87`, "match can be simplified with `.unwrap_or_default()`"), another developer's file |
| `npm test` | not run; I did not touch the frontend |

**The exact sentences `note_status_for_turn` can return** (`None` when the chat has no task)
- "Sent with the next message: you changed, paused, resumed or finished the task since the model last saw it."
- "Sent with the next message: it is the first message in this chat since the app started."
- "Sent with the next message: earlier messages were summarised since the model last saw the task state."
- "Sent with the next message: it is sent again every 8 messages, and 8 have passed."
- "Not sent with the next message: the model already has the task state from this conversation. It is sent again after you change the task, after a restart, when earlier messages are summarised, and every 8 messages."
- "Not sent with the next message: the task is done."
- "Not sent with the next message: the task has no goal, steps or next action yet, only the files and checks the app noted."
