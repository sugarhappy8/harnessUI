# WP0 fix report (0.0.5 groundwork, after the independent check)

I changed no git state. The tree now has these uncommitted changes, on top of `08702aa`:
- Modified: `src-tauri/src/dto.rs`, `src-tauri/src/hub.rs`, `src-tauri/src/models_cmd.rs`, `src-tauri/src/runtime_manager.rs`, `src/lib/ipc.ts`, `src/lib/types.ts`, `src/components/Welcome.test.tsx`.
- New: `src/lib/ipc.test.ts`.
- The two earlier uncommitted edits in `models_cmd.rs` are still there: `upgrade_note` is dropped on save, and the `reload_local_model` doc comment is longer.

Nothing changes on screen: no label, launch argument or behaviour is different.

## What I changed, problem by problem

**1. MUST-FIX (D3: no backend wait for switching to another model or for Unload). Fixed with option (a).**
- `models_cmd.rs`:
  - `load_local_model` takes a new last parameter, `when_idle: Option<bool>`.
  - `unload_model` takes `when_idle: Option<bool>`.
  - Both stubs ignore the value and act at once, as in 0.0.4. Each is marked `TODO(WP1b)` (lines 46 and 117).
- The doc comments of `reload_local_model` and `cancel_pending_reload` now set one rule for all three commands:
  - `true`: wait until the busy counter is zero, with `reload_waiting` set.
  - absent or `false`: act now, and stop whatever uses the model first.
  - Only one change waits at a time; any newer load, unload or reload cancels it.
  - The call returns when the change is over.
  - A cancelled wait changes nothing and ends with the existing error code `load_replaced`.
  - `cancel_pending_reload` cancels whichever change is waiting.
- `dto.rs` / `types.ts`: the docs of `reload_waiting` / `reloadWaiting` now say it covers a waiting reload, load or unload.
- `ipc.ts`:
  - New private helper `unlessReplaced`.
  - `loadLocalModel(path, settings?, whenIdle?)` still returns `Promise<boolean>` and sends `whenIdle: whenIdle ?? null`.
  - `unloadModel(whenIdle?)` and `reloadLocalModel(force, whenIdle)` now return `Promise<boolean>`. They resolve to false on `load_replaced`, so all three behave the same. Nothing in `src` called either of them before.
- Old calls still work: the e2e scripts send `{path, settings}` and `unload_model` with no arguments, which reads as absent, meaning "now".

**2. SHOULD-FIX (no "unloaded" state). Fixed.**
- New `unloaded_path: Option<String>` on `RuntimeStatusDto` and its `Default`, and `unloadedPath: string | null` on `RuntimeStatus`.
- The `Welcome.test.tsx` fixture gained `unloadedPath: null`.
- `runtime_status_defaults` now also checks `unloadedPath`.
- The stub leaves it `None` (`TODO(WP1b)`). Clearing it already works: `load()` and `unload()` both replace the whole status, so the next load and any other unload reset it.

**3. SHOULD-FIX (failed load keeps a stale `load_stage`). Fixed.**
- New `fn mark_failed(s, err)` in `runtime_manager.rs`. It sets the state to Failed, clears progress, sets `load_stage = None` and stores the error. It leaves `adjustments` alone, since they explain the failure.
- There were two places that mark a load as failed, not one: the `ServerState::Failed` branch of `on_event`, and the end of `load()` when the supervisor gives up. Both now use `mark_failed`.
- New test `a_failed_load_clears_its_stage_and_keeps_its_adjustments`. I saw it fail first, on the `load_stage` assertion, before adding the line.

**Notes:**
- **Note 4 (default context on a fresh install).** No code change; this is the lead's decision. Recorded as open in the contract below, with a recommendation.
- **Note 5 (owners of the new files).** The spec is lead-only, so I listed the owners in the contract.
- **Note 6 (no push when the busy counts change).** Written into the contract: there is no event, so WP2 calls `model_busy` and polls.
- **Note 7 (FileFacts).** Added `expert_count`, `expert_used_count` and `active_parameters`, all `Option<u64>`, in `dto.rs` and `types.ts`. The `hub.rs` stub sets them to None. New test `file_facts_expert_fields` checks that JSON saved without them still reads.
- **Note 8 (callers that still pass the defaults).** The three call sites are named under WP2 in the contract.
- **Note 9 (m2 drop target).** Nothing to change in WP0.

