# WP0 report (0.0.5 groundwork)

The lead committed most of this work as `08702aa` ("groundwork in progress"). Two small edits are still uncommitted, both in `src-tauri/src/models_cmd.rs`: `set_local_model_settings` now drops `upgrade_note`, and the doc comment of `reload_local_model` now spells out its meaning. I changed no git state myself.

## (1) Files

**New:**
- Rust:
  - `crates/runtime/src/llama/stage.rs` (WP1a owns it from now)
  - `crates/agent/src/thinking.rs` (WP3a owns it from now)
  - `src-tauri/src/load_settings.rs` (WP1b)
  - `src-tauri/src/share/mod.rs` (WP5a)
- Models dialog files:
  - `src/components/models/Badge.tsx` (shared by Installed, Browse and RepoDetail; lead-owned, nobody needs to edit it)
  - `src/components/models/Installed.tsx` (WP2)
  - `src/components/models/Browse.tsx` and `src/components/models/RepoDetail.tsx` (WP4b)
  - `src/components/models/Downloads.tsx`
- Other frontend files:
  - `src/components/settings/LocalModels.tsx` (WP2)
  - `src/lib/words.ts` and `src/lib/words.test.ts` (WP2)
  - `src/store/compare.ts` and `src/store/compare.test.ts` (WP4b)

**Changed (not shared):**
- `crates/runtime/src/llama/launch.rs`: new fields, `Default`, constructor, tests.
- `crates/runtime/src/llama/server.rs`: the `ServerEvent::Stage` variant, its `use` line, the test literal.
- Test literals only:
  - `crates/runtime/src/llama/server/supervisor.rs`
  - `crates/runtime/tests/llama_server.rs`
- Re-exports and module lines:
  - `crates/runtime/src/lib.rs`
  - `crates/runtime/src/llama/mod.rs`
  - `crates/agent/src/lib.rs`
- `crates/models/src/fit.rs`: `Default` on `FitInput`.
- `src-tauri/src/hub.rs`:
  - `..Default::default()` with `#[allow(clippy::needless_update)]`, because every field is set today;
  - the comparison stubs and one test.
- `src-tauri/src/models_cmd.rs`: the stubs, and the optional `settings` argument of `load_local_model`.
- `src-tauri/src/runtime_manager.rs`:
  - the `LaunchConfig` literal now uses `LaunchConfig::new(..)` with `..`;
  - the status now carries the typed load stage, the list of step-downs, and handling of `ServerEvent::Stage`.
- `src-tauri/src/commands.rs`: `only_main_window()` and its test.
- `src/components/Welcome.test.tsx`: the fixture gains the 5 new status fields. No assertion changed.

**Shared files, exactly what I added:**
- `src-tauri/src/lib.rs`:
  - `mod load_settings;` and `mod share;`;
  - 14 handler lines after `get_bench_run`;
  - `model_busy: Default::default()` in `AppState`.
- `src-tauri/commands.list` and `src-tauri/capabilities/main.json`: the 14 command names and their `allow-` entries, appended at the end.
- `src-tauri/src/dto.rs`:
  - one `use` line;
  - 5 fields on `RuntimeStatusDto` and its `Default`;
  - doc and 6 fields on `LocalModelSettings` and its `Default`;
  - `ModelBusy`;
  - the comparison section;
  - a `tests` module.
- `src-tauri/src/state.rs`: the `model_busy` field, `BusyKind`, `BusyCounter`, `BusyGuard`, and 3 tests.
- `src/lib/types.ts`: 5 fields on `RuntimeStatus`, 6 on `LocalModelSettings`, and a 0.0.5 section at the end.
- `src/lib/ipc.ts`: imports, an optional `settings` on `loadLocalModel`, and 14 wrappers.
- `src/components/SettingsDialog.tsx`: `Runtime()` removed, `LocalModels` imported and mounted, and three imports that became unused removed.
- `src/components/ModelsDialog.tsx`: now only the dialog shell and its tabs.
- Untouched: `settings.rs`, `toolset.rs`, `tool_catalog.rs`, `app.ts`, both `Cargo.toml` files (no new dependencies), and the e2e scripts.

