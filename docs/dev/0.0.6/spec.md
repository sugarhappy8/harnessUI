# 0.0.5: specification (revision 2, after three independent critiques)

The owner's four requests (verbatim):

1. allow per model on loading context window setting and it auto compresses.
2. More and more model loading setting and model reloading but simplify it to normal human language.
3. Allow for local loaded models to be able to talk to Claude or codex apps upon user request.
4. Add model comparisons for models so users can compare different models and what their good at from the download huggingface thing we have

The owner answered one question: for item 3 they want **both directions**. Other apps (Claude
Code, Codex, the Claude desktop app) may use the model loaded here, and the local model may
hand a task to Claude Code or Codex. Each direction is off by default, behind its own switch.

The owner is not technical. They use the app on a Windows PC with an AMD Radeon RX 9070 XT
graphics card (Vulkan) and on a 16 GB Apple silicon Mac. This build machine is Linux with no
graphics card.

Background reading, in this order: this file; `docs/dev/0.0.6/map.md` (combined map of the
code); `docs/dev/0.0.6/map-details.md` (the readers' findings with file:line references as
of commit 226bf13; item 3 lists the verified variable names and flags of the outside apps);
the `ARCHITECTURE.md` sections your package names. The critiques that shaped this revision
are summarised in Part 5.

---------------------------------------------------------------------------------------

## Part 1. Rules for every developer

### Working in one tree
- Several developers edit this working tree at the same time. Edit only the files your
  package owns (Part 3) and, in the shared files of Part 2, only the lines your package
  adds. Read a shared file again right before each edit; make one small additive edit;
  never reformat, reorder or rewrite other people's lines.
- Never change git state (no commit, stash, checkout, reset, restore, clean, rebase). The
  lead commits. Never run `cargo clean`, never delete `target/`.
- If the build breaks in a file you do not own, someone is mid-edit: wait 60 to 90 seconds
  and retry, up to 5 times, then say so in your report. Do not fix their file.
- When your package needs something another package owns, build against the WP0 contract
  (which returns fixed sample data until the owner lands it) and test with mocks; say in
  your report which end-to-end checks must wait for integration.

### Building and testing
- `export CARGO_INCREMENTAL=0` before every cargo command. Always `cargo +1.99 …` (also
  `cargo +1.99 build -p app`); `cargo fmt --all` is the one exception. Bash timeout 600000
  ms for cargo. Never `--release`. Check `df -h /` before a large build; under 3 GB free,
  stop building and say so. This machine has 4 processors and builds wait on a shared lock:
  build only what you need (`-p <crate>`), and do not start a second build while one runs.
- TypeScript: match the surrounding style by hand; do not run prettier over files. Run
  `npm run lint`, `npm run typecheck`, `npm test`.
- End-to-end tests drive the real debug app under Xvfb with a fake model endpoint
  (`e2e/scripted_agent.py`; read `e2e/README.md` and one short scenario first). The runner
  starts its own dev server when port 1420 is free. Runs take turns through a lock:
  `cargo +1.99 build -p app` then
  `flock /tmp/harness-e2e.lock env PROBES=<name> E2E_OUT=<your own scratch folder> timeout 1200 xvfb-run -a python3 -I e2e/scripted_agent.py`.
  Add `E2E_TRACE=1` to see what the app did. A real small model is at
  `/home/user/llama-dev/Qwen3-0.6B-Q8_0.gguf` with llama-server in
  `/home/user/llama-dev/llama-b12110/` (see `e2e/m1_chat.py` for how a script loads it).
  Claude Code 2.1.322 is installed on this machine (`claude`); Codex is not.
- Tests are part of the work. Write the test first where practical and say which tests you
  saw fail first. Never weaken or delete an existing test to get green. If a test pins
  wording this release deliberately changes (D7), change the expectation in the same edit,
  update **every** test and e2e line that pins it (shared files included), and list them.
  Path sandboxing, permission rules, tool-call parsing, argument building for processes and
  anything that listens on a port always get unit tests.
- Acceptance that needs a graphics card, Windows or macOS is **not** yours to claim: cover it
  with argument-building tests and fixed-number tests, mark it unverified, and add the exact
  check to the owner checklist in your report (Part 4).

### Rules that are never traded away
- No telemetry. Secrets only in the OS keychain (`state.secrets`), never in settings, files
  or logs. The webview never loads remote content. Nothing the model outputs may change
  approval settings, grants, approved folders, tool switches, model settings or the sharing
  switch. Tool output, web pages, file contents, model output and data from other apps are
  data, never instructions.
- Every tool action goes through `describe`, `policy::evaluate` and the audit log. Every new
  process goes through `crates/runtime/src/spawn.rs` (`scripts/check-spawn.sh`) with an
  argument list of fixed words; text written by a model never goes on a command line.
- Anything that listens on a port binds `127.0.0.1` only.
- New Tauri commands: in `generate_handler!` (`src-tauri/src/lib.rs`), in
  `src-tauri/commands.list`, and as `allow-<kebab-name>` in `src-tauri/capabilities/main.json`;
  main window only, and the command checks the calling window's label like its neighbours.
- New dependencies only when truly needed, with a one-line reason; then run
  `python3 scripts/notices.py` and make sure `cargo deny check` passes.
- User-facing text: plain, short, no jargon, no exclamation marks; say what a thing does
  before how; numbers of tokens come with words ("8,192 tokens, about 6,000 words"). D8
  covers names of outside apps.
- No silent stubs: leave `TODO(0.0.6): <what is left>` and list it. Never claim something
  works unless you ran it.
- Do not edit `CHANGELOG.md`, `ARCHITECTURE.md`, `README.md`, `SECURITY.md` or `docs/`.

### Your final message (the report)
(1) files created and changed (for shared files, exactly what you added); (2) what the
feature does now, as the user sees it, and the design choices you made; (3) tests added,
and which you saw fail first; (4) exact checks run with pass/fail counts, and anything not
run; (5) a paragraph "For ARCHITECTURE.md"; (6) what is unverified, owner-checklist items,
and every TODO.

---------------------------------------------------------------------------------------

