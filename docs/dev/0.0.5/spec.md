# 0.0.5: specification

The owner's four requests (verbatim):

1. allow per model on loading context window setting and it auto compresses.
2. More and more model loading setting and model reloading but simplify it to normal human language.
3. Allow for local loaded models to be able to talk to Claude or codex apps upon user request.
4. Add model comparisons for models so users can compare different models and what their good at from the download huggingface thing we have

The owner answered one question: for item 3 they want **both directions**. Other apps (Claude
Code, Codex, the Claude desktop app) may use the model loaded here, and the local model may
hand a task to Claude Code or Codex. Each direction is off by default, behind its own switch.

Background reading, in this order: this file; `docs/dev/0.0.5/map.md` (combined map of the
code, with the decisions it asks for); `docs/dev/0.0.5/map-details.md` (the five readers'
findings, with file and line references as of commit 226bf13); `ARCHITECTURE.md` sections
named in your package.

---------------------------------------------------------------------------------------

## Part 1. Rules for every developer

### Working in one tree
- Several developers edit this working tree at the same time. Edit only the files your
  package owns (Part 3) and, in the shared files listed in Part 2, only the lines your
  package adds. Read a shared file again right before each edit; make one small, additive
  edit; never reformat, reorder or rewrite other people's lines.
- Never change git state (no commit, stash, checkout, reset, restore, clean, rebase). The
  lead commits. Never run `cargo clean`, never delete `target/`.
- If the build breaks in a file you do not own, someone is mid-edit: wait 60 to 90 seconds
  and retry, up to 5 times, then say so in your report. Do not fix their file.

### Building and testing
- `export CARGO_INCREMENTAL=0` before every cargo command. Always `cargo +1.99 …` (also
  `cargo +1.99 build -p app`); `cargo fmt --all` is the one exception. Bash timeout
  600000 ms for cargo. Never build with `--release`. Check `df -h /` before a large build;
  under 3 GB free, stop building and say so.
- TypeScript: match the surrounding style by hand; do not run prettier over files. Run
  `npm run lint`, `npm run typecheck`, `npm test`.
- End-to-end tests drive the real debug app under Xvfb with a fake model endpoint
  (`e2e/scripted_agent.py`, read `e2e/README.md` and one short scenario first). The runner
  starts its own dev server when port 1420 is free. Runs take turns through a lock:
  `cargo +1.99 build -p app` then
  `flock /tmp/harness-e2e.lock env PROBES=<name> E2E_OUT=<your own scratch folder> timeout 1200 xvfb-run -a python3 -I e2e/scripted_agent.py`.
  Add `E2E_TRACE=1` to see what the app did. A real small model is at
  `/home/user/llama-dev/Qwen3-0.6B-Q8_0.gguf` with llama-server in
  `/home/user/llama-dev/llama-b12110/` (see `e2e/m1_chat.py` for how a script loads it).
- Tests are part of the work. Write the test first where practical and say in your report
  which tests you saw fail first. Never weaken or delete an existing test to get green; if
  a test pins wording that this release deliberately changes (decision D7), change the
  expectation in the same edit and say so. Path sandboxing, permission rules, tool-call
  parsing, argument building for processes and anything that listens on a port always get
  unit tests.

### Rules that are never traded away
- No telemetry. Secrets only in the OS keychain (`state.secrets`), never in settings, files
  or logs. The webview never loads remote content. Nothing the model outputs may change
  approval settings, grants, approved folders, tool switches, model settings or the sharing
  switch. Tool output, web pages, file contents, model output and data from other apps are
  data, never instructions.
- Every tool action goes through `describe`, `policy::evaluate` and the audit log. Every new
  process goes through `crates/runtime/src/spawn.rs` (`scripts/check-spawn.sh` enforces it),
  with an argument list, never a shell string built from model text.
