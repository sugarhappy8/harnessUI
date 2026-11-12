Feature I part 2 is finished. The context inspector, the "Pin" action on a user message and the end-to-end scenario `context-inspector` all work, and a real turn sends exactly what it sent before. Five other scenarios still pass after the refactor: `pinned`, `task-state`, `mentions`, `tool-errors-output` and `step-limit`. `subagent` fails, and it looks like a timing race in the test rather than my change (point 6). `context-inspector` checks:
- The request the fake endpoint received matches what the inspector showed just before: the same system message, the same tool definitions in the same order, every earlier message and the task note.
- Two inspections leave the database exactly as it was.

Your messages say the lead commits: my changes are already in the tree's last commit, and `git status` now shows only other developers' files.

**1. Files**
- New, mine:
  - `src-tauri/src/context_cmd.rs` (the `inspect_context` command) and `context_cmd/tests.rs`
  - `crates/agent/src/turn/runner/preview.rs`
  - `src/components/context/ContextInspector.tsx`, `ContextInspector.test.tsx`, `PinMessageButton.tsx`, `PinMessageButton.test.tsx`
- Changed, mine:
  - `crates/agent/src/turn/mod.rs`: new types `TurnPreview`, `SystemBlock`, `SystemBlockKind`, `NextCompaction`, and `preview_turn`.
  - `crates/agent/src/lib.rs`: one re-export line.
  - `crates/agent/src/turn/runner.rs`: `mod preview;`, and two helpers lifted out of the summary stage unchanged (`calibrated`, `summarisable`).
  - `crates/agent/tests/turn_loop.rs`: 4 new tests.
  - `crates/store/src/pinned.rs`: `Store::pin_message` and 2 tests.
  - `src-tauri/src/pinned.rs`: the `pin_message` command, and `for_turn(app, chat_id, turn)`.
  - `src-tauri/src/pinned/tests.rs`: updated for the new line and new command.
  - `src/components/context/api.ts` and `types.ts`.
  - `src/components/workbench/ContextTab.tsx`: now shows the pinned list, then the inspector.
  - The `src/components/context/PinnedList.test.tsx` mock.
- Changed, other owners' files (allowed by your message):
  - `src-tauri/src/turns.rs`: the refactor (point 2).
  - `src-tauri/src/tool_select.rs`: `for_turn` and the new `preview_for_turn` share one `choose(…, remember)`. Only the real turn records what was shown.
  - `src-tauri/src/chat.rs`:
    - `resolve_target` now calls `target_for(state, model_ref, remember)`. With `remember` false, a remote endpoint's reported context size is not stored.
    - `turn_settings(state, &target)` was taken out of `generate`, unchanged.
  - `src/components/MessageItem.tsx`: one import and one marked line `{!editing && m.text && <PinMessageButton messageId={m.id} />}`.
- Shared registration files:
  - `lib.rs`: `mod context_cmd;`, plus handler entries `pinned::pin_message` and `context_cmd::inspect_context`.
  - `commands.list`: `pin_message`, `inspect_context`.
  - `capabilities/main.json`: `allow-pin-message`, `allow-inspect-context`.
  - `src/lib/ipc.ts`: `setChatTools` now fires a window event `chat-tools-changed` after it succeeds, marked `[0.0.4 context]`. The backend has no event for a chat's tool switches, so this is how the tab knows to refresh.
  - `e2e/scripted_agent.py`: `p_context_inspector`, and its entry in `PROBES`.

**2. What the user sees, and the design**

Under the pinned list, the Context tab has a section "What the model gets next":
- A sentence about the model and its context size. Without a usable model it says why, and no numbers are shown.
- A bar with "About N of M tokens", saying whether the server counted it or it is estimated, and where the app starts making room.
- "Pinned instructions: X of Y tokens allowed" and "Fixed part without pins: X of Y tokens allowed with tools on".
- If the next message would not be sent, the loop's own reason.
- "Left out next:", taken from the loop's own context steps.
- The sections as rows, collapsed until opened, each with its token estimate.
- A "Copy all" button.

Section names, exactly as the tab shows them: System prompt; Pinned instructions; Tool instructions; Skills list; Loaded skills; Tool definitions; Summary of earlier messages; Messages kept; Messages left out; Task note; Attachments.
- Tool definitions shows the shown tools' names, how many more are on but not shown, and the full definitions.
- Task note shows G's `note_status_for_turn` sentence, plus the note itself when one would be sent. That function existed by the time I needed it, so there is no placeholder for it.

Text is always drawn as plain text, never as markup. Each section draws at most 20,000 characters and then says "Cut here for display: N more characters. Copy all has everything."

The tab reads the view again when:
- the chat changes, or its last message changes;
- a turn ends (nothing is read while a turn runs);
- pins change (`pinned-changed`) or the task changes (`task-changed`);
- the chat's tools change (`chat-tools-changed`) or the tool servers change (`mcp-changed`).

There is no polling.

The "Pin" action is a pin icon labelled "Pin for this chat" on the user's own messages. It sends only the chat id and message id. The store reads the text and refuses anything that is not the user's own message in that chat. On success the panel opens on the Context tab and the pin is listed.

Design choices:
- One assembly function. `turns::assemble(app, &TurnRequest, Assembly::Turn | Assembly::Preview)` builds the turn's input and the shown and hidden tool lists. Every line marked `[0.0.4 …]` was kept, including F's `[0.0.4 profile]` line, which now sits inside the assembly. Preview mode makes the same reads and changes nothing; the steps a real turn records are skipped or replaced:

  | A real turn does | The preview does instead |
  |---|---|
  | `artifacts::note_history` | nothing |
  | `computer_cmd::prepare_turn` | nothing |
  | `tool_select::for_turn` | `tool_select::preview_for_turn` |
  | skills server turn settings | nothing |
  | `task::begin_turn` | `task::note_for_turn` |
  | pins read that also ends the turn on failure | pins read only |
  | `path_to(reply_to)` | `current_path` |