## Check results

| Check | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo +1.99 clippy --workspace --all-targets -- -D warnings` | pass, 0 warnings |
| `cargo +1.99 test --workspace` | 1,728 passed, 0 failed, 6 ignored (was 1,726; the 2 new Rust tests are the difference) |
| `npm run lint` | pass |
| `npm run typecheck` | pass |
| `npm test` | 39 files, 255 tests passed (was 38 files and 253 tests; the new file is `src/lib/ipc.test.ts` with 2 tests) |
| e2e `settings-pages` | PASS, 0 console lines, after `cargo +1.99 build -p app`. Output in `/tmp/claude-0/-home-user-harnissUI/23ef2267-cb6f-5325-b082-a4b40b3ccb19/scratchpad/v005/e2e-wp0-fix`. |

- **Dev server:** the runner again did not start one, so I started `npm run dev` on port 1420 myself and stopped it afterwards.
- **`ipc.test.ts`:** I made one deliberate breakage, letting `unloadModel` skip `unlessReplaced`. The test failed, and I put the code back.
- **Real-app probe:** `/tmp/claude-0/-home-user-harnissUI/23ef2267-cb6f-5325-b082-a4b40b3ccb19/scratchpad/v005/probe_when_idle.py` gave PROBE PASS.
  - `runtime_status` contains `unloadedPath: null`.
  - `load_local_model` reaches its own "Pick a .gguf model file." check with `whenIdle` absent, null and true.
  - `whenIdle: "yes"` is refused at argument parsing for both commands.
  - `unload_model` accepts `{}`, null, true and false.
  - The real small model loads with `{ctx 4096}` and `whenIdle: true`: ready, nCtx 4096, `loadStage` null. `unload_model {whenIdle:true}` then leaves the state stopped.
- **Not re-run:** `npm run build`, `scripts/notices.py --check`, `scripts/check-spawn.sh` and `e2e/m2_models.py`. No dependencies, processes or e2e files changed.

## Current TODO markers
- `src-tauri/src/models_cmd.rs`: line 44 (resolver), line 46 (`when_idle` on load), line 117 (`when_idle` and `unloaded_path` on unload), line 125 (stubs).
- `src-tauri/src/load_settings.rs`: line 334.
- `src-tauri/src/hub.rs`: line 1072.
- `src-tauri/src/share/mod.rs`: line 107.
- `src/lib/words.ts`: line 53.

---

# CONTRACT (updated)

**Changes since the first version:**
- `load_local_model` and `unload_model` gain `when_idle`.
- One rule now covers every waiting change, and a cancelled wait ends with `load_replaced`.
- `reload_waiting` covers loads and unloads too.
- New `unloaded_path`.
- `load_stage` is also cleared on Failed.
- `unloadModel` and `reloadLocalModel` return `Promise<boolean>`.
- `FileFacts` gains three expert fields.
- New sections: busy polling, file owners, WP2 call sites, and the open default-context question.

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
- `load_stage` (`loadStage`): `Option<LoadStage>`. Set to Starting when a load begins. **Cleared on Ready and on Failed (amended).** Both places that mark a load as failed go through `runtime_manager::mark_failed`.
- `adjustments`: `Vec<LoadStage>`. Reset by each load. Kept after Ready and after Failed.
- `ctx_source` (`ctxSource`): `Option<ContextSource>`. Always None until WP1b fills it.
- `settings_changed` (`settingsChanged`): bool.
- `reload_waiting` (`reloadWaiting`): bool. **(amended)** True while any change of what is loaded waits for the model to be free: a reload, a load of another model, or an unload, each called with `when_idle: true`. The other status fields still describe the model loaded now. Cancelled with `cancel_pending_reload`. Shown as "Restarts when the reply ends · Cancel".
- **`unloaded_path` (`unloadedPath`): `Option<String>` (new).**
  - The model the user unloaded with `unload_model`, for "Unloaded to free memory · Load".
  - Cleared by the next load and by any other unload (removing the model or its backend, diagnostics). Both already replace the whole status.
  - Default None. The stub never sets it; WP1b sets it in `unload_model`.

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
- `FileFacts`:
  - architecture, parameters
  - `context_length` (`contextLength`)
  - tools, thinking, vision, moe, quantization
  - **`expert_count` (`expertCount`), `expert_used_count` (`expertUsedCount`), `active_parameters` (`activeParameters`): `Option<u64>` (new).** For a mixture of experts only. `active_parameters` is set only when WP4a can work it out from the file. JSON saved without these fields still reads (None). The stub sets all three to None.
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
- **The webview gets no busy event (new).** `subscribe()` is for Rust only. WP2 calls `model_busy` right before asking the D3 question. Where it shows a line that depends on the counts, it polls `model_busy` (about once a second, only while that line is visible). "Used by another app" comes from `ShareStatus.in_use`.

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

`load_local_model` and `unload_model` are existing commands. They keep the 0.0.4 shape, have no window check, and are granted only in `main.json`.

**Load settings (`models_cmd.rs`, WP1b):**

| Command (JS arguments) | Rust signature, after `window` where it has one | Stub returns |
|---|---|---|
| `load_local_model {path, settings, whenIdle}` **(amended)** | `(app: AppHandle, state, path: String, settings: Option<LocalModelSettings>, when_idle: Option<bool>) -> ()` | `settings` null or missing means the model's saved settings (for now the defaults for all models). `when_idle` is ignored; it loads at once. |
| `unload_model {whenIdle}` **(amended)** | `(state, when_idle: Option<bool>) -> ()` | unloads at once, as in 0.0.4; `when_idle` ignored; `unloaded_path` not set yet |
| `model_load_info {path}` | `(_state, path: String) -> ModelLoadInfo` | `sample_info(path)`, see below |
| `set_model_load_settings {path, overrides}` | `(_state, path: String, overrides: ModelLoadSettings) -> ModelLoadInfo` | the sample, with `overrides` echoed back; nothing saved |
| `reset_model_load_settings {path}` | `(_state, path: String) -> ModelLoadInfo` | the sample |
| `reload_local_model {force, whenIdle}` | `(_state, force: bool, when_idle: bool) -> ()` | does nothing |
| `cancel_pending_reload {}` | `(_state) -> ()` | does nothing |
| `model_busy {}` | `(state) -> ModelBusy` | already real: the live counter, always 0 for now |

**The D3 rule for `reload_local_model`, `load_local_model` and `unload_model` (amended):**
- `force` (reload only): restart even when the loaded copy already uses the current settings. This is Reload; it also retries the chosen backend after a step-down. Without it, nothing happens when nothing changed.
- `when_idle: true` ("Restart when it ends"):
  - If anything uses the model, wait until the busy counter is zero, with `reload_waiting` set; then act.
  - If nothing uses the model, act at once.
- `when_idle` false or absent ("Stop and restart", and every call when the model is idle): act now, stopping whatever uses the model first.
- At most one change waits at a time. A newer load, unload or reload, waiting or not, cancels the waiting one.
- Every call returns when its change is over: loaded, failed, unloaded or restarted.
- A waiting call that is cancelled changes nothing and ends with the error code `load_replaced`. The UI shows nothing for it, and the `ipc.ts` wrappers resolve to `false`.
- `cancel_pending_reload` cancels whichever change waits. Nothing waiting is not an error.

What `sample_info(path)` returns:
- `fileName` from the path, `sizeBytes` 4,920,000,000, `trainedCtx` 40,960, `thinking` true, `vision` false.
- `overrides` all `"default"`, `defaults` = `LocalModelSettings::default()`.
- `resolved`: ctx 8192, `{"fixed":8192}`, defaultSetting, all, false, false, noLimit, null, null, null.
- `fit` green, `tooLarge` false.
- `contextOptions`: auto 24,576 green; 4096, 8192 and 16,384 green; 32,768 yellow; 40,960 red.
- `gpuOptions`: all green, fit green, off yellow.
- One device: `{name "Vulkan0", description "Sample graphics card", 16384, 15000, recommended true}`.
- `loaded` false, `olderSettingsLoaded` false, `autoCtx` null.

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
  - Every item gets `fileFacts` (expert fields null) and a green `fit`.

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
  - `CompareItem`, `LicenseFacts`, `CatalogFacts`, `BaseModelFacts`, `HubFacts`, `FileFacts` (with `expertCount`, `expertUsedCount`, `activeParameters: number | null`), `YourResultKind`, `YourResult`, `CompareProblem`, `CompareFacts`, `CatalogListEntry`
  - `ShareStatus`, `ShareApp`, `SetupSystem`, `SetupStep`, `SetupWay`, `ShareSetup`
  - `RuntimeStatus` and `LocalModelSettings` gain the fields listed above, including `unloadedPath: string | null`; `upgradeNote?: number`.
- **`ipc.ts`** (amended where marked):
  - `loadLocalModel(path, settings?: LocalModelSettings | null, whenIdle?: boolean): Promise<boolean>`
    - Sends `{path, settings: settings ?? null, whenIdle: whenIdle ?? null}`.
    - Resolves `false` on `load_replaced`, meaning it was replaced by another pick or its wait was cancelled.
  - `unloadModel(whenIdle?: boolean): Promise<boolean>` **(amended, was `() => Promise<void>`)**. Resolves `false` when a waiting unload was cancelled or replaced.
  - `modelLoadInfo(path)`
  - `setModelLoadSettings(path, overrides)`
  - `resetModelLoadSettings(path)`
  - `reloadLocalModel(force, whenIdle): Promise<boolean>` **(amended, was `Promise<void>`)**. Resolves `false` when a waiting reload was cancelled or replaced.
  - `cancelPendingReload()`: cancels whichever reload, load or unload waits.
  - `modelBusy()`
  - `catalogList(ctx: number | null)`
  - `compareFacts(items, ctx: number | null)`
  - `shareStatus()`
  - `setShareEnabled(on)`
  - `setSharePort(port)`
  - `revealShareKey()`
  - `newShareKey()`
  - `shareSetup(app)`
  - Pinned by `src/lib/ipc.test.ts`.
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

## For WP2: callers that still pass the defaults (new)

- These three call sites still read `getLocalModelSettings()` and pass the result to `loadLocalModel(path, settings)`:
  - `src/components/models/Installed.tsx:26` (`use()`)
  - `src/components/FirstRun.tsx:79`
  - `src/store/app.ts:397` (`pickModel`)
- Once WP1b's resolver lands, these calls skip the per-model settings.
- WP2 owns all three and should change them to `loadLocalModel(path)`, or `loadLocalModel(path, null, whenIdle)` after the D3 question.
- Nothing in `src` calls `unloadModel` or `reloadLocalModel` yet.

## Owners of the files WP0 added (new; the spec's Owns lists do not name them, so the lead should add them to the briefs)

- `crates/runtime/src/llama/stage.rs`: WP1a.
- `crates/agent/src/thinking.rs`: WP3a.
- `src-tauri/src/load_settings.rs`: WP1b.
- `src-tauri/src/share/mod.rs`: WP5a.
- `src/components/models/Installed.tsx`, `src/components/settings/LocalModels.tsx`, `src/lib/words.ts`: WP2.
- `src/components/models/Browse.tsx`, `src/components/models/RepoDetail.tsx`, `src/store/compare.ts`: WP4b.
- `src/components/models/Badge.tsx`: lead.
- `src/components/models/Downloads.tsx`: lead. No package needs to change it.
- `src/lib/ipc.test.ts`: whoever changes the three load, unload or reload wrappers.

## Open for the lead (new; not decided by WP0)

- **The situation:** a fresh install has no `localModel` key, so it reads `LocalModelSettings::default()`, which is a fixed 8,192 tokens with `context_auto` false.
- **The mismatch:** a 0.0.4 user whose stored `ctx` is 8,192 gets Automatic under D2's upgrade rule, so a new user would get less than an upgrading one.
- **Recommendation:** WP1b's resolver treats "no stored `localModel` key" like the stored-8,192 case, meaning Automatic, when it reads the raw JSON. Then `LocalModelSettings::default()` and `dto.rs` need no edit later.
- If the lead decides otherwise, the fresh-install default stays a fixed 8,192.