- Anything that listens on a port binds `127.0.0.1` only, never `0.0.0.0` or a LAN address.
- New Tauri commands: registered in `generate_handler!` (`src-tauri/src/lib.rs`), in
  `src-tauri/commands.list`, and as `allow-<kebab-name>` in
  `src-tauri/capabilities/main.json` (a test enforces all three); main window only, and the
  command checks the calling window's label like the existing ones do.
- New dependencies only when truly needed; say why in one line, then run
  `python3 scripts/notices.py` (CI runs `--check`) and make sure `cargo deny check` passes
  (licences: MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0 files).
- User-facing text: plain, short, no jargon, no exclamation marks; say what a thing does
  before how. See decision D8 for names of outside apps.
- No silent stubs. If something cannot be finished, leave `TODO(0.0.6): <what is left>` and
  list it in your report. Do not claim something works unless you ran it.
- Do not edit `CHANGELOG.md`, `ARCHITECTURE.md`, `README.md`, `SECURITY.md` or `docs/`; the
  lead writes them from your report.

### Your final message (the report)
Plain text: (1) files created and changed (for shared files, exactly what you added);
(2) what the feature does now, as the user sees it, and the design choices you made;
(3) tests added, and which you saw fail first; (4) exact checks run with pass/fail counts,
and anything not run; (5) a paragraph "For ARCHITECTURE.md"; (6) what is unverified (for
example "GPU flags not tried on a real graphics card") and every TODO.

---------------------------------------------------------------------------------------

## Part 2. Shared decisions

### Decisions taken by the lead for the owner (the owner may change them later)
- **D1 Per model means per model file** (file name + size, not path, not backend), so a moved
  file keeps its settings and files opened with "Open a GGUF file…" work too. A model's
  settings are remembered and used automatically every time it loads: loading stays one
  click. Changing them is in the model's own settings panel.
- **D2 "Auto compresses" means three things.** (a) Long chats are already shortened
  automatically; make that visible and robust (a lasting divider with "Show summary", a
  plain note per stage, failures reported, a summary that still fits when the model's
  context gets smaller). (b) A new context choice **Automatic** picks the largest size that
  fits this computer's memory with the model's other settings, up to the model's trained
  length and at most 32,768 tokens, never below 4,096 (or the trained length if smaller).
  Automatic is the default for models with no saved choice; an existing user's saved global
  size stays their default. (c) A **Save memory** option stores the conversation memory at
  half size (`-fa on -ctk q8_0 -ctv q8_0`) so a longer context fits, at slightly lower
  quality. Off by default.
- **D3 Changing a loaded model's settings** takes effect on **Apply and restart**, never by
  itself. If a reply is being written (or another app is using the model, item 3), the app
  asks first: "A reply is being written. Stop it and restart the model?" with **Stop and
  restart** and **Wait** (wait = restart as soon as the reply ends).