- The request comes from the loop's own code. `agent::preview_turn` builds the turn object and runs the real fit checks, counting, request building and step 1 of making room. Step 2 needs the model to write a summary, so it is reported but never run. Policy, approvals and events are inert in a preview.
- Placeholder: when the chat does not end with a user message, the preview adds "(your next message)", so the task note sits where the next turn will put it. A note in the tab says so, and says the tools shown can change with what that message asks for.
- No secrets: only the request body is shown. The endpoint address, headers and key are never in what is built.

Anything in a real turn I had to touch (what is sent is unchanged):
- `run` now assembles first. So the pins read (and its `?`) happens before the persister starts.
- `task::begin_turn` now runs before the sanitizer probe.
- Tool switch settings are read after assembly.
- The helper's input is now a clone of the full input with history, summary and app notes cleared. That is identical to the earlier clone, which was taken before those were set.

**3. Tests**
- Agent, 4 tests:
  - A preview's request equals, as JSON, the first request a real turn sends: system message, tools, messages, `max_tokens`. Nothing is sent. The system blocks join into the system message.
  - Step 1 is applied when needed, and the next step is named.
  - Messages replaced by the summary are listed as left out.
  - A refusal is the loop's own.
- Store, 2 tests: assistant, tool and system messages, another chat's message, a missing message and a missing chat are all refused; 1,001 characters is refused with its sentence and 1,000 fits; a message with no text is refused.
- App, `context_cmd`, 7 tests:
  - sections in order with their exact texts and totals;
  - neither the endpoint key nor its address reaches the view;
  - no numbers without a model;
  - what goes next and refusals come from the loop;
  - each step's sentence;
  - main window only;
  - in preview mode, every call that records something sits inside an `if turn` branch. This reads the source, because the assembly needs the running app.
- App, `pinned`, 1 new test: the command takes the message id, never text, and tells the window.
- Frontend: 7 inspector tests and 2 Pin-button tests.
- I wrote all of these after the code, so none failed first.
- No unit test runs the assembly itself, because it needs the running Tauri app; the end-to-end scenario covers "no state change" against the real database. It checks the chat, task, tool outputs, audit, pins and messages rows, and the chat's remembered tools.

**4. Checks run**
- `cargo fmt --all --check`: my files are clean. It still flags `crates/policy/src/allow_all/own_command.rs`, `src-tauri/src/evals/sandbox.rs` and `src-tauri/src/evals/sandbox/tests.rs`, which are other developers' files.
- `cargo +1.99 clippy -p agent`, `-p store` and `-p app` (`--all-targets -- -D warnings`): all clean.
- `cargo +1.99 test -p agent`: 151 unit, 53 turn_loop, and 30, 12, 1, 1, 1 in the other suites, all passing.
- `cargo +1.99 test -p store`: 114 and 4 passing.
- `cargo +1.99 test -p app`: 405 passed, 1 failed. The failure is F's `evals::sandbox::tests::the_node_test_runner_runs_and_nothing_rides_along`.
- `npm run lint`: clean. `npm run typecheck`: clean. `npm test`: 36 files, 244 tests passing.
- End-to-end, through the flock line:
  - `PROBES=pinned,task-state,mentions,tool-errors-output,step-limit,context-inspector`: all 6 pass.
  - `PROBES=subagent,tool-selection,artifacts,allow-all`: 3 pass; `subagent` fails (point 6).

**5. For ARCHITECTURE.md**

The Context tab shows exactly what the next request of a chat would contain. It is built by the code that builds a real turn, never by a second copy. `turns::assemble` builds the turn input and tool list for both a real turn and the preview. In preview mode it makes the same reads and changes nothing: it reads the task note without recording that it was sent, chooses the tools without remembering what was shown, and skips the artifacts mark, the computer-use profile and the skills server's turn settings. A remote endpoint's reported context size is not stored either. `agent::preview_turn` then runs the loop's own fit checks, token counting (exact on the local server, otherwise the same estimator and calibrator) and context steps. Step 1 is applied to a copy when needed; step 2, a summary written by the model, is reported and never run, so the preview sends nothing to the model. The view holds only the request body (system message blocks, tool definitions, messages, attachments, task note), never the endpoint address, headers or keys. The tab draws everything as plain text, because it includes tool results and file contents, and caps how much it draws. The inspector is for the main window only. Pinning a message takes only a chat id and a message id: the store reads the stored text and accepts only the user's own message in that chat, so model output, tool results and outside content can never become a pin.

**6. Unverified and open items**
- `subagent` fails at "the run_subagent tool card does not show it is waiting". It failed the same way on two runs. The failure screenshot shows the card reading "Waiting for you": the scenario reads the page text once, as soon as the approval card appears, and the card's status arrives a moment later. Nothing I changed affects those events, but I could not run the pre-refactor build to prove it was failing before. The fix belongs in the scenario (a `wait_for` instead of a one-time read), which is not my file.
- A remote endpoint's context size is asked each time the tab refreshes; this is a network call but nothing is stored.
- Section estimates use the loop's estimator, while the total may be the local server's exact count, so the two can differ by a few tokens.
- The token numbers were only seen with the fake remote endpoint; a local model's exact count was not exercised.
- No model tool was added. No `TODO(0.0.5)` markers left.