## (2) What the user sees, and design choices

Nothing changes on screen: the labels, the launch arguments and the behaviour are exactly as before.

Design choices:
- **Per-model choices are explicit enums with a `Default` variant.** The concrete values have no such variant: `GpuUse`, `bool`, `ThinkingLimit`.
- **Some types live in the crates that use them.** `GpuUse` and `LoadStage` are in the runtime crate and `ThinkingLimit` is in the agent crate. This way WP1a and WP3a never need app types, and `load_settings` simply re-exports them.
- **New transitions travel on `ServerEvent::Stage(LoadStage)`.** WP1a can send its new retries without touching `server.rs`.
- **I added fields the dependent packages need but the spec did not list:**
  - on `RuntimeStatusDto`: `adjustments`, `ctx_source`, `settings_changed`, `reload_waiting`;
  - on `LocalModelSettings`: the response-only `upgrade_note`;
  - the command `cancel_pending_reload` (for "Restarts when the reply ends · Cancel").
- **The stubs return made-up sample data.** No model names appear in it.
- **The comparison types are in `dto.rs`; the share types are in `share/mod.rs`.**
- **`CompareItem` refuses any shape other than exactly one of the three.** It does this through `try_from` on its JSON form, with unknown keys denied.

## (3) Tests added

**Rust (21):**
- launch: `defaults_and_constructor`, `new_fields_are_not_arguments_yet`, `old_json_reads_with_defaults`
- stage: `llama_stage_names_map_and_unknown_names_are_generic`, `serialised_shapes`
- thinking: `serialised_names`
- commands: `only_the_main_window_passes`
- dto:
  - `json_from_0_0_4_reads` (0.0.4 JSON read by 0.0.5)
  - `json_from_0_0_5_reads_in_0_0_4` (0.0.5 JSON read by the 0.0.4 struct; backend kept)
  - `a_compare_item_names_exactly_one_model`
  - `busy_snapshot_shape`
  - `runtime_status_defaults`
- load_settings: `the_key_is_file_name_and_size_only`, `choices_serialise_as_documented`, `default_means_the_defaults_for_all_models`, `the_sample_is_complete_and_serialises`
- share: `shapes`
- state: `busy_guards_count_while_they_live`, `a_panic_does_not_leave_a_count_behind`, `wait_idle_returns_when_the_last_guard_goes`
- hub: `compare_samples_echo_the_item_and_the_context`

**Frontend (5):** `words.test.ts` (3) and `compare.test.ts` (2).

**Seen failing first:** I wrote the tests together with the types, so none failed first on its own. I made one deliberate breakage to check a test: I let `CompareItem` accept a repository without a file, and `a_compare_item_names_exactly_one_model` failed. Then I put the code back.

## (4) Checks run