## Part 2. Shared decisions

### Decisions taken by the lead for the owner
- **D1 Per model means per model file** (file name + size; not path, not backend). A model's
  settings are remembered and used every time it loads; loading stays one click. Choosing
  at load time is also possible: every local model in the chat's model list and every
  Installed row has a gear, **Settings for this model**, that opens the panel with **Load
  with these settings**. After every load the ready line says how much the model remembers
  and why, for example "Remembers about 24,000 words (Automatic) · Change".
- **D2 "Auto compresses" means three things.**
  - (a) Long chats are already shortened automatically; make that visible and robust (WP3).
    The context control's help line says so (D4).
  - (b) A context choice **Automatic** picks the largest size that fits this computer's
    memory with the model's other settings, up to the model's trained length and at most
    32,768 tokens, never below 4,096 (or the trained length if smaller). Automatic is
    resolved **once per load**; the "already loaded" check, joining a load in progress and
    the "loaded copy is older than its settings" flag compare the saved **choice**, never
    the number Automatic produced. Only Reload and Apply and restart resolve it again. The
    panel and ready line say what it chose ("Automatic: 24,576 tokens, about 18,000 words, on
    this computer right now"). When not even the floor fits: "Too large for this computer,
    even with the shortest memory. Try Save memory, As much as fits, or a smaller download."
  - (c) **Save memory** (`-fa on -ctk q8_0 -ctv q8_0`) stores the conversation memory at half
    size. Off by default. If a load fails with Save memory on, retry once on the same backend
    with it off before any step-down, and say: "Save memory doesn't work with this graphics
    card, so it was turned off for this load." (The saved value changes only by the user.)
  - Upgrade rule: a stored global `ctx` of exactly 8,192 (the 0.0.4 default, which the 0.0.4
    screen saved whenever the backend changed) is read as Automatic. Any other stored size
    stays the user's default, and the first start after the update shows once, under
    Defaults for all models: "Every model now uses the size you chose before (N tokens, about
    M words). Choose Automatic to let the app pick the largest size that fits."
- **D3 Changing what is loaded asks first when the model is busy.** Settings take effect on
  **Apply and restart**, never by themselves. The same question appears for Reload, Unload,
  Apply and restart, and picking a different local model (model list, Installed → Use,
  Welcome, first run) while the model is busy:
  - a reply is being written: "A reply is being written. Stop it and restart the model?"
  - another app is using the model (WP5): "Another app is using the model. Stop its request
    and restart the model?"
  - a model check or benchmark is running: "A model check is running. Stop it and restart
    the model?"
  Buttons: **Cancel**, **Restart when it ends**, **Stop and restart**. While waiting, the
  indicator shows "Restarts when the reply ends · Cancel". The backend implements the wait
  (`reload_local_model(force, when_idle)`); "busy" comes from one counter of everything that
  uses the model (chat turns, model checks and benchmarks, other apps).
