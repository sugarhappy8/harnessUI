# End-to-end tests

These drive the real app (a debug build) through
[`tauri-driver`](https://crates.io/crates/tauri-driver) and WebKitGTK's WebDriver on
Linux. They use only the Python standard library.

They complement, not replace, the manual checklists in each milestone summary: Linux is
a development platform only, so window behaviour, keychains, GPUs and installers are
tested on Windows and macOS by hand.

## Setup (Linux)

```bash
sudo apt-get install -y webkit2gtk-driver xvfb xdotool
```

```bash
cargo install tauri-driver --locked
```

```bash
python3 scripts/fetch_llama.py linux
```

```bash
python3 scripts/fetch_test_model.py
```

## Run

Start the Vite dev server (debug builds load the UI from it), build the app, then run a
scenario under a virtual display:

```bash
npm run dev
```

```bash
cargo build -p app
```

```bash
xvfb-run -a python3 -I e2e/m1_chat.py
```

Environment variables: `APP_BIN` (default `target/debug/app`), `TEST_GGUF`,
`APP_DEV_LLAMA_DIR` (default `.cache/llama-linux`), `E2E_OUT` (screenshots and logs).
Each run uses a fresh temporary home folder, so it never touches your own app data.

Writing scenarios: `click_button(label)` clicks the first visible button whose text contains
the label. A model's name is on the menu's own button and on the welcome screen as well as
in the menu, so pick a model with `click_menu_item(name)`.

## Scenarios

| Script | Covers |
|---|---|
| `m0_close.py` | Closing the main window quits the app (no hidden copy left running), and it starts again. Needs `xdotool` |
| `m1_chat.py` | First run, loading a GGUF, the same model picked again while it loads, another model picked while one loads, picking a model with no chat open, streaming, regenerate and versions, edit-and-resend, stop, search, rename, delete, an OpenAI-compatible endpoint |
| `m2_models.py` | Hugging Face search, a real download with verification, installed models, a vision model with its projector |
| `m3_agent.py` | A tool-calling turn: Allow once, Allow for this chat then a call under the grant, Deny, Stop with a card open, audit rows |
| `m5_skills.py` | Creating a skill in the editor, the strict validator, the model loading it without a card, disable and delete |
| `m6_browser.py` | The browser tool registered and off by default, the browser found, Settings → Browser |
| `scripted_agent.py` | No model needed: a fake endpoint with scripted replies (text, thinking, tool calls, HTTP errors, dropped and unfinished streams). Markdown safety, every settings page, per-chat folders and working folder, skill loading and unloading, terminal and git cards (a push URL's token never shown), failed turns (one error message, partial text kept, fitting advice), stop, regenerate, edit-and-resend, switching or deleting a chat mid-reply, recovered tool calls, forgetting grants, MCP config refusals, subagents (off in a new chat, switched on in the Tools panel; the helper's fresh history and tool list, its approval card and audit rows, nothing of it stored in the chat or counted in its context; a nested call refused; Stop during the helper), the step limit (refused values, the wrap-up request, the notice with Continue, the setting in Settings → General) |
| `m9_model_check.py` | 0.0.4: "Check this model" with the real test model: every setting listed with a mark per probe (passing is not required), temporary folders gone, audit rows with no chat, nothing written outside the temporary folders; then a benchmark ended early with "End the benchmark", which must stop within a few seconds and save the partial run |
| `m8_offline.py` | Start, a local chat and every settings page with all proxies pointing at a recorder that refuses everything: no outbound request |

`scripted_agent.py` also has one scenario per 0.0.4 feature. `PROBES=name,name` runs only
those (for example `PROBES=mentions xvfb-run -a python3 -I e2e/scripted_agent.py`). Step 1
added `allow-all`, `artifacts`, `search` and `tool-selection`; steps 2 and 3 added:

| Scenario | Covers |
|---|---|
| `tool-errors-output` | An ambiguous `edit_file` asks as a read and returns the match list as data, the second call with `occurrence` edits; a 6,000-line `read_file` is shortened with the "Shortened" line; `read_output` with grep runs without a card; `get_tool_output` refuses another chat; the card's *Full output* opens the saved text |
| `task-state` | `update_task` sets goal and steps; a `write_file` appears under changed files; Pause stops a streamed reply; the task survives a reload; Resume sends the continue message with the task note once; an ordinary follow-up carries no note; an edit through the command refreshes the tab |
| `mentions` | `@add` lists the definition in `calc.py`; sending it puts the definition lines with path and line range in the request; the chip survives a reload; a path outside the approved folders is refused by both commands and by `send_message` |
| `pinned` | Pins for this chat and for every chat typed in the Context tab; the exact block, once, after the system prompt; another chat gets only the every-chat pin; pins survive a reload |
| `context-inspector` | What the fake endpoint receives equals what the inspector showed just before (system message, tools in order, messages, task note); two inspections change no database row |
| `capability-check` | The model check's confirmation, two settings by three probes, the saved profile (and `set_setting` refusing its key), audit rows with no chat, temporary folders removed, `temperature: 0.2` in a chat turn and none after Forget; a benchmark run through *Settings → Diagnostics*, saved, listed and loaded |

The scenarios with a model use Qwen3 0.6B, which sometimes answers without calling the
tool it was asked to use; those steps ask again up to three times.