| Check | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo +1.99 clippy --workspace --all-targets -- -D warnings` | pass, 0 warnings |
| `cargo +1.99 test --workspace` | 1,726 passed, 0 failed, 6 ignored, in 40 test binaries. The ignored ones are 4 search tests that need the network, plus one each in two other crates. Before WP0 it was 1,705. Two full runs gave the same result; I saw no flaky test. |
| `npm run lint` | pass |
| `npm run typecheck` | pass |
| `npm test` | 38 files, 253 tests passed (248 before) |
| `npm run build` | pass (the warning about chunks over 500 kB was already there) |
| `python3 scripts/notices.py --check` | pass |
| `bash scripts/check-spawn.sh` | pass |
| e2e `settings-pages` | PASS, twice; the second run was after the final edit |

- **Dev server:** the e2e runner does not start its own dev server, although Part 1 says it does. I started `npm run dev` on port 1420 myself and stopped it afterwards.
- **Extra check:** with a scratch script, `/tmp/claude-0/-home-user-harnissUI/23ef2267-cb6f-5325-b082-a4b40b3ccb19/scratchpad/v005/probe_cmds.py`, I called each of the 14 new commands in the real app from the main window. All answered. A bad `CompareItem` and the override value `gpu: "half"` were refused, and the 0.0.4 shape `{ctx, backend, device}` still saved.
- **`e2e/m2_models.py`:** huggingface.co is reachable, so I ran it twice. It got through:
  - first run;
  - search;
  - the repository page (fit verdict green);
  - downloading Q8_0 and its projector, with the checksum checked;
  - the Installed badges;
  - **Use**, which now runs the moved `use()` in Installed.tsx: the model reached Ready with `vision` true.

  It then **FAILED** at "send an image from the composer" with `null is not an object (evaluating 'target.dispatchEvent')`. The script drops the image on `textarea.closest('.border-t')`, and the message box's outer element (`px-4 pt-1 pb-3`) has had no `border-t` class since the 0.0.4 visual refresh. This was broken before WP0 and has nothing to do with it. The lead should fix the drop target in m2.

## (5) For ARCHITECTURE.md

0.0.5 keeps its shared contracts in a few places:
- **Runtime crate:** `GpuUse`, `MODEL_ALIAS`, the new `LaunchConfig` fields and `LoadStage`, with `ServerEvent::Stage` for transitions that have no event of their own.
- **Agent crate:** `ThinkingLimit`, a limit applied per request, not a launch flag.
- **`src-tauri/src/load_settings.rs`:**
  - per-model overrides under `modelLoad:<16 hex of SHA-256("local\n<file>\n<size>")>`;
  - one enum per field, each with a `Default` variant;
  - `ResolvedLoad`, whose `ctx_choice` is what gets compared, never Automatic's number;
  - `ModelLoadInfo`.
- **`localModel`** keeps `ctx` and gains fields that read with defaults when absent.
- **`AppState.model_busy`:** one counter of everything that uses the model, raised by RAII guards and waited on through a `watch` channel.
- **Comparison types** in `dto.rs`, and **sharing types** in `share/`.

Every new command checks the calling window's label (`commands::only_main_window`).

## (6) Unverified items and TODOs

Nothing in WP0 needs a graphics card or the owner's computers, so there are no owner-checklist items.

The stubs are marked (grep `TODO(WP`):
- `src-tauri/src/models_cmd.rs`:
  - near line 40: `load_local_model` without settings uses the defaults for all models until the resolver exists;
  - near line 111: the load-settings command stubs.
- `src-tauri/src/load_settings.rs`, near line 334: `sample_info`.
- `src-tauri/src/hub.rs`, near line 1072: `catalog_list` and `compare_facts`.
- `src-tauri/src/share/mod.rs`, near line 107: all six share commands.
- `src/lib/words.ts`, near line 53: `loadStageWords` wording.

Items marked `#[allow(dead_code, reason = "0.0.5 contract: …")]`: `KEY_PREFIX`, `settings_key`, the `resolve` impls, `is_default`, `SETTINGS_KEY`, `ShareSettings`, `BusyKind`, `BusyCounter`, `slot` and `BusyGuard`. Owners remove the attribute once they use the item.

---

# CONTRACT

## Rust: runtime crate (`runtime::…`)

- `pub const MODEL_ALIAS: &str = "harnessui-local";`
- `pub enum GpuUse { All /*default*/, Fit, Off }`
  - Derives: Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize.
  - JSON: `"all" | "fit" | "off"`.
  - Meaning for WP1a: All is `-ngl all`; Fit leaves out `-ngl` and passes `--fit on`; Off is `-ngl 0`.
- **New `LaunchConfig` fields:**
  - `gpu: GpuUse` (`#[serde(default)]`)
  - `save_memory: bool` (`#[serde(default)]`)
  - `keep_in_memory: bool` (`#[serde(default)]`)
  - `alias: String` (default `MODEL_ALIAS`)
  - **None of them is read by `build_args` yet:** `gpu_layers` still decides `-ngl`.
  - `runtime_manager` sets `gpu = Off` on the CPU and `All` otherwise, the same as `gpu_layers`, so WP1a can switch `-ngl` over to `gpu` with no change to the app.
  - WP1a must rewrite the test `new_fields_are_not_arguments_yet`.
  - Debug now prints the new fields.
- **`impl Default for LaunchConfig`:** model empty, mmproj None, ctx 8192, device None, gpu_layers All, cache_ram_mib 512, reasoning_budget None, port 0, api_key "", hip None, gpu All, save_memory false, keep_in_memory false, alias "harnessui-local". Port 0 and an empty key are refused by `start`.
- **`LaunchConfig::new(model: PathBuf, ctx: u32, port: u16, api_key: String) -> Self`**: everything else from `Default`.
- **`pub enum LoadStage`** (`crates/runtime/src/llama/stage.rs`): `#[serde(tag="kind", rename_all="camelCase")]`. Variants:
  - `Starting`
  - `ReadingModel`
  - `ReadingImageReader`
  - `Loading`
  - `Restarting{attempt:u32,max:u32}`
  - `RetryingWithoutSaveMemory{backend:BackendKind}`
  - `RetryingWithLessGpu{backend:BackendKind}`
  - `SteppingDown{from:BackendKind,to:BackendKind,reason:LoadFailure}`

  Example: `{"kind":"steppingDown","from":"vulkan","to":"cpu","reason":{"code":"out_of_memory"}}`.

  `LoadStage::from_llama_stage(&str)` maps `text_model` to ReadingModel, `mmproj_model` to ReadingImageReader, and anything else (including `spec_model`) to Loading.
- **`ServerEvent::Stage(LoadStage)`**, for WP1a's retries:
  - The app sets `load_stage`.
  - For `RetryingWithoutSaveMemory`, `RetryingWithLessGpu` and `SteppingDown` it also pushes onto `adjustments`.
  - Do **not** also send `Stage(SteppingDown)` for a `SteppedDown` event: `SteppedDown` is already mapped, and the step-down would be pushed twice.

## Rust: agent and models

- `agent::ThinkingLimit { NoLimit /*default*/, Short, Medium, Long }`, JSON `"noLimit"|"short"|"medium"|"long"`. The token number behind each choice is WP3a's.
- `models::fit::FitInput` now derives `Default`. `hub.rs` builds it with `..Default::default()`. No new fields yet.

## Rust: `src-tauri/src/load_settings.rs`

All types are `rename_all = "camelCase"`. `pub use runtime::GpuUse; pub use agent::ThinkingLimit;`

**Keys:**
- `KEY_PREFIX = "modelLoad:"`
- `settings_key(file_name: &str, size: u64) -> String`, for example `settings_key("model.gguf", 4920000000) == "modelLoad:63f81cec99ade548"`.

**Per-field choices:**
- `ContextChoice { Default, Auto, Fixed(u32) }`, JSON `"default" | "auto" | {"fixed":8192}`.
- `ContextSize { Auto, Fixed(u32) }`, JSON `"auto" | {"fixed":n}`. This is the resolved choice.
- `GpuChoice { Default, All, Fit, Off }`; `.resolve(GpuUse) -> GpuUse`.
- `Toggle { Default, On, Off }`, JSON `"default"|"on"|"off"`; `.resolve(bool) -> bool`.
- `DeviceChoice { Default, Auto, Named(String) }`, JSON `"default"|"auto"|{"named":"Vulkan0"}`; `.resolve(Option<&str>) -> Option<String>`.
- `ThinkingChoice { Default, NoLimit, Short, Medium, Long }`; `.resolve(ThinkingLimit) -> ThinkingLimit`.

**`ModelLoadSettings`** (every field `#[serde(default)]`, derives Default; `.is_default()`):

| Field | Type |
|---|---|
| `context` | `ContextChoice` |
| `gpu` | `GpuChoice` |
| `save_memory` (`saveMemory`) | `Toggle` |
| `keep_in_memory` (`keepInMemory`) | `Toggle` |
| `device` | `DeviceChoice` |
| `thinking_limit` (`thinkingLimit`) | `ThinkingChoice` |

**`ContextSource { Automatic, ModelSetting, DefaultSetting, TrainedLimit }`**, JSON `"automatic"|"modelSetting"|"defaultSetting"|"trainedLimit"`.

**`ResolvedLoad`:**

| Field | Type |
|---|---|
| `ctx` | `u32` |
| `ctx_choice` (`ctxChoice`) | `ContextSize` |
| `ctx_source` (`ctxSource`) | `ContextSource` |
| `gpu` | `GpuUse` |
| `save_memory` | `bool` |
| `keep_in_memory` | `bool` |
| `thinking_limit` | `ThinkingLimit` |
| `backend` | `Option<String>` |
| `device` | `Option<String>` |
| `mmproj` | `Option<String>` |

**Options shown in the panel:**
- `ContextOption { choice: ContextSize, tokens: u32, fit: models::Verdict }`. Verdict JSON is `"green"|"yellow"|"red"`.
- `GpuOption { gpu: GpuUse, fit: Verdict }`.
- `DeviceOption { name, description, total_mib (totalMib), free_mib (freeMib), recommended: bool }`.

**`ModelLoadInfo`:**

| Field (JSON name) | Type |
|---|---|
| `path` | String |
| `file_name` (`fileName`) | String |
| `size_bytes` (`sizeBytes`) | u64 |
| `trained_ctx` (`trainedCtx`) | Option<u32> |
| `thinking` | bool |
| `vision` | bool |
| `overrides` | ModelLoadSettings |
| `defaults` | LocalModelSettings |
| `resolved` | ResolvedLoad |
| `fit` | Option<Verdict> |
| `too_large` (`tooLarge`) | bool |
| `context_options` (`contextOptions`) | Vec<ContextOption>, Automatic first |
| `gpu_options` (`gpuOptions`) | Vec<GpuOption>, in the order All, Fit, Off |
| `devices` | Vec<DeviceOption> |
| `loaded` | bool |
| `older_settings_loaded` (`olderSettingsLoaded`) | bool |
| `auto_ctx` (`autoCtx`) | Option<u32> |

`sample_info(path)` builds the sample described under the commands below.

## Rust: `dto.rs`

**New fields on `RuntimeStatusDto`** (`stage: Option<String>` stays as it was):
- `load_stage` (`loadStage`): `Option<LoadStage>`. Set to Starting when a load begins and cleared on Ready.
- `adjustments`: `Vec<LoadStage>`. Reset by each load.
- `ctx_source` (`ctxSource`): `Option<ContextSource>`. Always None until WP1b fills it.
- `settings_changed` (`settingsChanged`): bool.
- `reload_waiting` (`reloadWaiting`): bool.

**New fields on `LocalModelSettings`**, each `#[serde(default)]`; `ctx: u32` stays:
- `context_auto` (`contextAuto`): bool. An absent key reads as false. The upgrade rule is WP1b's and must read the raw JSON to tell "absent" from "false".
- `gpu`: GpuUse.
- `save_memory`: bool.
- `keep_in_memory`: bool.
- `thinking_limit`: ThinkingLimit.
- `upgrade_note` (`upgradeNote`): `Option<u32>`, left out of the JSON when None. It is response-only, and `set_local_model_settings` drops it.

**`ModelBusy { chat_turns (chatTurns), model_checks (modelChecks), other_apps (otherApps): u32 }`**, with `.is_idle()`.

**Comparison types:**
- `CompareItem`:
  - Rust variants: `Catalog{catalog_id}`, `Repo{repo,file}`, `Installed{installed_path}`.
  - JSON: exactly one of `{"catalogId"}`, `{"repo","file"}` or `{"installedPath"}`. Anything else is refused.
- `LicenseFacts { name, url: Option }`.
- `CatalogFacts`:
  - id, name, summary
  - `source` (URL of the publisher's model card)
  - `good_at` (`goodAt`): Vec<String>
  - publisher
  - `parameters`, `active_parameters` (`activeParameters`): Option<u64>
  - `languages`: Vec
  - `released`
  - `capabilities`: Vec
  - `license`: Option<LicenseFacts>
- `BaseModelFacts { base_model (baseModel), catalog: CatalogFacts }`.
- `HubFacts`:
  - repo, author
  - `pipeline_tag` (`pipelineTag`), `base_model` (`baseModel`)
  - languages, license, tags
  - downloads, likes
  - `last_modified` (`lastModified`)
  - gated
  - `card_url` (`cardUrl`)
- `FileFacts { architecture, parameters, context_length (contextLength), tools, thinking, vision, moe, quantization }`.
- `YourResultKind { ModelCheck, Benchmark }`, JSON `"modelCheck"|"benchmark"`.
- `YourResult { kind, id, at: i64 (ms), backend, passed: u32, total: u32 }`.
- `CompareProblem { Offline, RateLimited, Gated, NotFound, Failed }`, JSON camelCase.
- `CompareFacts`:
  - item, title, file, quant
  - `size_bytes` (`sizeBytes`)
  - `installed_path` (`installedPath`)
  - `ctx`: u32
  - `fit`: Option<FitResult>
  - catalog
  - `base_catalog` (`baseCatalog`)
  - hub
  - `file_facts` (`fileFacts`)
  - `results`: Vec<YourResult>
  - `problem`
- `CatalogListEntry`:
  - `catalog`: CatalogFacts
  - repo, file, quant
  - `size_bytes` (`sizeBytes`)
  - fit
  - `installed_path` (`installedPath`)
  - results

## Rust: `state.rs`: the ModelBusy API

- `AppState.model_busy: Arc<BusyCounter>`
- `enum BusyKind { ChatTurn, ModelCheck, OtherApp }`
- `BusyCounter` methods:
  - `hold(self: &Arc<Self>, kind) -> BusyGuard` (`#[must_use]`)
  - `snapshot() -> ModelBusy`
  - `is_busy() -> bool`
  - `async wait_idle()`: returns at once when idle. Cancel it by dropping the future, for example inside `select!` with a token.
  - `subscribe() -> watch::Receiver<ModelBusy>`
- `BusyGuard`: `kind()`. Dropping it counts down, even when the drop happens because of a panic.

## Rust: `share/mod.rs`

- `SETTINGS_KEY = "shareWithApps"`
- `ShareSettings { enabled: bool, port: u16 }`
- `ShareStatus { enabled, port: Option<u16>, listening, error: Option<String>, refused: u64, in_use (inUse), model_ctx (modelCtx): Option<u32> }`
- `ShareApp { ClaudeCode, Codex, ClaudeApp }`, JSON `"claudeCode"|"codex"|"claudeApp"`
- `SetupSystem { Windows, Mac, Any }`
- `SetupStep { text, code: Option, file: Option }`
- `SetupWay { title, system, steps }`
- `ShareSetup { app, title, not_tried (notTried), ways, notes: Vec<String> }`

## Commands

Every new command takes `window: tauri::WebviewWindow` and checks it with `commands::only_main_window` (the error is `bad_request`, "Only the main window can do this."). All return `AppResult<…>`. Rust parameter names are snake_case; JavaScript sends them in camelCase.

**Load settings (`models_cmd.rs`, WP1b):**

| Command (JS arguments) | Rust signature, after `window` | Stub returns |
|---|---|---|
| `load_local_model {path, settings}` | `(app: AppHandle, state, path: String, settings: Option<LocalModelSettings>) -> ()` | `settings` may be null or missing; that means the model's saved settings, and for now the defaults for all models. |
| `model_load_info {path}` | `(_state, path: String) -> ModelLoadInfo` | `sample_info(path)`, see below |
| `set_model_load_settings {path, overrides}` | `(_state, path: String, overrides: ModelLoadSettings) -> ModelLoadInfo` | the sample, with `overrides` echoed back; nothing saved |
| `reset_model_load_settings {path}` | `(_state, path: String) -> ModelLoadInfo` | the sample |
| `reload_local_model {force, whenIdle}` | `(_state, force: bool, when_idle: bool) -> ()` | does nothing |
| `cancel_pending_reload {}` | `(_state) -> ()` | does nothing |
| `model_busy {}` | `(state) -> ModelBusy` | already real: the live counter, always 0 for now |

What `reload_local_model` means:
- `force`: restart even when nothing changed.
- `when_idle`: wait until the busy counter is zero, with `reload_waiting` set on the status, and cancellable through `cancel_pending_reload`.
- Without `when_idle`: stop whatever uses the model, then restart. This is D3's "Stop and restart".

What `sample_info(path)` returns:
- `fileName` from the path, `sizeBytes` 4,920,000,000, `trainedCtx` 40,960, `thinking` true, `vision` false.
- `overrides` all `"default"`, `defaults` = `LocalModelSettings::default()`.
- `resolved`: ctx 8192, `{"fixed":8192}`, defaultSetting, all, false, false, noLimit, null, null, null.
- `fit` green, `tooLarge` false.
- `contextOptions`: auto 24,576 green; 4096, 8192 and 16,384 green; 32,768 yellow; 40,960 red.
- `gpuOptions`: all green, fit green, off yellow.
- One device: `{name "Vulkan0", description "Sample graphics card", 16384, 15000, recommended true}`.
- `loaded` false, `olderSettingsLoaded` false, `autoCtx` null.

`unload_model` already exists; WP1b wires it.

**Comparison (`hub.rs`, WP4a):**

| Command (JS arguments) | Rust signature, after `window` | Stub returns |
|---|---|---|
| `catalog_list {ctx}` | `(_state, ctx: Option<u32>) -> Vec<CatalogListEntry>` | 2 entries, `sample-small` (green) and `sample-large` (yellow) |
| `compare_facts {items, ctx}` | `(_state, items: Vec<CompareItem>, ctx: Option<u32>) -> Vec<CompareFacts>` | one entry per item, in the order given, echoing the item and `ctx` |

- A null `ctx` means 8192 in the stubs.
- Every `catalog_list` entry: repo `example/<id>-GGUF`, file `<id>-Q4_K_M.gguf`, quant Q4_K_M, 2.7 GB, a catalog summary marked as sample, goodAt `[chat, coding]`.
- `compare_facts`, depending on the item:
  - a catalog item gets `catalog`;
  - a repo item gets `hub`, with `cardUrl` `https://huggingface.co/<repo>`;
  - an installed item gets `installedPath`, with the title taken from the file name.
  - Every item gets `fileFacts` and a green `fit`.

**Sharing (`share/mod.rs`, WP5a; nothing is saved and nothing listens):**

| Command (JS arguments) | Rust signature, after `window` | Stub returns |
|---|---|---|
| `share_status {}` | `(_state) -> ShareStatus` | all off and null, `refused` 0 |
| `set_share_enabled {on}` | `(_state, on: bool) -> ShareStatus` | `enabled = on`, port 51234 when on, `listening` false |
| `set_share_port {port}` | `(_state, port: u16) -> ShareStatus` | the port echoed back |
| `reveal_share_key {}` | `(_state) -> String` | `"harnessui-share-" + "0"×32` |
| `new_share_key {}` | `(_state) -> ShareStatus` | the sample status |
| `share_setup {app}` | `(_state, app: ShareApp) -> ShareSetup` | one sample way for windows and one for mac, `notTried` true only for claudeApp |

## Frontend

- **`types.ts`**, mirrors of all the types above:
  - `BackendKind`, `LoadFailure`, `LoadStage`, `GpuUse`, `ThinkingLimit`
  - `ContextChoice`, `ContextSize`, `GpuChoice`, `Toggle`, `DeviceChoice`, `ThinkingChoice`
  - `ModelLoadSettings`, `ContextSource`, `ResolvedLoad`, `ContextOption`, `GpuOption`, `DeviceOption`, `ModelLoadInfo`, `ModelBusy`
  - `CompareItem`, `LicenseFacts`, `CatalogFacts`, `BaseModelFacts`, `HubFacts`, `FileFacts`, `YourResultKind`, `YourResult`, `CompareProblem`, `CompareFacts`, `CatalogListEntry`
  - `ShareStatus`, `ShareApp`, `SetupSystem`, `SetupStep`, `SetupWay`, `ShareSetup`
  - `RuntimeStatus` and `LocalModelSettings` gain the fields listed above; `upgradeNote?: number`.
- **`ipc.ts`:**
  - `loadLocalModel(path, settings?: LocalModelSettings | null)`
  - `modelLoadInfo(path)`
  - `setModelLoadSettings(path, overrides)`
  - `resetModelLoadSettings(path)`
  - `reloadLocalModel(force, whenIdle)`
  - `cancelPendingReload()`
  - `modelBusy()`
  - `catalogList(ctx: number | null)`
  - `compareFacts(items, ctx: number | null)`
  - `shareStatus()`
  - `setShareEnabled(on)`
  - `setSharePort(port)`
  - `revealShareKey()`
  - `newShareKey()`
  - `shareSetup(app)`
- **`words.ts`** (WP2 finishes it):
  - `BACKEND_WORDS: Record<BackendKind,string>`, with the five phrases from the spec
  - `backendWords(kind | null)`
  - `GPU_USE_WORDS`
  - `THINKING_WORDS` (`noLimit` is "Default")
  - `aboutWords(tokens)`: three quarters of the tokens, rounded down to the thousand, or to the hundred below 1,000
  - `tokensWithWords(tokens)`: for example "8,192 tokens, about 6,000 words" or "32,768 tokens, about 24,000 words"
  - `loadStageWords(stage)`: an exhaustive switch; the wording is still to finish (`TODO(WP2)`)
- **The comparison store API, `src/store/compare.ts`:**
  - `COMPARE_LIMIT = 4`
  - `type CompareEntry = {catalogId} | {installedPath} | {repo; file: string | null}`
  - `sameEntry(a, b)`
  - `toCompareItem(entry): CompareItem | null`, null for a repository entry with no file yet
  - `useCompare` (zustand), holding `entries` and the actions:
    - `add(e): boolean`, false when the model is already there or the list is full
    - `remove(e)`
    - `setFile(repo, from, to)`
    - `clear()`
  - `useAddToComparison(entry): { added, full, add(): boolean, remove() }`
- **Layout of the split components:**
  - `ModelsDialog.tsx`: the shell and the tabs (Installed, Browse, Downloads); `tabClass` lives here.
  - `models/Installed.tsx`: `export function Installed(props: { onClose: () => void })`. It contains `use()`, which reads the defaults, sets the chat's model, calls `onClose`, loads the model and refreshes the model list.
  - `models/Browse.tsx`: `Browse({ onStarted })`, which renders `RepoDetail`.
  - `models/RepoDetail.tsx`: `RepoDetail({ repoId, onBack, onStarted })`.
  - `models/Downloads.tsx`: `Downloads()`.
  - `models/Badge.tsx`: `Badge({ children, title? })`.
  - `settings/LocalModels.tsx`: `LocalModels()`, the old `Runtime()` unchanged, mounted in the "runtime" tab as `<LocalModels /><ModelCheck />`.