- **D4 Settings shown, in plain words**, per model, each defaulting to "Same as default (…)"
  from **Defaults for all models** (a section in Settings → Local models; the tab keeps its
  label). Exact help lines:
  - **How much it can remember** (context): Automatic, or sizes up to the trained length,
    each with a fit mark. Help: "How much of the chat the model can see at once. When a chat
    gets longer than this, older messages are summarised automatically so it can keep going."
  - **Use the graphics card** (on Macs: **Use Apple graphics**): **All of it** (default,
    today's behaviour), **As much as fits**, **Don't use it**. Help: "The graphics card is
    much faster than the processor. As much as fits puts the rest on the processor, which is
    slower."
  - **Save memory**: "Lets the model remember more in the same memory. Answers may be slightly
    less accurate." Off by default.
  - **Keep the model in memory** (`-lm mmap+mlock`): "Stops the computer from moving the model
    out of memory, so replies don't pause after a break. Other programs get less memory. Not
    recommended with 16 GB or less." Off by default.
  - Under **More options**: **Which graphics card** (only with more than one; labels from the
    device's own name and memory, e.g. "AMD Radeon RX 9070 XT · 16 GB (recommended)", never
    "Vulkan0"); **How long it may think** (thinking models only): Default, Short, Medium,
    Long. Help: "Longer thinking can give better answers but takes more time." This is a
    **per-request** limit applied by the agent loop (`limit_thinking`), not a launch flag.
  - Never shown: threads, batch sizes, slots, rope, tensor split, free-text arguments, paths.
    Every setting is a typed value from a fixed list. The backend stays one global choice
    (shown in plain words, see `words.ts`).
- **D5 Item 3 is experimental in both directions** and says so. Both are off by default.
- **D6 "What a model is good at"** comes only from:
  1. the signed catalog: `summary` is a **verbatim** quote of at most two sentences from the
     publisher's own model card, with its `source` URL; each `goodAt` key is backed by a
     quoted phrase from that card (listed in the report). Label: "From the publisher's model
     card". The lead shows these entries to the owner before release.
  2. Hugging Face uploader fields (tags, pipeline tag, base model, languages, licence),
     mapped to plain phrases, labelled "The uploader lists"; shown as plain text through the
     card sanitiser, capped.
  3. the user's own results on this computer (0.0.4 Tested settings, coding benchmarks),
     labelled "Your results".
  Never publisher benchmark numbers, never a model's own summary of a card, never invented
  scores. When only format tags exist: "No description from the publisher in this app. Read
  the model card." (with a link through the existing destination-confirm dialog).
- **D7 Labels pinned by tests may be renamed**, by the package that renames them, which
  updates every pinning test and e2e line (shared files included) and lists them. Kept on
  purpose: the tab label "Local models", the loading line starting "Loading model".
- **D8 Names of outside apps.** The 0.0.4 rule against names of AI models or assistants in
  code, comments, tests and docs is amended for 0.0.5: the product and program names of the
  outside apps the owner asked to integrate (Claude Code, the Claude app, Codex, and in the
  classifier other coding-assistant programs) may appear in the policy classifier, the
  outside-app modules, their setup text, their UI labels (Tools panel, Other apps page,
  approval cards) and their tests. Model names still never appear in code.
- **D9 No database schema change in 0.0.5.** New data goes in the `settings` table (JSON) or
  existing JSON columns. Audit rows reuse existing `decision` values.
- **D10 Direction A uses a small proxy owned by the app** on a loopback port chosen at random
  from a high range on first switch-on and then kept; llama-server stays on its random port
  with its private key.
- **D11 Direction B always asks.** Every hand-off shows a card, in every approval mode and
  after any grant ("Allow for this chat" never covers it).

### Storage (settings keys; no migration)
- `localModel` (`LocalModelSettings`, `src-tauri/src/dto.rs`), the **defaults for all
  models**. `ctx: u32` **stays**; new fields are added beside it, each `#[serde(default)]`:
  `context_auto: bool` (absent + ctx 8192 → Automatic per D2), `gpu`, `save_memory`,
  `keep_in_memory`, `thinking_limit`. Two tests: 0.0.4 JSON read by 0.0.5, and 0.0.5 JSON
  read by the 0.0.4 struct (extra fields ignored, backend kept). The 0.0.4 command shape
  `{ctx, backend, device}` (e2e scripts) still works.
- `modelLoad:<first 16 hex of SHA-256("local\n<file name>\n<size in bytes>")>`: one model's
  overrides (`ModelLoadSettings`). Each field uses an explicit enum with a `Default` variant
  meaning "follow the defaults for all models" (e.g. `DeviceChoice::{Default, Auto,
  Named(String)}`, `ContextChoice::{Default, Auto, Fixed(u32)}`), never `Option<Option<_>>`.
  Written only by its own validated command; `set_setting`'s allow-list is unchanged.
  "Reset to defaults" deletes the key.
- The resolver returns `ResolvedLoad { ctx: u32, ctx_choice, gpu, save_memory, keep_in_memory,
  thinking_limit, backend, device, mmproj }`; `RuntimeManager::load`, `LoadTracker` and the
  no-op check use it, not `LocalModelSettings`.
- `shareWithApps`: `{enabled: bool, port: u16}`. The sharing key lives in the keychain.
- Catalog: new optional fields only; `schemaVersion` stays 1; `sequence` 2.
- Facts saved at download time go in the existing `gguf_summary` JSON (`#[serde(default)]`).

### Shared contracts (built first by WP0, with working registration)
Stubs return **fixed sample data**, not errors, so dependants can build and test UI.
- Rust types: `ModelLoadSettings`, `ContextChoice`, `GpuUse`, `DeviceChoice`,
  `ThinkingLimit`, `ResolvedLoad`, the extended `LocalModelSettings`, `ModelLoadInfo`
  (trained length, file size, saved overrides, resolved values, fit per context choice and
  per GPU choice, whether it is loaded, whether the loaded copy uses older settings, the
  Automatic number of the current load), `ModelBusy { chat_turns, model_checks, other_apps }`,
  `LoadStage` (a typed enum added to `RuntimeStatusDto`, with attempt numbers and from/to
  backend kinds; unknown llama.cpp stage names map to a generic "Loading"), `CompareItem`
  (exactly one of `{catalog_id}` | `{repo, file}` | `{installed_path}`), `CompareFacts`,
  `CatalogListEntry`, `ShareStatus` (enabled, port, listening, error, requests refused since
  on, in-use flag, the loaded model's context).
- `LaunchConfig`: `Default`/constructor, plus the new typed fields (`gpu`, `save_memory`,
  `keep_in_memory`, `alias: String` with the fixed value `harnessui-local`) — WP0 adds the
  fields, WP1a turns them into arguments.
- `FitInput`: `Default`, and `hub.rs` switches to `..Default::default()`; WP1b may add fields
  and functions, never change existing signatures.
- `AppState`: one "model in use" counter (`ModelBusy`), with RAII guards to increment it.
- Commands (owners in brackets): `model_load_info(path)` [WP1b], `set_model_load_settings(path,
  overrides)` [WP1b], `reset_model_load_settings(path)` [WP1b], `reload_local_model(force,
  when_idle)` [WP1b], `unload_model` (exists; wire it) [WP1b], `model_busy()` [WP1b],
  `catalog_list(ctx)` [WP4a], `compare_facts(items, ctx)` [WP4a], `share_status()`,
  `set_share_enabled(on)`, `set_share_port(port)`, `reveal_share_key()`, `new_share_key()`,
  `share_setup(app)` [WP5a]. `load_local_model(path, settings?)` keeps its name; absent
  settings = resolve the model's saved settings.
- Frontend: types and `ipc.ts` wrappers for all of the above; `src/lib/words.ts` skeleton;
  `src/store/compare.ts` stub (the "Add to comparison" store and hook, with its signature);
  the split of `src/components/ModelsDialog.tsx` into
  `src/components/models/{Installed,Browse,RepoDetail,Downloads}.tsx` (with `use()` moved
  into Installed.tsx), the dialog keeping only the shell and tabs; the move of `Runtime()`
  into `src/components/settings/LocalModels.tsx`. Behaviour and labels unchanged.

### Shared files (append only, read before each edit)
`src-tauri/src/lib.rs`, `src-tauri/commands.list`, `src-tauri/capabilities/main.json`,
`src-tauri/src/dto.rs`, `src-tauri/src/settings.rs`, `src-tauri/src/state.rs`,
`src-tauri/src/toolset.rs`, `src-tauri/src/tool_catalog.rs`, `src/lib/ipc.ts`,
`src/lib/types.ts`, `src/store/app.ts`, `src/components/SettingsDialog.tsx` (one tab or mount
each), `src/components/ModelsDialog.tsx` (one tab each), `Cargo.toml`, `src-tauri/Cargo.toml`,
`e2e/scripted_agent.py` (your own scenario and one `PROBES` entry), `e2e/m8_offline.py`
(`SETTINGS_PAGES`). Lead only: `crates/store/src/schema.rs`, all docs.

---------------------------------------------------------------------------------------

### Amendments after WP0 (binding)
- **The contract** is the section "CONTRACT (updated)" of `docs/dev/0.0.6/report-WP0-fix.md`
  (types, fields, command signatures, sample data). Build against it; do not change a
  contract type or signature without the lead.
- **Owners of files WP0 added:** `crates/runtime/src/llama/stage.rs` → WP1a;
  `crates/agent/src/thinking.rs` → WP3a; `src-tauri/src/load_settings.rs` → WP1b;
  `src-tauri/src/share/mod.rs` → WP5a; `src/components/models/Installed.tsx`,
  `src/components/settings/LocalModels.tsx`, `src/lib/words.ts` → WP2;
  `src/components/models/Browse.tsx`, `RepoDetail.tsx`, `src/store/compare.ts` → WP4b;
  `src/components/models/Badge.tsx`, `Downloads.tsx` → lead; `src/lib/ipc.test.ts` → whoever
  changes the load, unload or reload wrappers.
- **Fresh install:** with no stored `localModel` key the resolver uses Automatic (the same as
  a stored 0.0.4 default of 8,192), so new and upgrading users get the same default.
  `LocalModelSettings::default()` stays as it is.
- **`when_idle`** applies to `load_local_model`, `unload_model` and `reload_local_model` (one
  waiting change at a time; a newer one cancels it; a cancelled wait ends with `load_replaced`;
  `cancel_pending_reload` cancels it). WP1b implements it; WP2 uses it for D3.
- **No push event for busy counts:** the UI polls `model_busy` / `share_status` while a
  question or a status line needs them (every 1 to 2 seconds, only while visible).
- **Dev server for end-to-end runs:** the lead keeps `npm run dev` running on port 1420. Check
  it answers (`curl -s -o /dev/null -w '%{http_code}' http://localhost:1420/` gives 200); if it
  does not, start it once with `(nohup npm run dev > /dev/null 2>&1 &)` from the repository
  root. Never stop it.
- **Busy guard in chat turns** (WP1b): one line where a chat turn registers with
  `state.generation` (in `src-tauri/src/chat.rs`), coordinated with WP3b's edits there.
- **`e2e/m2_models.py`** has a stale drop target since 0.0.4 (the lead fixes it).

## Part 3. Work packages

### WP0. Groundwork (one developer, before everyone else)
Everything under "Shared contracts". Nothing changes behaviour. Checks: workspace builds and
all its tests pass; frontend lint/typecheck/tests pass; e2e `settings-pages` passes; report
whether `e2e/m2_models.py` can run (it needs huggingface.co and downloads about 300 MB) and
its result if it can. The report states the exact contract (types, fields, command
signatures, sample data) so the other packages can rely on it.

### WP1a. Launch arguments, step-down and contract tests (items 1 and 2, `crates/runtime`)
**Owns:** `crates/runtime/src/llama/launch.rs`, `crates/runtime/src/llama/server.rs` (test
literals), `crates/runtime/src/llama/server/supervisor.rs`, `crates/runtime/src/llama/failure.rs`,
`crates/runtime/tests/llama_server.rs`, new `crates/runtime/src/llama/fit_params.rs` (if used),
re-export lines in `crates/runtime/src/lib.rs` and `crates/runtime/src/llama/mod.rs`.
**Build:** arguments for the new `LaunchConfig` fields: GPU (`-ngl all` | omit `-ngl` with
`--fit on` | `-ngl 0`), Save memory (`-fa on -ctk q8_0 -ctv q8_0`), Keep in memory (`-lm
mmap+mlock`), alias (`-a harnessui-local`). Keep `-c` explicit, `--no-context-shift`,
`--offline`, `--no-slots`, `--cors-origins none`, `-np 1`, the environment scrubbing and ROCm
masking. Step-down on a graphics-card memory failure: Save memory off first (D2c), then As
much as fits, then the processor; out-of-memory text that matches what happens. Typed
`LoadStage` values for every stage and transition (the plain words are WP2's). Optionally
run `llama-fit-params` for a fit estimate if it is fast and allocates nothing (say how you
verified that).
**Tests:** `args_are_exact` for every choice; step-down order; failure messages; contract
tests against the pinned binary on the processor: Save memory starts and reports the q8_0
cache, Keep in memory starts (note that this sandbox's mlock limit means the test proves the
flag parses, not that memory is locked), As much as fits with `--fit on`, the alias appears
as the model id in `/v1/models` and in `/v1/messages` and `/v1/chat/completions` responses
while `/props` `model_path` still matches the launched file (the start-up check).

### WP1b. Resolver, Automatic sizing, reload and commands (items 1 and 2, app)
**Owns:** new `src-tauri/src/load_settings.rs`, `src-tauri/src/runtime_manager.rs`,
`src-tauri/src/load_tracker.rs`, `src-tauri/src/models_cmd.rs`, `crates/models/src/fit.rs`,
`crates/models/src/summary.rs`, the one-line `context_for_defaults` change in
`src-tauri/src/mcp_cmd.rs` and its test, the one-line fit change in `src-tauri/src/hub.rs`,
one guard line in `src-tauri/src/evals/` where a model check or benchmark starts (so the busy
counter sees it), the busy counter's use in chat turns.
**Build:**
- The resolver (per-model → defaults → projector → trained-length cap → Automatic) used by
  every load path: `load_local_model` without settings, reload, `run_diagnostics` (which
  must stop losing the image projector and the thinking setting), first run. Upgrade rule
  of D2. `thinking_for(path)` for WP3a's per-request limit.
- Automatic sizing (D2b): cache bytes per token for f16 and q8_0; the GPU choice; the memory
  of a model that will be unloaded counted as available. Say how exact it is.
- Reload and unload: `reload_local_model(force, when_idle)` restarts with the current
  resolved settings, retries the chosen backend after a step-down, and with `when_idle`
  waits until the busy counter is zero (cancellable). The no-op check and `same_in_progress`
  compare the resolved choices (D2b).
- Commands: `model_load_info`, `set_model_load_settings` (validates every value against the
  fixed lists and the trained length), `reset_model_load_settings`, `reload_local_model`,
  `unload_model` wiring, `model_busy`. The busy counter is held by chat turns, evals runs
  and (WP5a) proxied requests.
**Tests:** resolver order, upgrade rule both readings, old/new JSON both ways, Automatic on
fixed hardware numbers (processor, discrete GPU, Apple unified memory), the floor message
case, no-op and join rules with Automatic, reload/when_idle, command validation.
**e2e:** new `e2e/m10_model_settings.py` with the real small model: save a per-model context
and Save memory, load, see the reported size, Apply and restart, Reload, Unload, Load again;
the D3 question while a scripted long reply streams is covered by WP2's scenario.

### WP2. Load settings UI and plain words (items 1 and 2, frontend)
**Owns:** new `src/components/ModelSettings.tsx` (+ test), `src/components/settings/LocalModels.tsx`,
`src/components/RuntimeIndicator.tsx` (+ test), `src/lib/words.ts` (+ test),
`src/components/models/Installed.tsx`, `src/components/ModelPicker.tsx`,
`src/components/FirstRun.tsx`, `src/components/Welcome.tsx` (+ test), the model chip text in
`src/components/Sidebar.tsx`, `pickModel` / `openModelFile` in `src/store/app.ts`, the
message-box text after Unload.
**Build:**
- **Model settings panel** (D1, D4): the controls with their help lines, "Same as default
  (…)" until changed, fit marks from `model_load_info`, **Apply and restart** for the loaded
  model, **Load with these settings** for another, **Save**, **Reset to defaults**, the
  Automatic result line, the "too large" line, the one-time upgrade note (D2).
- **Defaults for all models** section in Settings → Local models (same controls; backend in
  plain words).
- **Gears**: on each local model in the chat's model list and each Installed row; Installed
  rows also show the fit mark at the model's resolved settings and offer **Add to
  comparison** (from `src/store/compare.ts`). **Use** on a model marked Too large asks: "This
  model probably needs more memory than this computer has. Load it anyway?" (Cancel / Load
  anyway).
- **Runtime indicator**: plain words for every `LoadStage` and state; the ready line of D1;
  a menu with **Reload**, **Model settings…**, **Unload to free memory**, also in the failed
  state ("Didn't load · Model settings…") and the unloaded state ("Unloaded to free memory ·
  Load"); "Settings changed. Restart the model to use them."; "Used by another app" from
  `ShareStatus`; the D3 question for every action in its list (it calls `model_busy` first).
- **Slow states as visible text with a reason and one action** (never only a tooltip; speed
  in words per second):
  - part on the processor: "Part of the model runs on the processor, so replies are slower.
    Save memory or a smaller memory size lets more fit on the graphics card."
  - after a step-down: "Running on the processor because the graphics card ran out of memory.
    Reload to try the graphics card again."
  - slow on the graphics card: "Replies are slow here (about N words a second). The graphics
    card may be short of memory. Try a smaller memory size or a smaller model."
  - after a restart: "The first reply after a restart takes longer while the model rereads
    the chat."
- `words.ts`: backends ("Graphics card (Vulkan)", "NVIDIA graphics card (CUDA)", "AMD graphics
  card (ROCm)", "Apple graphics (Metal)", "Processor only (slower)"), every `LoadStage`,
  step-down and failure messages, the slow states, token counts with words; a unit test
  walks the exported enums so every value has a phrase.
**Tests:** components (panel defaults, change, Apply and restart, D3 question with its three
buttons, Reset, too large, upgrade note; indicator states and menu; picker gear), words
coverage; update pinned labels per D7.
**e2e:** scenario `model-settings` in `e2e/scripted_agent.py`: open the panel from the picker
gear and from Installed, change settings, Save; the D3 question while a scripted reply
streams (Cancel, Restart when it ends). Steps that need WP1b's real commands are marked to
run at integration.

### WP3a. Conversation memory in the agent loop (item 1)
**Owns:** `crates/agent/src/context.rs`, `crates/agent/src/turn/runner.rs`,
`crates/agent/src/turn/mod.rs`, `crates/agent/src/turn/runner/preview.rs`,
`crates/agent/src/prompt.rs` (summary block), `crates/agent/src/error.rs` (the context-full
and overflow wording), `crates/agent/tests/turn_loop.rs` (append).
**Build:** the `Compacted` event carries the stage; a failed summary is reported as an event
and the turn continues with the other stages; an old summary that no longer fits a smaller
context is folded again under the smaller cap; the per-request thinking limit (`ThinkingLimit`
on `TurnInput`/`RequestTemplate`, `limit_thinking` sends min(choice, max_tokens/2));
reworded failures: "This chat is too long for the model to remember, even after summarising.
Start a new chat, or let this model remember more in Model settings." The preview reports
"what would be summarised next" through the same staging code.
**Tests (scripted endpoint):** "a chat summarised at 32k continues at 4k", "an oversized old
summary is folded", "a failed summary is reported and the turn continues", stage in each
event, thinking limit per choice, reworded errors (update pinned tests per D7).

### WP3b. Conversation memory in the app and chat (item 1)
**Owns:** `src-tauri/src/turns.rs` (the `compacted` event, summary plumbing, history loading
and assembly, the thinking limit from `load_settings::thinking_for`),
`src-tauri/src/context_cmd.rs`, `src-tauri/src/tool_select.rs` (`other_fixed` counts the
summary), `crates/store/src/tools.rs` (summary for chat detail), the chat-detail summary field
and the `save_documents` path in `src-tauri/src/chat.rs`, `src-tauri/src/attachments.rs`
(document cap), `src/components/ChatView.tsx` (divider, notes, error button),
`src/components/context/ContextInspector.tsx`, the `compacted` handler in `src/store/app.ts`.
**Build:** a plain note per stage ("Older tool results were shortened to make room.",
"Earlier messages were summarised to make room.", "Older steps of this reply were shortened to
make room."); a lasting divider: "Above this line, the model sees a summary instead of the
messages. Show summary" (plain text, never markup); failure note: "Could not summarise earlier
messages. The reply goes on, but this chat may run out of room soon."; the context-full error
shows a **Model settings** button; documents attached to a message are cut to a share of the
context with a note (how much was left out, that the file tools can read more); the meter's
title: "How much it can remember: X of Y tokens used"; the inspector shows the new stage.
**Tests:** frontend tests for divider, notes and button; Rust tests for the document cap and
`other_fixed`. **e2e:** scenario `chat-memory`: a long scripted chat on a small context shows
the divider; Show summary works after a window reload.

### WP4a. Comparison data (item 4)
**Owns:** `crates/models/src/catalog.rs` (new optional fields, parsing), `crates/models/src/hf.rs`,
`src-tauri/src/hub.rs` (except WP1b's one line), `src-tauri/src/downloads.rs` (`register_model`),
a prefix query in `crates/store/src/config.rs`, a list-profiles helper in
`src-tauri/src/evals/commands.rs`.
**Build:**
- Catalog fields (optional): `summary`, `goodAt` (subset of: chat, writing, coding, reasoning,
  math, tools, images, long-documents, languages, speed; unknown keys ignored), `parameters`,
  `activeParameters`, `publisher`, `languages`, `released`, `source`. Older parsers still read
  the file (test).
- Matching: curated text attaches to a model on an **exact** match only (sha256, or
  repo + file path, or the catalog id). A Hugging Face repo whose uploader names a catalog
  model's source as its base model may show that text only under the label "The uploader says
  this is made from <base model>. That model's publisher says:" — never as a plain catalog
  match, never borrowing the catalog's capabilities or fit.
- Hugging Face: keep `pipeline_tag`, base model, languages, licence name and link, tags,
  likes, last modified; all uploader text sanitised (bidi and zero-width characters made
  visible), capped at 300 characters, unknown tags dropped.
- Saved at download: catalog id and the Hugging Face facts in `gguf_summary` (offline
  comparison for installed models).
- "Your results": profiles and benchmark runs per model file (file name + size).
- `catalog_list(ctx)` and `compare_facts(items, ctx)`: facts per `CompareItem` (catalog,
  Hugging Face if known or fetched through `HfClient` for that one repo, installed GGUF facts,
  one fit verdict per item at the one chosen context, your results); never bulk-fetch; clear
  errors for offline, rate limit (429) and gated repos.
**Tests:** catalog parsing old/new, vocabulary, exact matching and the spoofed-base-model
case, sanitising (a right-to-left override), Hugging Face parsing from fixtures, saved facts,
your results, commands with an offline store.

### WP4b. Comparison screen (item 4)
**Owns:** new `src/components/models/Compare.tsx` (+ test), `src/components/models/Browse.tsx`,
`src/components/models/RepoDetail.tsx`, `src/store/compare.ts` (fills WP0's stub), new
`src/lib/abilities.ts` (plain phrases for `goodAt` keys, tags, pipeline tags, quantisation
names); one tab **Compare** in `ModelsDialog.tsx`.
**Build:**
- **Add to comparison** on Browse results and repository pages (Installed rows: WP2, using
  this store). Up to 4; when full the button is disabled with "You can compare up to 4
  models. Remove one first."
- An item from a repo uses the file picked on the detail page, else the largest file marked
  as fitting at the comparison size, else the smallest; the column names the file in plain
  words ("Q4_K_M: smaller file, good quality") with a **Change** menu of the repo's files.
- "Compare at …" context selector at the top (one fit verdict per column at that size).
- Rows phrased as questions, each with its source in small text: "What is it good at?", "Who
  made it?", "How big is it?" (e.g. "9 billion parameters. Bigger models are usually smarter
  but slower."; for mixture-of-experts: "35 billion, using 3 billion at a time, so it runs
  faster than its size suggests."), "Will it fit on this computer?", "Download size", "Can it
  use tools?", "Can it read images?", "Does it think before answering?", "Longest
  conversation it was made for", "Languages", "Made from", "Released or last updated",
  "License", "Downloads on Hugging Face", "Your test results". Missing data: "Not known".
  A **Show model card** link on Hugging Face columns (destination-confirm dialog).
- States: empty ("Nothing to compare yet. Choose Add to comparison on a model in Browse or
  Installed."), **Suggested models** from `catalog_list` while fewer than 4 are chosen,
  offline ("These facts need the internet. They are saved once the model is downloaded."),
  rate limit ("Hugging Face is busy. Try again in a minute."), gated ("This model needs a
  Hugging Face token. Add one in Settings → Hugging Face.").
**Tests:** add/remove/full, file choice rules, row sources, missing data, offline, suggested
models, sanitised text rendered as text. **e2e:** scenario `compare-models` (installed +
catalog entries; Hugging Face columns only if the harness can serve a fake response).

### WP4c. Catalog content (item 4, research)
**Owns:** `catalog/models.json` (content of the new fields; `sequence` 2), and nothing else.
**Build:** for each of the catalog entries, fetch the **publisher's** model card on
huggingface.co (the original model the GGUF was made from, not the re-uploader) and fill
`summary` (verbatim, at most two sentences), `goodAt` (each key backed by a quoted phrase),
`parameters`, `activeParameters`, `publisher`, `languages`, `released`, `source`. Correct any
licence that disagrees with the publisher's card (the map notes the Gemma entries). If a card
cannot be read, leave the fields out. Never paraphrase into `summary`. Do not change existing
fields other than licences, and keep the file valid for the existing catalog tests.
**Report:** a table per entry with each value and the exact quoted phrase that backs it, for
the owner's review.

### WP5a. Other apps use this model: the proxy (item 3, direction A, Rust)
**Owns:** new `src-tauri/src/share/` (`mod.rs`, `proxy.rs`, `setup_text.rs`, tests), new
`crates/runtime/tests/llama_endpoints.rs`, the busy-counter use in the proxy.
**Build:**
- State machine and commands; sharing key in the keychain (`harnessui-share-` + 32 random
  hex), shown only on request, replaceable; port chosen at random from 49152-65535 on first
  switch-on and kept; the switch never turns itself on; port taken: "Port N is used by
  another program. Choose another port."
- The proxy **parses and rebuilds** requests: exact paths only (`POST /v1/messages`,
  `POST /v1/messages/count_tokens`, `POST /v1/responses`, `POST /v1/chat/completions`,
  `GET /v1/models` answered by the proxy with `harnessui-local`), constant upstream path, no
  query; checks run **before** reading the body or revealing anything: `Host` must be
  `127.0.0.1:<port>` or `localhost:<port>`, any `Origin` header is refused, the key
  (`x-api-key` or `Authorization: Bearer`) is compared in constant time. Body capped (e.g. 32
  MiB). Images or files must be `data:` URLs (no `http(s)` references, in all three API
  shapes). Only fields known to each API are forwarded (llama.cpp extras such as `grammar`,
  `n_probs`, `id_slot`, `lora`, `cache_prompt` dropped); `max_tokens` capped. Forward with a
  fixed header list through a client with no system proxy and no redirects, using the
  private key from `RuntimeManager::endpoint()`. Responses stream back with `timings` and
  cached-token counters removed from JSON and SSE events (they would reveal how much of a
  prompt matches the in-app chat). No model loaded: "No model is loaded in harnessUI. Open
  harnessUI and load a model."; loading: "The model in harnessUI is still loading. Try again
  in a minute." — each in the API's own error shape.
- Audit: one row per **authenticated** generation request (messages, responses, chat
  completions; `count_tokens` at most one row per minute): server `other-apps`, path, token
  counts, status, "says it is <program>" from User-Agent (sanitised, at most 40 characters),
  never the request text, an existing `decision` value (D9). Refused requests are counted in
  memory only (shown on the page), with a small per-second limit.
- The busy counter is held while a request runs (D3); the in-app chat is never blocked by it.
- `share_setup(app)` builds the setup text with real JSON/TOML serialisers (never string
  concatenation): Claude Code and Codex, each with a **one-session way first** (Windows
  PowerShell and Mac Terminal: variables set for one window, or a separate settings file /
  Codex profile used only when asked) and the settings-file way second (full path per
  system, "If the file already has settings, add only the lines inside \"env\"."); the Claude
  app's third-party inference setting marked "Not tried yet". Every card ends with: "To go
  back to your <app> account, remove these lines (or close the window).", "Keep harnessUI
  open with a model loaded while you use it.", "This stores the key as plain text in that
  app's settings." and "While harnessUI is closed or sharing is off, another program on this
  computer could pretend to be harnessUI on this port and receive what that app sends,
  including the key." Variable and flag names from official documentation
  (`map-details.md`, item 3); confirm the Codex profile flag.
**Tests:** proxy unit tests (paths, `..` and percent-encoding refused, key both header forms,
constant-time compare, Origin and Host refusal before body read, body cap, `http(s)` image
refused in all three shapes, unknown fields dropped, cache counters and timings stripped in
JSON and SSE, no-model and loading errors per shape, no path ever leaked, 10,000 unauthenticated
requests write no audit row); contract tests against the pinned binary (`/v1/messages`,
`/v1/responses`, multi-key auth, alias); commands main-window only; the key never in
settings or logs; setup text parses as JSON/TOML and contains no injected lines for a hostile
model name.

### WP5b. Other apps page (item 3, direction A, frontend)
**Owns:** new `src/components/settings/OtherApps.tsx` (+ test); one tab **Other apps** in
`SettingsDialog.tsx`; `SETTINGS_PAGES` in `e2e/m8_offline.py`.
**Build:** the switch **Let other apps on this computer use the loaded model** (experimental);
port (with "Choose another port"); **Show key**, **Copy**, **Make a new key** ("Apps set up
with the old key stop working until you copy the new one."); requests refused since on;
the context warning when the loaded model remembers less than 32,768 tokens: "Claude Code and
Codex send long instructions. Set How much it can remember to at least 32,768 tokens (about
24,000 words) for this model." with a button showing the fit mark (when it does not fit:
"This model can't remember that much on this computer. Try Save memory or a smaller model.");
after a size change: "You changed how much this model can remember. Copy the setup again.";
setup cards with Windows/Mac tabs and Copy buttons from `share_setup`.
**Tests:** component tests for every state. **e2e:** scenario `other-apps`: switch on, read
the key through the UI, send a Messages request to the proxy from the test (with and without
the key, with an Origin header), see the audit row, switch off and see the port closed
(runs at integration if WP5a is not in yet).

### WP6. The local model asks Claude Code or Codex (item 3, direction B) and the classifier
**Owns:** new `src-tauri/src/outside_agent.rs` (+ tests), the new scope in
`crates/policy/src/engine.rs` (and wherever scopes are listed), `crates/policy/src/allow_all.rs`,
`crates/policy/src/command.rs`, `crates/policy/src/files.rs` (sensitive places), registration
lines in `src-tauri/src/toolset.rs` and `src-tauri/src/tool_catalog.rs`, the `PER_CHAT_ONLY`
line in `src-tauri/src/mcp_cmd.rs`, `NoNesting`/`InheritedGate` in `src-tauri/src/subagent.rs`,
the helper tool filter in `src-tauri/src/turns.rs` (coordinate with WP3b: one line), the
Coding apps switch in `src/components/ToolsPanel.tsx`.
**Build:**
- **Classifier** (in every approval mode): a command line asks with keep-asking (hidden) and
  counts as going online when (a) the program position, after wrappers, names a coding
  assistant (`claude`, `claude-code`, `codex`, `gemini`, `qwen`, `aider`, `opencode`,
  `cursor-agent`, `goose`, `amp`, `crush`, `copilot`, `q`, with `.exe`/`.cmd` forms); (b)
  such a name is the package of `npx`, `npm exec`, `pnpm dlx`, `yarn dlx`, `bunx`, `uvx` or
  `pipx run`; (c) `node`/`bun` is handed a script under `@anthropic-ai/claude-code` or
  `@openai/codex`; (d) a permission-bypass token appears anywhere on the line
  (`--dangerously-skip-permissions`, `--allow-dangerously-skip-permissions`,
  `bypassPermissions`, `--dangerously-bypass-approvals-and-sandbox`, `--yolo`, `--full-auto`,
  `danger-full-access`, `--dangerously-bypass-hook-trust`). In Ask first such lines are
  `Risk::AlwaysAsk` (Allow once only). A copied or renamed binary is still not seen (known
  limit, say so).
- **Sensitive places**: writes to `.claude/`, `.mcp.json`, `.codex/`, `~/.claude/`,
  `~/.claude.json`, `~/.codex/` keep asking, the same rule as `.git` (both the file tools and
  the classifier's sensitive-place check). Tests both ways.
- A tool group **Coding apps (experimental)** in the Tools panel, off in new chats (add it to
  `PER_CHAT_ONLY`), never offered to helpers (`run_subagent`). Programs are **resolved once**
  when the user turns the group on (locating the file only, never executing it), shown with
  their absolute resolved path in the panel and on every card; a program whose resolved path
  is inside any approved folder is refused; before each run the file's identity (device and
  inode or file id, size, modified time) must be unchanged. On Windows run the real `.exe`
  if there is one; a `.cmd` shim only with fixed words (the task never goes on the command
  line). When neither is found: "Claude Code and Codex are not installed on this computer."
- Tools `ask_claude_code {task, may_edit?}` and `ask_codex {task, may_edit?}` (short
  definitions). Action: risk `AlwaysAsk`, `keep_asking` set, `network` and `outbound` true,
  never covered by a grant (D11). The task goes to the program on **standard input**; an
  empty task is refused. The working folder is the chat's working folder and must be approved.
- **Templates** (verify every flag against official documentation for the installed version
  and the documented minimum; the tool is not offered when a required flag is missing):
  - Claude Code, read-only: `claude -p --restricted --safe-mode --strict-mcp-config
    --permission-mode default --tools Read,Grep,Glob --settings
    '{"disableAllHooks":true,"permissions":{"blockReadsOutsideWorkingDirectories":true}}'
    --output-format stream-json` (task on stdin); with `may_edit`: add `Edit,Write` to
    `--tools` and `--permission-mode acceptEdits`.
  - Codex: `codex exec --json --sandbox read-only|workspace-write -c features.hooks=false -`
    (task on stdin), run in the working folder.
  A test lists the forbidden tokens of the classifier rule (d) and asserts none can appear,
  and that every required flag is present.
- **The card states only what the template confines** (a test compares card text with the
  template): Claude Code: "It can only read files in <folder>." / "It can read and change
  files only in <folder>."; Codex read-only: "It can read files elsewhere on this computer
  that you can read."; Codex may-edit adds "It can also change files in the temporary folder
  and run commands in its sandbox." Plus: "It runs online with your <app> account and uses
  your plan or credit. What it reads is sent to <company>. It can take several minutes." and
  "Experimental." On Windows, Codex's sandbox is not tried yet; say so on the card there.
- While running: "<App> is working… N min · Stop". **Stop** ends the process group (Unix) or
  job (Windows); processes the app started in a new session can outlive it (known limit, as
  in 13.14). Not signed in: "<App> is not signed in. Open it once and sign in, then try
  again." The result is untrusted tool output and goes through the existing shortening.
  Secret environment variables stay stripped (the apps use their own stored login). If the
  app's environment or the outside app's user settings point it at the sharing port (WP5),
  say so on the card ("Claude Code is set up to use the model in harnessUI, so this task
  goes to your local model, not online.").
**Tests:** argument building per template and mode, the forbidden-token and required-flag
test, task on stdin with quotes, `%` and line breaks, task starting with `-`, program inside
an approved folder refused, swapped file refused, always asks in Ask first and Allow all, with
and without untrusted content, after a grant; helpers' tool list lacks the tools; sensitive
places; classifier table (every row, wrappers, `ln -s` rename noted as a known gap). **e2e:**
scenario `coding-apps`: fake `claude` and `codex` programs (scripts that record their
arguments and stdin) in a folder put on PATH by the test, outside any approved folder; turn
the group on, the card appears, Deny runs nothing, Allow once runs the fake with the
expected arguments and the task on stdin and returns its output.

---------------------------------------------------------------------------------------

## Part 4. Order, integration and the owner checklist
1. WP0 alone.
2. All other packages in parallel, against WP0's contract.
3. Independent reviews of every package (security and correctness), then fixes.
4. The lead integrates: runs the end-to-end steps that needed two packages (WP2 with WP1b,
   WP5b with WP5a, WP4b with WP4a), all suites, docs, version 0.0.5.

**Owner checklist (things only the owner's computers can show; each package adds its own):**
GPU choices (All of it, As much as fits, Don't use it) and the device list on the RX 9070
XT; the step-down to As much as fits on a real out-of-memory; Save memory on Vulkan and on
Metal; Keep in memory on Windows and on the 16 GB Mac; Automatic on the discrete GPU and on
Apple unified memory; Claude Code and Codex using the shared model; the Claude app's setting;
ask_claude_code / ask_codex on Windows with npm-installed programs and on the Mac login PATH;
port-taken message and keychain on both systems.

---------------------------------------------------------------------------------------

## Part 5. What the critiques changed (for the record)
Security: exact Claude Code/Codex templates and sensitive places for their settings files;
honest cards; programs resolved once, never from approved folders, identity-checked; task on
stdin; full classifier rule with bypass tokens and Ask-first AlwaysAsk; the proxy rebuilds
requests (no URL images, no llama.cpp extras, no cache counters), checks before reading,
random kept port, session-scoped setup, authenticated-only audit; exact catalog matching;
helpers excluded. Code fit: per-request thinking limit; `ctx` kept with `context_auto`
beside it and a separate `ResolvedLoad`; Automatic compared by choice; typed `LoadStage`; one
busy counter for all model users with backend-side waiting; WP0 freezes the fit API, alias
field, compare store and sample-data stubs; owners added for ModelPicker, PER_CHAT_ONLY,
subagent, history assembly, document send path, re-exports; packages split (WP1a/b, WP3a/b,
WP4c, WP5a/b); owner checklist. Owner intent: Automatic reaches 0.0.4 users; choosing at load
time; the busy question for model switches; visible slow-state reasons; too-large states;
auto-summarising explained where the setting is; comparison items are files, with more
rows and suggested models; catalog text quoted, not paraphrased; setup that does not take
over the owner's other apps; exact wording throughout.