- **D4 Settings shown, in plain words** (per model, each defaulting to "Defaults for all
  models" in Settings → Local models):
  - **How much it can remember** (context): Automatic, or sizes up to the trained length,
    each with a fit mark for this computer.
  - **Use the graphics card**: **All of it** (`-ngl all`, today's behaviour and the default),
    **As much as fits** (omit `-ngl`, `--fit on`), **Don't use it** (processor only).
  - **Save memory** (D2c).
  - **Keep the model in memory** (`-lm mmap+mlock`): stops the computer from moving it out
    when memory is short; uses more memory.
  - Under **More options**: **Which graphics card** (device, when there is more than one),
    **How long it may think** (thinking models only: Default, Short, Medium, Long, mapped to
    fixed `--reasoning-budget` values; never free text).
  - Never shown: threads, batch sizes, slots, rope, tensor split, free-text extra arguments,
    file paths. Every setting is a typed value from a fixed list.
  The backend (Vulkan, CUDA, …) stays one global choice.
- **D5 Item 3 is experimental in both directions** and says so in the app. Both are off by
  default.
- **D6 "What a model is good at"** comes only from: (1) short curated text in the signed
  catalog, written from each model publisher's own model card and labelled "The publisher
  says"; (2) the uploader's own tags and description fields on Hugging Face, mapped to plain
  phrases and labelled "The uploader lists"; (3) the user's own results on this computer
  (0.0.4 Tested settings and coding benchmarks), labelled "Your results". Never publisher
  benchmark numbers, never a model's own summary of a card, never made-up scores.
- **D7 Labels pinned by tests may be renamed** when this release changes the wording; the
  test changes in the same edit.
- **D8 Names of outside apps.** The 0.0.4 rule "no names or identifiers of AI models or
  assistants in code, comments, tests or docs" is amended for 0.0.5: the program names and
  product names of the outside apps the owner asked to integrate (Claude Code, the Claude
  app, Codex, and in the policy classifier other coding-assistant programs) may appear in
  the policy classifier, the outside-app modules and setup text, and their tests. Names of
  models still never appear in code; they live in the catalog.
- **D9 No database schema change in 0.0.5** unless a package proves it unavoidable (then it
  asks the lead first). New data goes in the `settings` key/value table (JSON values) or in
  existing JSON columns. Audit rows reuse existing `decision` values.
- **D10 Item 3, direction A, uses a small proxy owned by the app** on a stable loopback port,
  with its own sharing key; llama-server stays on its random port with its private key.
- **D11 Item 3, direction B, asks every time.** Handing a task to Claude Code or Codex always
  shows a card, in every approval mode, and "Allow for this chat" never covers it.

### Storage (settings keys; no migration)
- `localModel` (existing, `LocalModelSettings` in `src-tauri/src/dto.rs`): the **defaults for
  all models**. Gains the new fields of D4, each `#[serde(default)]` so the 0.0.4 value
  already on users' machines and the 0.0.4 command shape (`{ctx, backend, device}`, used by
  the e2e scripts) still read. `ctx` gains the Automatic choice without breaking a stored
  number (the builder chooses the encoding and tests old and new JSON both ways).
- `modelLoad:<first 16 hex of SHA-256("local\n<file name>\n<size in bytes>")>`: one model's
  overrides, every field optional (`None` = follow the defaults for all models). Written
  only by its own validated command; `set_setting`'s allow-list stays as it is. "Reset to
  defaults" deletes the key.
- `shareWithApps`: `{enabled: bool, port: u16}` (item 3A). The sharing key lives in the
  keychain under its own name.
- Catalog: new optional fields only, `schemaVersion` stays 1, `sequence` goes to 2.
- Facts saved at download time (item 4) go in the existing `gguf_summary` JSON of the
  `models` row, with `#[serde(default)]` fields.

### Shared contracts (built first by the groundwork package, WP0)
WP0 adds these with full types and working registration; bodies that belong to another
package start as `todo!()`-free stubs that return a clear "not built yet" error, so every
package can compile against them from the start.
- Rust DTOs (in `src-tauri/src/dto.rs` or a new `src-tauri/src/load_settings.rs`):
  `ModelLoadSettings` (per-model overrides), the extended `LocalModelSettings`,
  `ContextChoice`, `GpuUse`, `ThinkingLimit`, `ModelLoadInfo` (for the settings panel:
  trained length, file size, saved overrides, resolved values, fit per candidate context
  and per GPU choice, whether it is the loaded model and whether the loaded copy uses
  older settings), `CompareEntry` / `CompareFacts` (item 4), `ShareStatus` (item 3A).
- Commands (names fixed; owners in brackets): `model_load_info(path)` [WP1],
  `set_model_load_settings(path, overrides)` [WP1], `reset_model_load_settings(path)` [WP1],
  `reload_local_model(force)` [WP1], `unload_model` (exists; wire it) [WP1],
  `generation_busy()` [WP1], `catalog_list(ctx)` [WP4a], `compare_facts(items, ctx)` [WP4a],
  `share_status()` / `set_share_enabled(on)` / `set_share_port(port)` /
  `reveal_share_key()` / `new_share_key()` [WP5]. `load_local_model(path, settings?)`
  keeps its name; `settings` becomes optional (absent = resolve the model's saved settings).
- Frontend: types and `ipc.ts` wrappers for all of the above; `src/lib/words.ts` (plain
  words for backends, load stages, warnings; WP2 fills it); the split of
  `src/components/ModelsDialog.tsx` into `src/components/models/{Installed,Browse,RepoDetail,Downloads}.tsx`
  with the dialog keeping only the shell and tabs; the move of `Runtime()` out of
  `SettingsDialog.tsx` into `src/components/settings/LocalModels.tsx`. Behaviour and
  labels unchanged by the split (the existing tests and e2e must still pass).
- `LaunchConfig` gets a constructor or `Default` so new fields do not touch every literal.
- `state.generation` gets a read-only "is anything being written" query.

### Shared files (append only, read before each edit)
`src-tauri/src/lib.rs`, `src-tauri/commands.list`, `src-tauri/capabilities/main.json`,
`src-tauri/src/dto.rs`, `src-tauri/src/settings.rs`, `src-tauri/src/toolset.rs`,
`src-tauri/src/tool_catalog.rs`, `src/lib/ipc.ts`, `src/lib/types.ts`, `src/store/app.ts`,
`src/components/SettingsDialog.tsx` (one tab or one mount each),
`src/components/ModelsDialog.tsx` (one tab each), `Cargo.toml` and `src-tauri/Cargo.toml`
(dependencies), `e2e/scripted_agent.py` (your own scenario function and one `PROBES`
entry). Lead only: `crates/store/src/schema.rs`, all docs.

---------------------------------------------------------------------------------------

## Part 3. Work packages

### WP0. Groundwork (one developer, before everyone else)
Everything under "Shared contracts" above. Plus: nothing else changes behaviour. Checks:
the whole workspace builds and its tests pass, frontend lint/typecheck/tests pass, and the
e2e scenarios `settings-pages` and the models script `e2e/m2_models.py` still pass (the
latter needs the real small model; if it cannot run, say why). Report the exact contract
(types and command signatures) so the other packages can rely on it.

### WP1. Load engine (items 1 and 2, Rust)
**Owns:** `crates/runtime/src/llama/launch.rs`, `crates/runtime/src/llama/server.rs` (test
literals only), `crates/runtime/src/llama/server/supervisor.rs`,
`crates/runtime/src/llama/failure.rs`, `crates/runtime/tests/llama_server.rs`, new
`crates/runtime/src/llama/fit_params.rs` if used, `src-tauri/src/runtime_manager.rs`,
`src-tauri/src/load_tracker.rs`, `src-tauri/src/models_cmd.rs`,
`src-tauri/src/load_settings.rs`, `crates/models/src/fit.rs`, `crates/models/src/summary.rs`,
the busy query in `src-tauri/src/state.rs`; one-line edits in `src-tauri/src/mcp_cmd.rs`
(`context_for_defaults`) and `src-tauri/src/hub.rs` (the Browse fit check uses the
model's resolved context).
**Build:**
- The resolver: per-model overrides, then defaults for all models, then the projector, then
  the trained-length cap, then Automatic sizing (D2b). One function used by every load path:
  `load_local_model` (settings absent), reload, `run_diagnostics` (which must also stop
  losing the image projector and the thinking setting), first run.
- New launch fields as typed values (D4) and the exact arguments they produce; keep `-c`
  explicit, keep `--no-context-shift`, `--offline`, `--no-slots`, `--cors-origins none`,
  `-np 1`, the environment scrubbing and the ROCm device masking. Update `args_are_exact`
  and the contract tests; add contract tests against the pinned binary for every new flag
  combination that can run on a processor (Save memory on CPU, Keep in memory if the
  sandbox allows it, As much as fits with `--fit on`).
- Fit sizing aware of Save memory (cache bytes per token for q8_0) and of the GPU choice;
  count the memory of a model that will be unloaded first as available. Say in the report
  how exact this is.
- Reload and unload: `reload_local_model(force)` restarts with the current resolved
  settings even when nothing changed and retries the chosen backend after a step-down.
  The "already loaded" check and `same_in_progress` compare every launch-relevant field.
  `generation_busy` for D3 (the UI asks; the backend never stops a reply by itself).
- Step-down on a graphics-card memory failure: try **As much as fits** before the
  processor, so the out-of-memory message ("run part of the model on the processor") is
  true. Messages in plain words (D4 wording), passed through the existing status events.
- Commands: `model_load_info`, `set_model_load_settings` (validates every value against the
  fixed lists and the model's trained length), `reset_model_load_settings`,
  `reload_local_model`, `unload_model` wiring, `generation_busy`.
**Tests:** resolver order and old-JSON compatibility both ways; argument building for every
choice; Automatic sizing on fixed hardware numbers; reload and no-op rules; step-down
order; command validation (bad values refused, model output can never reach these
commands because they are UI commands only). Real-binary contract tests as above.
**e2e:** extend `e2e/m1_chat.py` or add `e2e/m10_model_settings.py` with the real small
model: save a per-model context, load, see the size reported, Apply and restart with Save
memory, Reload, Unload.
**Unverifiable here:** GPU flags (this machine has no graphics card). Say so.

### WP2. Load settings panel and plain words (items 1 and 2, frontend)
**Owns:** new `src/components/ModelSettings.tsx` (+ test), `src/components/settings/LocalModels.tsx`
(after WP0 moves it), `src/components/RuntimeIndicator.tsx` (+ test), `src/lib/words.ts`,
`src/components/models/Installed.tsx`, `src/components/FirstRun.tsx`,
`src/components/Welcome.tsx` (+ test), the model chip text in `src/components/Sidebar.tsx`,
and `pickModel` / `openModelFile` in `src/store/app.ts`.
**Build:**
- **Model settings panel** for one model (opened from a gear on each Installed row, from the
  model chip / runtime indicator in the chat header, and from "Load with settings…"): the
  D4 controls, each showing "Same as default (…)" until changed, a fit mark per context
  choice and GPU choice from `model_load_info`, **Apply and restart** when it is the loaded
  model, **Save** otherwise, **Reset to defaults**. Every control has one short help line.
- **Defaults for all models** (Settings → Local models, renamed section): the same controls
  for the global defaults, plus the backend choice in plain words.
- **Runtime indicator** in the chat header: plain words for every state and stage from
  `words.ts` (no raw backend or stage names); a menu with **Reload**, **Model settings…**
  and **Unload to free memory**; "Settings changed. Restart the model to use them." when
  the loaded copy is older than its saved settings; the D3 question when a reply is being
  written.
- Loading from the model list and from Installed passes no settings (the backend resolves
  them), so per-model settings apply everywhere.
- `words.ts`: backends ("Graphics card (Vulkan)", "NVIDIA graphics card (CUDA)", "AMD
  graphics card (ROCm)", "Apple graphics (Metal)", "Processor only (slower)"), load stages,
  step-down and failure messages, context sizes ("8,192 tokens, about 6,000 words"), with a
  unit test that every backend kind and stage the backend can send has a phrase.
**Tests:** component tests for the panel (defaults shown, change, Apply and restart, the D3
question, Reset), the indicator (each state's words, the menu), words coverage.
**e2e:** scenario `model-settings` in `e2e/scripted_agent.py` (fake endpoint where possible;
local-model parts go in WP1's script): open the panel, change context and Save memory,
see "Settings changed…", Apply and restart.

### WP3. Conversation memory (item 1, "auto compresses")
**Owns:** `crates/agent/src/context.rs`, `crates/agent/src/turn/runner.rs`,
`crates/agent/src/turn/mod.rs`, `crates/agent/src/turn/runner/preview.rs`,
`crates/agent/src/prompt.rs` (summary block only), `crates/agent/tests/turn_loop.rs`
(append), `src-tauri/src/turns.rs` (the `compacted` event and summary plumbing only),
`src-tauri/src/context_cmd.rs`, `src-tauri/src/tool_select.rs` (`other_fixed`),
`crates/store/src/tools.rs` (summary read for chat detail), the chat-detail summary field in
`src-tauri/src/chat.rs`, `src-tauri/src/attachments.rs` (document cap),
`src/components/ChatView.tsx` (divider and notes), `src/components/context/ContextInspector.tsx`,
and the `compacted` handler in `src/store/app.ts`.
**Build:**
- The `compacted` event carries the stage; the chat shows a plain note per stage ("Older
  tool results were shortened to make room.", "Earlier messages were summarised to make
  room.", "Older steps of this reply were shortened to make room."), replacing today's one
  note that is wrong for two stages.
- A lasting divider where the summary starts: "Earlier messages are summarised for the
  model." with **Show summary** (plain text, never markup; the summary is model output).
  Needs the summary on the chat-detail DTO.
- A failed summary is reported in plain words and the turn goes on with the other stages.
- Robust when the context gets smaller between turns (after a reload with a smaller size,
  or a per-model change): an old summary that no longer fits is folded again under the
  smaller cap; `tool_select::other_fixed` counts the summary; documents attached to a
  message are cut to a share of the context with a note saying how much was left out and
  that the file tools can read more (as mentions do).
- The Context inspector shows the new behaviour through the same code (`turns::assemble`
  and `agent::preview_turn`), including "what would be summarised next".
**Tests (agent crate, scripted endpoint):** "a chat summarised at 32k continues at 4k", "an
oversized old summary is folded", "an attachment larger than the context is shortened",
"a failed summary is reported and the turn continues", stage notes; frontend tests for the
divider and notes.
**e2e:** scenario `chat-memory`: a long scripted chat on a small context shows the divider
and Show summary works after a window reload.

### WP4a. Comparison data (item 4)
**Owns:** `crates/models/src/catalog.rs`, `catalog/models.json`, `crates/models/src/hf.rs`,
`src-tauri/src/hub.rs` (except WP1's one-line fit change), `src-tauri/src/downloads.rs`
(`register_model`), a prefix query in `crates/store/src/config.rs`, a "list profiles"
helper in `src-tauri/src/evals/commands.rs`.
**Build:**
- Catalog: optional `summary` (one or two sentences), `goodAt` (subset of a fixed list:
  chat, writing, coding, reasoning, math, tools, images, long-documents, languages, speed;
  unknown keys ignored), `parameters`, `activeParameters`, `publisher`, `languages`,
  `released`, `source` (the URL of the publisher's model card used). `schemaVersion` stays
  1; `sequence` 2; older app versions must still read it (test with the 0.0.4 parser
  behaviour: unknown fields ignored). Fill the fields for each catalog entry **only from the
  publisher's own model card** (fetch it from huggingface.co; if a card cannot be read,
  leave the fields out). Correct any licence that disagrees with the publisher's card (the
  map notes the Gemma entries). The release workflow signs the catalog; nothing to sign here.
- Hugging Face: keep `pipeline_tag`, base model, languages, licence name and link, and the
  tags in `parse_repo_detail`; show likes and "last updated" in search results data.
- Save the catalog id and those Hugging Face facts at download time, so comparison works
  offline for installed models.
- "Your results": list the 0.0.4 model profiles and benchmark runs per model file (match by
  file name + size, as profiles already do).
- Commands `catalog_list(ctx)` (entries with fit at that context, installed state, your
  results) and `compare_facts(items, ctx)` (the facts for up to 4 chosen models: catalog,
  Hugging Face if known or fetchable through the existing `HfClient`, installed GGUF facts,
  one fit verdict per model at the one chosen context, your results). Never bulk-fetch:
  only models the user added to the comparison.
**Tests:** catalog parsing old and new, `goodAt` vocabulary, licence fields, Hugging Face
parsing from saved fixtures, saved-at-download facts, matching your results, the commands
with an offline store.

### WP4b. Comparison screen (item 4)
**Owns:** new `src/components/models/Compare.tsx` (+ test), `src/components/models/Browse.tsx`,
`src/components/models/RepoDetail.tsx`, new `src/lib/abilities.ts` (plain phrases for
`goodAt` keys, Hugging Face tags and pipeline tags), reuses `FitBadge.tsx`. One tab
"Compare" in `ModelsDialog.tsx`. The "Add to comparison" control on Installed rows is
added by WP2 (it owns Installed.tsx) using the exported hook/store from this package.
**Build:**
- **Add to comparison** on Browse results, on a repository's detail page and on Installed
  rows; up to 4 models; the Compare tab lists them side by side.
- Rows phrased as questions, each with its source in small text: "What is it good at?" (The
  publisher says / The uploader lists / Your results), "Will it run well on this computer?"
  (one fit verdict at the context chosen at the top: "Compare at …"), "Download size",
  "Can it use tools?", "Can it read images?", "Does it think before answering?", "Longest
  conversation it was made for", "Licence", "Downloads on Hugging Face", "Your test results".
  Missing data says "Not known", never a guess. A model known only from Hugging Face says
  when its facts need the internet.
- Plain words for quantisation names and sizes (for example "Q4_K_M: smaller file, good
  quality").
**Tests:** component tests for adding/removing, row sources, missing data, offline entries.
**e2e:** scenario `compare-models` with a fake catalog/Hugging Face response if the e2e
harness supports it, otherwise installed + catalog only.

### WP5. Other apps use this model (item 3, direction A)
**Owns:** new `src-tauri/src/share/` (`mod.rs`, `proxy.rs`, `setup_text.rs`, tests), new
`src/components/settings/OtherApps.tsx` (+ test), new `crates/runtime/tests/llama_endpoints.rs`
(contract tests). One new Settings tab "Other apps" (append to the tab list). One launch
change, coordinated with WP1 (WP1 owns `launch.rs`): an `alias` launch field so the API
reports a short model name instead of the file path, if the contract test shows it works
without breaking the start-up identity check; otherwise the proxy answers `/v1/models`
itself and strips paths, and the report says how.
**Build:**
- A switch **Let other apps on this computer use the loaded model** (off by default,
  experimental). When on: a proxy on `127.0.0.1:<port>` (default port fixed by the builder,
  changeable to another number), its own sharing key in the keychain (**Show key**, **Copy**,
  **Make a new key**; the app's private llama-server key is never shown).
- The proxy allows only `POST /v1/messages`, `POST /v1/messages/count_tokens`,
  `POST /v1/responses`, `POST /v1/chat/completions` and `GET /v1/models` (answered by the
  proxy with the short name); everything else is refused. It checks the sharing key
  (`x-api-key` or `Authorization: Bearer`), refuses any request carrying an `Origin` header
  and any `Host` other than `127.0.0.1:<port>` or `localhost:<port>` (web pages and DNS
  rebinding), caps request size, forwards with the private key, and streams responses
  through unchanged. No model loaded or a model loading: a clear error in the API's own
  error format ("No model is loaded in harnessUI. Open harnessUI and load a model.").
- One audit row per request (server "other-apps", the path, token counts, status, the
  calling program's name from User-Agent cleaned and shortened; never the request text),
  reusing an existing `decision` value (D9).
- The chat header shows "Used by another app" while a shared request runs; the D3 question
  covers it.
- If the loaded model's context is under 32,768: "Claude Code and Codex send long
  instructions. Give this model at least 32,768 tokens to remember." with a button that sets
  that for this model (WP1's command) and asks to restart.
- Setup cards with copy buttons for **Claude Code** (environment variables or the `env` block
  of its settings file), **Codex** (a provider block in its `config.toml` with
  `wire_api = "responses"`), **the Claude app** (its third-party inference setting; marked
  "not tried yet"). Each says, in one line, that the key is then stored as plain text in
  that app's settings file and that the setup is experimental. Exact variable names come
  from official documentation (see `map-details.md`, item 3, for the verified list).
- If the port is taken by another program: "Port N is used by another program. Choose
  another port." The switch never turns itself on.
**Tests:** proxy unit tests (allowed paths, key check both header forms, Origin and Host
refusal, body cap, no-model error per API shape, streaming pass-through, path never leaked),
contract tests against the pinned binary (`/v1/messages`, `/v1/responses`, multi-key auth,
alias), commands main-window only, the sharing key never in settings or logs.
**e2e:** scenario `other-apps`: switch on, read the key through the UI, send an Anthropic
Messages request to the proxy from the test (with and without the key, with an Origin
header), see the audit row, switch off and see the port closed.

### WP6. The local model asks Claude Code or Codex (item 3, direction B) and the classifier fix
**Owns:** new `src-tauri/src/outside_agent.rs` (+ tests), the new scope in
`crates/policy/src/engine.rs` (and wherever scopes are listed), registration lines in
`src-tauri/src/toolset.rs` and `src-tauri/src/tool_catalog.rs`, the switch in the Tools
panel (`src/components/ToolsPanel.tsx`), `crates/policy/src/allow_all.rs` and
`crates/policy/src/command.rs` (classifier), their tests.
**Build:**
- **Classifier fix (whatever else happens):** in Allow all, a command line that runs a
  coding-assistant program (`claude`, `codex`, and others of the same kind the builder
  confirms) always asks (keep-asking, hidden) and counts as going online. Tests both ways.
- A tool group **Coding apps** in the Tools panel, off in new chats, switched per chat. Two
  tools, `ask_claude_code {task, may_edit?}` and `ask_codex {task, may_edit?}`, each offered
  only when its program is found. Each call always asks (D11): a card naming the app, the
  task, the folder (the chat's working folder, which must be approved), whether it may
  change files, and "It runs online with your <app> account. What it reads is sent to
  <company>."
- Fixed argument templates verified against official documentation; the task is one
  argument, never parsed by a shell; never any flag that skips the outside app's own
  permissions or sandbox; the outside app's project-level hooks and servers are not loaded
  if the program offers a way to skip them (say what you used). Read-only by default;
  `may_edit` maps to that app's narrowest "may edit files in this folder" mode. Secret
  environment variables stay stripped (the apps use their own stored login).
- Long runs: a generous time limit, **Stop** kills the process tree, the result is returned
  as untrusted tool output and goes through the existing output shortening.
- Not available to helpers started by `run_subagent`.
**Tests:** argument building per template and mode (no bypass flag can ever appear), the
always-ask rule in both approval modes including after a grant, folder refusal, program
detection, Stop, classifier rows.
**e2e:** scenario `coding-apps`: with a fake `claude` and fake `codex` program placed first
on PATH by the test (a script that prints its arguments), the tool shows its card, Deny
runs nothing, Allow once runs the fake program with the expected arguments and returns its
output.

---------------------------------------------------------------------------------------

## Part 4. Order
1. WP0 alone.
2. WP1, WP2, WP3, WP4a, WP4b, WP5, WP6 in parallel. WP2 and WP4b work against WP0's
   contract; WP5 coordinates its one launch field with WP1 through the lead.
3. Independent reviews of every package (security and correctness), fixes.
4. The lead integrates, runs everything, writes the docs, raises the version to 0.0.5.
