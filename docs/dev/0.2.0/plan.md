# harnessUI 0.2.0 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Any picture or video model the pinned engine can run, from Hugging Face or from disk, is recognised from its file header, set up from the engine's own recipe plus a pinned table of companion files, downloaded with one button and run in Studio, with LoRAs. The spending guards become optional switches, local run blocks become warnings, chat mode switches stop asking, and three standing failures are fixed.

**Architecture:**
- A new `models::sd` module:
  - vendors the engine's 112 recipes;
  - reads safetensors and GGUF tensor tables, from local files and from bounded HTTP Range prefixes;
  - ports the engine's `ModelLoader::get_sd_version` in its own evaluation order;
  - labels each file as a model, a LoRA, an extra file or one Studio can't run;
  - picks a recipe for it and fills the recipe's slots from installed files, the same repo, or a pinned companion table.
- Schema 4 records each Studio model's recipe and adds `gen_loras`.
- `src-tauri/src/studio` gains a detection cache, a set-up plan command, import from disk, a *Set it up yourself* backend and LoRA commands.
- `runtime::sd` builds engine arguments from the recipe's slots, defaults and constraints, with the pinned engine's full sampler and scheduler lists.
- The six 0.1.x catalog families map onto recipes, and a 0.1.2 picture run keeps its exact arguments.
- Phase U rebuilds Studio's screens (section 10) on Phase S's DTOs and store actions: one composer, the model sheet, Get models with a detail panel, downloads with *Use it*, the Settings panel, and the results. A preview harness takes before/after screenshots.

**Tech Stack:**
- Rust: tokio, reqwest, rusqlite, serde, regex, sha2, Tauri 2.
- Webview: React 19, TypeScript, zustand, Radix, vitest.
- Scripts: Python 3, standard library only.
- Engine: stable-diffusion.cpp `master-1106-532d2c1`, run as a pinned external program.
- No new Rust or npm dependency.

**Spec:** `docs/dev/0.2.0/design.md`. The owner's decisions at its top override its body. The engine facts below come from its source at commit `532d2c15c430a785a712b2d94281f04e733008a4`, cited as `file:line`. The Hugging Face facts were checked against the API on 2026-11-24.

**Commit rules:**
- Every commit message ends with the attribution trailer your session requires.
- Run `cargo fmt --all` before each commit.
- `crates/agent`, `crates/store`, `crates/runtime`, `crates/reverse` and `crates/mcp-host` have `#![warn(missing_docs)]`. Every public item there needs a doc comment, or clippy (`-D warnings`) fails.
- A task is done when its own tests pass and these stay clean: `cargo clippy --workspace --all-targets -- -D warnings`, `bash scripts/check-spawn.sh`, `npm run lint`, `npm run typecheck`, `npm test` and `python3 -I scripts/docs_test.py`.

**Order:** Phase R → Phase S → Phase U → Phase G → Phase F → Phase D. Each phase, and each task, leaves the app building and working.
- **Phase R** adds library code and catalog data only. Nothing in the app calls it yet.
- **Phase S** switches Studio's local side onto it, keeping the 0.1.2 webview working with small adaptations.
- **Phase U** replaces those screens. Its click-count target (at most 4 clicks from the results to using a new model) is a test in Task U7.
- **Phases G and F** do not depend on Phase U. They may run beside it.

---

## Spec corrections

Each is decided below in the task named. None is silently diverged from.

1. **"61 model versions" (section 1).**
   - The engine's `SDVersion` enum has 61 values, but `VERSION_ESRGAN` is never returned by detection.
   - **60 versions are detectable.** SVD is detected but cannot run (section 3), so `match_recipe` refuses it (Task R7).
2. **The recipe schema (7.1) is wider than listed.** The 112 recipes at the pin also use:
   - `video_constraints.max_frames` (1 recipe) and `video_constraints.fixed_fps` (5);
   - the defaults `custom_sigmas` (a list, 5 recipes), `img_cfg_scale`, `strength` and `shifted_timestep`.
   - `size_multiple` is omitted by 30 recipes, and then means 8 (`examples/server/api.md`).
   - `high_noise_flow_shift` (2 recipes) has no `sd-cli` flag, so it is not sent.
   - Tasks R1 and S2.
3. **"The engine refuses anything else: size multiples, the frame rule, maximum sides and frames" (9.2).**
   - The pinned engine does not refuse these. It rounds them (`src/pipeline/request.cpp:211-231`, `src/pipeline/diffusion_engine.cpp:3007-3044`).
   - Studio keeps validating, so the person gets exactly the size and length shown:
     - the recipe's multiple;
     - Studio's own 64–2048 px side range;
     - the recipe's frame rule.
   - Tasks S2 and S11.
4. **"Studio does not pass `--offload-to-cpu` unless the person chooses *Use the processor*" (7.6).**
   - *Use the processor* sends `--backend cpu`, where `--offload-to-cpu` does nothing.
   - The flag is passed only with *Memory saving: On*, and on the one out-of-memory retry, as in 0.1.2.
   - Task S11.
5. **"It adds `--vae-tiling` for video and for pictures above 1024×1024" (7.6).**
   - Four recipes disable the `vae_tiling` feature: `chroma-radiance`, `iris-3b`, `minit2i` and `z-image-l2p`.
   - For those, tiling is never sent. Task S11.
6. **"The existing reader, extended to 5-D tensors" (7.2).**
   - 0.1.2 already reads 5-D GGUF tensors (`gguf::read_diffusion_tensor_shapes`).
   - What is missing is the engine's folding of the extra dimensions into `ne[3]` (`src/model_io/gguf_reader_ext.h:171-178`, `src/model_io/safetensors_io.cpp:330-344`), and the order in which the engine visits safetensors names (sorted, through `nlohmann::json`).
   - Task R2.
7. **"The database migration is additive" (5, 7.4).**
   - Adding `import` to the `source` CHECK needs a SQLite table rebuild.
   - The rebuild copies every row, keeps every column and is pinned by a test, so nothing is lost. Task S1.
8. **The engine runs some non-goal models under another name (section 3 lists them as can't-run).** Studio adds guards so that "Works in Studio" is never shown for them (Task R6):
   - **Lumina 2 and Neta Lumina** (`model.diffusion_model.cap_embedder.0.weight`) detect as `Z_IMAGE`. Studio tells them apart by that tensor's width: 2304 (Gemma-2-2B) for Lumina 2, 2560 (Qwen3-4B) for Z-Image.
   - **LTX-Video 0.9** detects as `LTXAV`. Studio tells it apart by the missing `audio_patchify_proj`.
   - **HunyuanVideo v1** detects as `HUNYUAN_VIDEO`, the same as 1.5. Studio tells it apart by the missing `byt5_in.fc1.weight`, which 1.5 has.
9. **Diffusers folders and sharded files (section 3, 7.2).**
   - Section 3 implies SD 1.x/2.x/XL diffusers folders run. They do, with `-m <folder>`: the engine reads `unet/`, `vae/`, `text_encoder/` and `text_encoder_2/` (`src/model_loader.cpp:421-441`). Task R6.
   - The engine loads one file per slot and never joins shards. A `*.safetensors.index.json` model is read, to say what it is, and labelled "Can't run in Studio: split into parts". Its index file also has no LFS SHA-256 to pin.
10. **The `tokenizer` slot takes a `.json` file, not weights.** The recipes that need it are `lens*`, `llada-image*`, `ming-image` and `moss-image-*`. `.json` is allowed for that slot only. Task S3.
11. **Removing C5, the saved Full-access start mode asked again at launch, removes a documented protection** against another program editing the setting. The owner chose this (9.3). SECURITY.md says so in Task G4.
12. **Section 11's guesses at the Reverse engineering failures, refined (Tasks F1 and F2):**
    - **Linux.** The non-loopback `connect(… 10.1.0.32 port 0)` is glibc `getaddrinfo`'s RFC 6724 source-address check. It is a UDP `connect` that sends nothing. It happens because the workflow appends a second `127.0.1.1` line for the host name, so the name resolves to two addresses.
    - **macOS.** rea 6.9.0's Kuna version probe has a 10 s deadline (`dist/kuna/KunaInstallation.js:19, 270`). That deadline also covers the first compile of rea's Darwin process helper, which has its own 120 s bound (`dist/process/OwnedCommand.js:60-73`).
13. **The llama-server tail (11).** The supervisor does drain llama-server's output before it reports the exit, but with problems:
    - it gives each reader at most 2 s;
    - it drains one reader after the other;
    - it aborts a slow reader (`crates/runtime/src/llama/server.rs:709-719`).
    - Task F3 drains both together, to the end, with an idle deadline.
14. **LoRA names (5, 8).**
    - A LoRA name cannot contain `:`, and it also cannot contain `>`: the engine's pattern is `<lora:([^:>]+):([^>]+)>` (`examples/common/common.cpp:2356`).
    - `|` is reserved for the `|high_noise|` prefix.
    - Studio's LoRA names drop `:`, `<`, `>` and `|`. Task S10.
15. **Typed LoRA tags (7.6).**
    - Once `--lora-model-dir` is set, any `<lora:…>` text typed into the prompt also takes effect (`common.cpp:2352-2353, 2585-2588`).
    - Studio removes such spans from the typed prompt, so only attached LoRAs apply. Task S2.
16. **"Wan 2.2 video model, 14B, high-noise part" (7.3).**
    - Detection alone cannot say this. Wan 2.2 A14B high- and low-noise files both detect as `WAN2`, like Wan 2.1 14B T2V.
    - The 2.2 and the part come from the file and folder name ("HighNoise", "low_noise"). Task R7.
17. **The 10 MB floor for header reads (7.3) would skip most LoRAs**, many of which are 2–9 MB. Files whose name says LoRA are read at any size. Task S5.
18. **Design 11a's Flex.2 control picture.**
    - The `flex2` recipe lists no `control_image` feature, but the engine reads a control picture for Flex.2 (`src/pipeline/image.cpp:478-487`), and a mask and a picture to change too (`image.cpp:450-487`).
    - Studio offers all three, each optional (Task R7).
19. **Design 11a's "black-and-white" mask.**
    - The engine resizes any mask to the output size and rounds each pixel to black or white (`image.cpp:262-264`), so a grey mask works too.
    - Studio accepts any picture as a mask, and says white areas are the ones changed (Tasks R7 and S12).

## Global Constraints

**Engine and recipes**
- Engine: stable-diffusion.cpp commit `532d2c15c430a785a712b2d94281f04e733008a4` (`sd.lock.json`, tag `master-1106-532d2c1`, build 1106). The pin does not change in 0.2.0.
- Recipes come from that same commit. A test fails if `catalog/sd-recipes.json` names any other commit than `sd.lock.json`.

**Downloads and header reads**
- **Downloads stay pinned.** Every downloaded file keeps its repo, revision, path, size and SHA-256.
  - The SHA-256 is Hugging Face's LFS oid, and the download is verified against it.
  - Gated repos still need the person's Hugging Face token.
  - A file with no published SHA-256 is never offered for a set-up.
- **Header reads are bounded.**
  - A remote header read uses an HTTP Range request against the revision-pinned resolve URL.
  - It never follows a redirect off Hugging Face's own hosts and CDN with the token attached, matching 0.1.x's guarded fetcher.
  - Hard caps:
    - a safetensors header of at most **100 MiB**;
    - at most **64 MiB** of GGUF metadata and tensor table;
    - at most **24 files** read per repo opened.
- **Candidates for a header read:** `.safetensors`, `.sft` and `.gguf` files of at least **10 MB** that aren't obviously companions, plus LoRA-named files of any size (correction 17).
- Studio reads `.safetensors`, `.sft` and `.gguf` files.
  - Pickle formats (`.ckpt`, `.pt`, `.pth`, `.bin`) stay downloadable and are never parsed.
  - The on-screen sentence is "Studio can't read .ckpt files" (or `.pt`, …).

**Running**
- **Engine arguments come only from typed fields**, never a shell.
  - The prompt is passed by file (`--prompt-file`), and so are the LoRA tags.
  - Video recipes run with `-M vid_gen`.
  - Every filled slot passes its flag, from 7.1's table.
  - The adapter slots (`control_net`, `ip_adapter`, `motion_module`, `photo_maker`, `pulid_weights`) put a recipe out of scope.
- **LoRAs:**
  - `--lora-model-dir <Studio LoRA folder>`, with `<lora:NAME:W>` tags appended to the prompt file. For a high-noise part, the tag is `<lora:|high_noise|NAME:W>`.
  - **Strength slider: −1 to 2, step 0.05, default 1.**
  - At most 8 LoRAs per run. Plan decision: a bound against pathological drafts, well above any real use.
  - LoRA names are relative to `--lora-model-dir`, because the engine's LoRA name cannot contain `:` (correction 14).
- **Prompt length:** the 4,000-character local cap and the 32,000-character cloud cap go. **One hidden safety bound of 1 MiB (1,048,576 bytes) stays**, against pathological input.
- **Memory:**
  - `--vae-tiling` for video and for pictures above 1024×1024, unless the recipe disables `vae_tiling`;
  - `--offload-to-cpu` only with *Memory saving: On* or on the out-of-memory retry;
  - the engine's own auto-fit otherwise.

**Spending (Settings → Studio)**
- **Old defaults:** daily limit **on at $10**, ask before every video **on**, ask before runs of **$1** or more **on**, ask when the price is unknown **always**.
- **New defaults:** *Daily limit*, *Ask before every video*, *Ask before runs of $X or more* and *Ask when the price is unknown*, **every switch off**.
- **Upgrade:** prefs still at the old defaults move to the new ones once, through a prefs version marker. Values the person changed are kept.
- **The $5 hold** for unknown prices applies only while a daily limit is on.
- **Kept regardless:** prices are still shown before every cloud run, and the card still says what a run cost.

**Exact wording** (in `src/lib/words.ts`)
- Local warnings:
  - "May not fit this computer: <reason>. It may fail or be very slow."
  - "Video on the processor is very slow"
  - "Installing stops the picture being made"
- File labels: "Works in Studio", "LoRA for <type>", "Extra file", "Can't run in Studio".
- OpenRAIL licences (design 11a): amber "Commercial use allowed, with use rules", with a link to the licence.
- *Set it up yourself* runs carry an "untested set-up" note.

**Kinds and kept rules**
- **Studio kinds:** picture, picture edit (it takes a picture: references, or a picture to change with its mask), video.
- **Input pictures** (design 11a): reference, start frame, end frame, picture to change, mask, control picture. Adapter files (ControlNet, IP-Adapter, PhotoMaker, PuLID, AnimateDiff) stay out.
- **Kept:**
  - the child-safety prompt check;
  - typed engine arguments with no shell;
  - the prompt passed by file;
  - checked output paths;
  - files pinned by size and SHA-256;
  - the API key never sent to a file host;
  - update signatures;
  - the notices, the chat approval rules other than the confirms, and the link confirmation.
  - Chats still start in Ask first unless the person saved another start mode.
- **Upgrading loses nothing.**
  - Models installed with 0.1.x keep working, mapped to recipes.
  - Settings, gallery and runs keep working.
  - Schema 4 can be read by nothing older, as with schema 3.
- **Windows rules stay.**
  - Files that are in use are never replaced or removed.
  - Unicode paths work.
  - Studio never deletes a file outside its models folder. A file imported in place is the person's own.

**Code and project rules**
- **Gates:** `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`, `npm run lint`, `npm run typecheck`, `npm test` and `python3 -I scripts/docs_test.py` all pass after every task, with `bash scripts/check-spawn.sh`.
- **Wording:** all on-screen wording lives in `src/lib/words.ts`, with no raw identifiers on screen: no recipe ids, version words, slot ids or file hashes. Backend error copy stays in `src-tauri/src/studio/errors.rs`, the existing pattern.
- **Accessibility:** keyboard and screen-reader use stay as good as 0.1.x: roles, labels, focus traps and Esc. Every new control has an accessible name.
- **Serde:** a new enum with struct variants uses `#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]`, as `LocalSource` does (`local_models.rs:52`), so its TypeScript type matches field for field.
- **Commands:** each new command:
  - is listed in `src-tauri/commands.list`;
  - is granted as `allow-<kebab>` in `src-tauri/capabilities/main.json`;
  - is registered in `generate_handler!` in `src-tauri/src/lib.rs`, with its autogenerated permission file;
  - calls `commands::only_main_window`;
  - is covered by `commands_list_matches_capabilities`.
- **Processes:** child processes start only through `crates/runtime/src/spawn*` (`scripts/check-spawn.sh`).
- **No new dependencies.**
  - `Cargo.toml`, `package.json` and `package-lock.json` gain no package.
  - The name tokeniser (Task R7) and the 512-entry LRU (Task S5) are hand-written, small enough not to need a crate.
  - The webview's file picker is the existing `@tauri-apps/plugin-dialog`.
- **Scripts:** standard-library Python, run as `python3 -I`. A script's test runs in `ci.yml`, except the fixture capture script (Task R4), which reaches the network and is run by hand.
- **Tests never reach the network.** Remote reads are tested against the local HTTP server pattern of `crates/models/src/download.rs` with `HfClient::for_test`.

## Review Focus

1. **Many quantisations of one model.** A GGUF repo with 15 quantisations of a high-noise and a low-noise file should label every quantisation after one header read per group, and still have read budget left. If it reads one file per quantisation, the 24-file cap labels most files "not read".
   - Test: `one_header_read_labels_every_quantisation_of_a_model` (Task S5).
2. **A gated repo without a Hugging Face token.** Its files should say they need a token, and Download should be refused before any byte or any other part starts. Partial downloads of a set-up would leave companions nobody asked for.
   - Tests: `a_gated_repo_without_a_token_labels_files_not_read` (Task S5) and `a_gated_download_without_a_token_is_refused_before_anything_starts` (Task S7).
3. **A model download that finishes after the person left Get models, or Studio.** The model should be in the model sheet and the card's *Use it* should work. 0.1.2 left it unlisted until Studio was shown again (screenshot critique §2.7 and §6), and the redesign's four-click path depends on *Use it*.
   - Test: `a download finished elsewhere lists the model and offers Use it` (Task U7, vitest).
4. **Windows paths and LoRA names with spaces or Unicode** ("pixel art ü.safetensors", "日本語 スタイル.safetensors"). The LoRA must keep its readable name, drop only `: < > |`, and reach the prompt file as UTF-8 that the engine's pattern matches.
   - Tests: `lora_names_keep_spaces_and_unicode_and_drop_colons_and_angles` (Task S10) and `lora_tags_reach_the_prompt_file_with_unicode_names` (Task S2).
5. **Upgrading from 0.1.2** with installed klein and Wan models, and spending prefs still at the old defaults. Every row should survive with a recipe, a klein picture should get byte-identical arguments, and the prefs should move to off once while a changed value stays.
   - Tests: `v4_maps_each_01x_family_to_its_recipe_and_keeps_every_row` (Task S1), `an_01x_klein_picture_keeps_its_0_1_2_arguments` (recorded from the unchanged code in Task S2, kept green by Task S11) and `old_default_spending_prefs_move_to_off_once_and_changed_ones_stay` (Task G1).

Also pinned, though below these five:
- a host that ignores `Range`: Task R3's `a_server_that_ignores_range_is_read_only_up_to_the_cap`;
- companions shared between models, when one model is deleted: Task S7's `deleting_one_model_keeps_a_companion_a_generic_model_reuses`;
- the narrow 900×650 window: Task U1's harness check that no screen scrolls sideways, made strict in Task U9.

## File structure

**New in `crates/models/src/sd/`** (Plan decision: one module per stage of section 7, so each test file holds one concern)

| File | Responsibility |
|---|---|
| `mod.rs` | Re-exports; `FileFacts`, the per-file result cached by Studio |
| `recipes.rs` | `catalog/sd-recipes.json` parsed: `Recipe`, `SlotId`, `VideoRule`, features |
| `index.rs` | Tensor index of a safetensors or GGUF file, local or from a prefix; engine order and `ne` |
| `remote.rs` | Bounded Range reads of a Hugging Face file's header |
| `detect.rs` | `SdVersion` and the port of `get_sd_version` (two passes, name conversion) |
| `inspect.rs` | Layout, embedded parts, LoRA format and base hint, extra-file kind, guards, diffusers folders, parameter count |
| `matching.rs` | Version → recipe, name tokens, recipe kinds, out-of-scope recipes |
| `companions.rs` | The companion table in `catalog/models.json` |
| `setup.rs` | The set-up planner: slots in 7.4's order |
| `candidates.rs` | Which files of a repo listing to read, quantisation groups, labels by name |
| `testing.rs` | `fixture(name)` and builders (feature `test-support`) |

**New elsewhere**
- `catalog/sd-recipes.json`
- `scripts/vendor_sd_recipes.py` and `scripts/vendor_sd_recipes_test.py`
- `scripts/capture_sd_fixtures.py`
- `crates/models/tests/fixtures/sd/*.json` and `README.md`
- `crates/store/src/gen_loras.rs`
- In `src-tauri/src/studio/`:
  - `detect.rs`: the cache and repo reading;
  - `setup_cmd.rs`: set-up plans and *Set it up yourself*;
  - `import.rs`: model import;
  - `loras.rs`: the LoRA store and commands.
- `scripts/check_strace.py` and `scripts/check_strace_test.py`
- `crates/reverse/src/warmup.rs`
- `crates/runtime/tests/llama_tail.rs`
- `e2e/preview/`: the screenshot harness (Task U1)
- `src/components/ui/`: styled Radix controls (Task U2)
- In `src/components/studio/` (Phase U):
  - `Composer`, `ModelChip`, `ComposerMenus` and `InputZones`;
  - `ModelSheet`, `modelSheet.ts` and `ImportDialog`;
  - `GetModels`, `ModelSearch`, `ModelCard` and `ModelDetail`;
  - `DownloadState` and `DownloadIndicator`;
  - `LorasSection` and `ModelFilesSection`.
- **Deleted in Phase U:** `PromptDock`, `ModelPicker`, `LocalModels`, `LocalModelRow`, `LocalSearch`, `DownloadLocalModel` and `localOffers`, each with its test replaced as its task says.

**Changed**
- `crates/models/src/{gguf,gen_catalog,gen_search,gen_fit,hf,files,lib}.rs`
- `crates/store/src/{schema,gen_models,lib}.rs`
- `crates/runtime/src/sd/{args,run,testing}.rs` and `crates/runtime/src/llama/server.rs`
- `crates/agent/src/media/spec.rs`
- In `src-tauri/src/`:
  - `studio/{local,local_models,local_cmd,manager,manager/lane,prefs,spend,errors,commands}.rs`;
  - `{downloads,dto,hub,approvals,updates,lib}.rs`;
  - `commands.list`, `capabilities/main.json`.
- `src/components/studio/{types,api,StudioView,TopBar,RefTray,SettingsPanel,AdvancedDrawer,ParamField,SettingField,RunCard,results,Grid,EmptyStates,Viewer,Working,styles}.ts(x)`
- `src/store/{studio,studioLocal}.ts`
- `src/components/{AllowAllButton,ApprovalCard,PermissionsSettings,settings/StudioSettings,UpdateBox}.tsx`
- `src/lib/words.ts`
- `.github/workflows/{ci,reverse}.yml`
- `scripts/docs_test.py`
- `ARCHITECTURE.md`, `SECURITY.md`, `README.md`, `CHANGELOG.md`, `THIRD-PARTY-NOTICES.md`
- `e2e/{m8_offline,m12_studio}.py` and `e2e/fake_sd/sd-cli`
- The version files

---

## Phase R: recipes and reading what a file is

### Task R1: The engine's recipes, vendored

**Files:**
- Create:
  - `scripts/vendor_sd_recipes.py`, `scripts/vendor_sd_recipes_test.py`
  - `catalog/sd-recipes.json`
  - `crates/models/src/sd/mod.rs`, `crates/models/src/sd/recipes.rs`
- Modify:
  - `crates/models/src/lib.rs` (`pub mod sd;`)
  - `.github/workflows/ci.yml`: add `- run: python3 -I scripts/vendor_sd_recipes_test.py` after the `release_notes_test.py` step
  - `THIRD-PARTY-NOTICES.md`: one sentence. "`catalog/sd-recipes.json` is copied from stable-diffusion.cpp (MIT) at the pinned commit."
- Test: inline in `recipes.rs`; `scripts/vendor_sd_recipes_test.py`

**Interfaces:**
- Produces (`crates/models/src/sd/recipes.rs`):
```rust
pub const SD_RECIPES: &str = include_str!("../../../../catalog/sd-recipes.json");
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum SlotId { Model, DiffusionModel, HighNoiseDiffusionModel, UncondDiffusionModel, ClipL, ClipG, T5xxl, Llm, LlmVision, ClipVision,
                  Tokenizer, Vae, Tae, AudioVae, AudioEncoder, EmbeddingsConnectors,
                  ControlNet, IpAdapter, MotionModule, PhotoMaker, PulidWeights }
impl SlotId {
    pub const RUNNABLE: [SlotId; 16];                 // the first 16, in 7.1's table order
    pub fn flag(self) -> Option<&'static str>;        // 7.1's table; None for the five adapter slots
    pub fn is_adapter(self) -> bool;
    pub fn word(self) -> &'static str;                // "t5xxl", "clip_l", … (the recipe's own id)
}
#[derive(Debug, Clone, Deserialize)] #[serde(deny_unknown_fields)]
pub struct RecipeSlot { pub id: SlotId, #[serde(default)] pub label: Option<String>, pub required: bool }
#[derive(Debug, Clone, Default, Deserialize)] #[serde(deny_unknown_fields)]
pub struct RecipeDefaults { pub sampling_method: Option<String>, pub scheduler: Option<String>, pub cfg_scale: Option<f32>, pub steps: Option<u32>,
    pub width: Option<u32>, pub height: Option<u32>, pub distilled_guidance: Option<f32>, pub flow_shift: Option<f32>, pub video_frames: Option<u32>,
    pub fps: Option<u32>, #[serde(default)] pub custom_sigmas: Vec<f32>, pub img_cfg_scale: Option<f32>, pub high_noise_steps: Option<i32>,
    pub high_noise_cfg_scale: Option<f32>, pub high_noise_sampling_method: Option<String>, pub high_noise_flow_shift: Option<f32>,
    pub moe_boundary: Option<f32>, pub shifted_timestep: Option<u32>, pub strength: Option<f32> }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)] #[serde(rename_all = "lowercase")]
pub enum FrameRounding { Floor, Ceil }
#[derive(Debug, Clone, Deserialize)] #[serde(deny_unknown_fields)]
pub struct VideoConstraints { pub frame_step: Option<u32>, pub frame_offset: Option<u32>, pub frame_rounding: Option<FrameRounding>,
    pub min_frames: Option<u32>, pub max_frames: Option<u32>, pub fixed_fps: Option<u32> }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct VideoRule { pub frame_step: u32, pub frame_offset: u32, pub rounding: FrameRounding, pub min_frames: u32, pub max_frames: u32, pub fixed_fps: Option<u32> }
#[derive(Debug, Clone, Deserialize)] #[serde(deny_unknown_fields)]
pub struct RecipeContext { pub vae_format: Option<String>, pub model_args: Option<String> }
#[derive(Debug, Clone, Deserialize)] #[serde(deny_unknown_fields)]
pub struct Recipe { pub schema_version: u32, pub id: String, pub name: String, #[serde(default)] pub description: String,
    pub model_url: Option<String>, pub slots: Vec<RecipeSlot>, #[serde(default)] pub required_any: Vec<Vec<SlotId>>,
    pub defaults: RecipeDefaults, #[serde(default)] pub features: Vec<String>, #[serde(default)] pub disabled_features: Vec<String>,
    pub size_multiple: Option<u32>, pub video_constraints: Option<VideoConstraints>, pub context: Option<RecipeContext> }
impl Recipe {
    pub fn size_multiple(&self) -> u32;               // 8 when omitted (api.md)
    pub fn video(&self) -> Option<VideoRule>;         // api.md defaults: step 1, offset 0, floor, min 1, max 4096, fixed_fps None
    pub fn is_video(&self) -> bool;                   // video_constraints present (all 20 video recipes have it; no picture recipe does)
    pub fn feature(&self, name: &str) -> bool;        // see below
    pub fn slot(&self, id: SlotId) -> Option<&RecipeSlot>;
    pub fn main_slot(&self) -> SlotId;                // Model when the recipe has a `model` slot, else DiffusionModel
}
pub fn sd_commit() -> &'static str;
pub fn recipes() -> &'static [Recipe];                // parsed once (OnceLock), sorted by id
pub fn recipe(id: &str) -> Option<&'static Recipe>;
```
`feature` follows `examples/server/api.md`:
- On by default: `img2img`, `mask_image`, `lora`, `vae_tiling`, `cache`, `cache_dit` and `preview`.
- Turned on by a slot: `controlnet` (`control_net`), `ip_adapter`, `high_noise` (`high_noise_diffusion_model`) and `preview_tae` (`tae`).
- Otherwise on when listed in `features`.
- `disabled_features` always wins.

**Steps**

- [ ] **Step 1: Write the failing tests.**

  `scripts/vendor_sd_recipes_test.py` has three cases. The first two make a temporary git repo (`git init`, two recipe files plus `README.md` and `defaults.md` under `examples/server/recipes/`, one commit) and run the script against it:
  - `test_writes_every_recipe_of_the_commit_sorted`: `--commit <sha> --out <tmp>` gives:
    - `sdCommit == sha`;
    - `[r["id"] for r in recipes] == ["a-recipe", "b-recipe"]`;
    - each recipe dict equal to the committed JSON;
    - no README.
  - `test_reads_the_commit_not_the_working_tree`: a recipe changed in the working tree but not committed is written as committed.
  - `test_the_vendored_file_matches_the_lock`, on the real files:
    - `json.load(open("catalog/sd-recipes.json"))["sdCommit"] == json.load(open("sd.lock.json"))["sd_cpp"]["commit"]`;
    - `len(recipes) == 112`.

  In `recipes.rs`:
```rust
#[test] fn the_recipes_name_the_pinned_engine_commit() {
    let lock: Value = serde_json::from_str(include_str!("../../../../sd.lock.json")).unwrap();
    assert_eq!(sd_commit(), lock["sd_cpp"]["commit"].as_str().unwrap());
    assert_eq!(sd_commit(), "532d2c15c430a785a712b2d94281f04e733008a4");
}
#[test] fn every_recipe_parses_with_known_fields() {
    assert_eq!(recipes().len(), 112);
    assert!(recipes().iter().all(|r| r.schema_version == 1));
    assert!(recipes().iter().all(|r| r.slots.iter().any(|s| s.required && matches!(s.id, SlotId::Model | SlotId::DiffusionModel))));
}
#[test] fn slots_map_to_the_flags_of_the_design() {
    assert_eq!(SlotId::RUNNABLE.map(|s| s.flag().unwrap()), ["-m", "--diffusion-model", "--high-noise-diffusion-model",
        "--uncond-diffusion-model", "--clip_l", "--clip_g", "--t5xxl", "--llm", "--llm_vision", "--clip_vision", "--tokenizer",
        "--vae", "--tae", "--audio-vae", "--audio-encoder", "--embeddings-connectors"]);
    for s in [SlotId::ControlNet, SlotId::IpAdapter, SlotId::MotionModule, SlotId::PhotoMaker, SlotId::PulidWeights] { assert!(s.is_adapter() && s.flag().is_none()); }
}
#[test] fn size_multiple_defaults_to_8() {
    assert_eq!(recipe("sdxl-checkpoint").unwrap().size_multiple(), 64);
    assert_eq!(recipe("flux1-dev").unwrap().size_multiple(), 8);
    assert_eq!(recipe("chroma-radiance").unwrap().size_multiple(), 1);
}
#[test] fn video_rules_default_like_the_engine() {
    assert_eq!(recipe("wan22-t2v").unwrap().video(), Some(VideoRule { frame_step: 4, frame_offset: 1, rounding: FrameRounding::Floor, min_frames: 1, max_frames: 4096, fixed_fps: None }));
    assert_eq!(recipe("minimax-h3-fl2va").unwrap().video().unwrap().fixed_fps, Some(24));
    assert_eq!(recipes().iter().filter(|r| r.is_video()).count(), 20);
    assert!(recipe("flux1-dev").unwrap().video().is_none());
}
#[test] fn features_follow_the_api_defaults() {
    let wan = recipe("wan22-t2v").unwrap();
    assert!(wan.feature("lora") && wan.feature("high_noise") && !wan.feature("img2img"));
    assert!(!recipe("z-image-l2p").unwrap().feature("vae_tiling"));
    assert!(recipe("sdxl-checkpoint").unwrap().feature("vae_tiling"));
    assert_eq!(recipe("wan22-t2v").unwrap().main_slot(), SlotId::DiffusionModel);
    assert_eq!(recipe("sdxl-checkpoint").unwrap().main_slot(), SlotId::Model);
}
```

- [ ] **Step 2: Run them and see them fail.**
  - Run: `python3 -I scripts/vendor_sd_recipes_test.py; cargo test -p models --lib sd::recipes`
  - Expected: FAIL with `can't open file 'scripts/vendor_sd_recipes.py'`, and a compile error in `sd::recipes`.

- [ ] **Step 3: Implement.**
  - `scripts/vendor_sd_recipes.py <checkout> [--commit SHA] [--out PATH]`:
    - `--commit` defaults to `sd.lock.json`'s `sd_cpp.commit`, and `--out` to `catalog/sd-recipes.json`.
    - It lists `git -C <checkout> ls-tree --name-only <commit> examples/server/recipes/`, keeping only `*.json`.
    - It reads each file with `git -C <checkout> show <commit>:<path>`, so a blobless clone works and the working tree is never read.
    - It parses each, then writes `{"sdCommit": <commit>, "recipes": [...]}` sorted by `id`, with `json.dumps(…, indent=2, ensure_ascii=False) + "\n"`.
    - It runs nothing else.
  - Generate the real file from a checkout of the pinned commit. Any blobless clone works, for example `git clone --filter=blob:none https://github.com/leejet/stable-diffusion.cpp <dir>`:
    - Run: `python3 -I scripts/vendor_sd_recipes.py <dir>`
    - Expected: `wrote 112 recipes for 532d2c15c430a785a712b2d94281f04e733008a4`.
  - Rust: `recipes()` parses `SD_RECIPES` once with `serde_json` and panics on a parse error, because the file is compiled in and a broken file must not ship. `deny_unknown_fields` makes a future pin bump fail the test on any new recipe key.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `python3 -I scripts/vendor_sd_recipes_test.py && cargo test -p models --lib sd::recipes`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add scripts/vendor_sd_recipes.py scripts/vendor_sd_recipes_test.py catalog/sd-recipes.json crates/models/src/sd crates/models/src/lib.rs .github/workflows/ci.yml THIRD-PARTY-NOTICES.md
git commit -m "Studio: vendor the pinned engine's 112 recipes, checked against sd.lock.json"
```

### Task R2: The tensor index of a local file

**Files:**
- Create: `crates/models/src/sd/index.rs`
- Modify: `crates/models/src/gguf.rs`:
  - add `GgufError::Truncated { need: u64 }`;
  - add `tensor_table_from_prefix`;
  - add `ggml_type_name`.
- Test: inline

**Interfaces:**
- Produces:
```rust
// gguf.rs
pub fn tensor_table_from_prefix(bytes: &[u8]) -> Result<(Vec<RawTensor>, BTreeMap<String, String>), GgufError>; // string-valued KVs only; Truncated{need} when `bytes` ends inside the header or table
pub fn ggml_type_name(id: u32) -> String;   // ggml's type_traits names at the ggml commit in sd.lock.json (eeda9ec…): "f32", "f16", "q4_0", "q8_0", "q4_K", "bf16", …; unknown ids "type<N>"
// sd/index.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "lowercase")]
pub enum FileFormat { Safetensors, Gguf }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TensorInfo { pub name: String, pub dtype: String, /** As written in the file: safetensors outermost first, GGUF `ne` order. */ pub shape: Vec<u64> }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TensorIndex { pub format: FileFormat, /** GGUF: file order; safetensors: sorted by name. */ pub tensors: Vec<TensorInfo>, pub metadata: BTreeMap<String, String> }
#[derive(Debug, thiserror::Error)]
pub enum IndexError { #[error("not a weights file")] NotWeights, #[error("header larger than {cap} bytes")] TooLarge { cap: u64 },
    #[error("needs {need} bytes")] Truncated { need: u64 }, #[error("corrupt header: {0}")] Corrupt(String),
    #[error("a pickle file")] Pickle, #[error(transparent)] Io(#[from] std::io::Error) }
pub const SAFETENSORS_HEADER_CAP: u64 = 100 * 1024 * 1024;   // 100 MiB
pub const GGUF_HEADER_CAP: u64 = 64 * 1024 * 1024;           // 64 MiB
pub const PICKLE_EXTENSIONS: [&str; 4] = [".ckpt", ".pt", ".pth", ".bin"];
pub const READ_EXTENSIONS: [&str; 3] = [".safetensors", ".sft", ".gguf"];
pub fn sniff(bytes: &[u8]) -> Option<FileFormat>;                      // "GGUF" magic, or a u64 LE length followed by '{'
pub fn parse_prefix(bytes: &[u8]) -> Result<TensorIndex, IndexError>;  // Truncated{need} when more bytes are needed; TooLarge past the caps
pub fn read_local(path: &Path) -> Result<TensorIndex, IndexError>;     // reads only the header; Pickle by extension, without opening
#[derive(Debug, Clone, PartialEq, Eq)] pub struct ShardIndex { pub shards: Vec<String> }   // distinct files of weight_map, sorted
pub fn parse_shard_index(json: &[u8]) -> Result<ShardIndex, IndexError>;
impl TensorIndex {
    pub fn engine_order(&self) -> Vec<&TensorInfo>;   // safetensors: sorted by name; GGUF: file order
    pub fn parameters(&self) -> u64;                  // Σ product of shape over all tensors
}
pub fn engine_ne(t: &TensorInfo, format: FileFormat) -> [u64; 4];
```
- **Safetensors:**
  - The engine order follows `safetensors_io.cpp`: the header is a `nlohmann::json`, a `std::map`, so names are visited sorted.
  - Every tensor is kept, whatever its dtype. Plan decision: the engine maps dtypes and repacks `U8` ones (`safetensors_io.cpp:79-110, 232, 285`), but names decide detection, and the few shape-checked tensors are never `U8` in published files.
  - For `engine_ne`, a 5-D shape `[d0, d1, d2, d3, d4]` is first folded to `[d0*d1, d2, d3, d4]` (`safetensors_io.cpp:330-344`). Then the shape is reversed and padded with 1 to four values.
- **GGUF:**
  - Dimensions past the fourth multiply into `ne[3]` (`gguf_reader_ext.h:171-178`).
  - `metadata` keeps the string-valued KVs, among them `general.architecture` and `general.name`.
  - A safetensors `__metadata__` keeps its string values only.

**Steps**

- [ ] **Step 1: Write the failing tests.** They use a `safetensors(header: &Value)` builder in `sd/testing.rs` and the existing `gguf::testing::Writer`.
```rust
#[test] fn a_safetensors_header_is_read_and_visited_in_engine_order() {
    let ix = parse_prefix(&safetensors(&json!({"b": {"dtype": "F16", "shape": [2, 3], "data_offsets": [0, 12]},
        "a": {"dtype": "BF16", "shape": [4], "data_offsets": [12, 20]}, "__metadata__": {"ss_base_model_version": "sdxl_base_v1-0", "n": 3}}))).unwrap();
    assert_eq!(ix.engine_order().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(engine_ne(&ix.tensors.iter().find(|t| t.name == "b").unwrap(), FileFormat::Safetensors), [3, 2, 1, 1]);
    assert_eq!(ix.metadata.get("ss_base_model_version").map(String::as_str), Some("sdxl_base_v1-0"));
    assert!(!ix.metadata.contains_key("n"));
}
#[test] fn a_5d_safetensors_shape_folds_like_the_engine() {
    let t = TensorInfo { name: "x".into(), dtype: "F16".into(), shape: vec![3072, 48, 1, 2, 2] };
    assert_eq!(engine_ne(&t, FileFormat::Safetensors), [2, 2, 1, 147456]);
}
#[test] fn a_gguf_5d_tensor_stacks_into_ne3() {
    let t = TensorInfo { name: "x".into(), dtype: "f16".into(), shape: vec![2, 2, 1, 48, 3072] };
    assert_eq!(engine_ne(&t, FileFormat::Gguf), [2, 2, 1, 147456]);
}
#[test] fn a_header_over_the_cap_is_refused_before_it_is_read() {
    let mut b = (SAFETENSORS_HEADER_CAP + 1).to_le_bytes().to_vec(); b.push(b'{');
    assert!(matches!(parse_prefix(&b), Err(IndexError::TooLarge { cap: SAFETENSORS_HEADER_CAP })));
}
#[test] fn a_short_prefix_asks_for_exactly_what_it_needs() {
    let file = safetensors(&big_header(2_000));                 // header JSON of 2,000 bytes
    assert!(matches!(parse_prefix(&file[..100]), Err(IndexError::Truncated { need: 2_008 })));
    let g = gguf::testing::diffusion_gguf();                    // a whole small GGUF
    assert!(matches!(parse_prefix(&g[..64]), Err(IndexError::Truncated { .. })));
}
#[test] fn pickle_files_are_named_and_never_opened() {
    assert!(matches!(read_local(Path::new("/does/not/exist/model.ckpt")), Err(IndexError::Pickle)));
}
#[test] fn unicode_and_spaces_in_paths_read() {
    let dir = tempfile::tempdir().unwrap(); let p = dir.path().join("ü dir").join("モデル 1.safetensors");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap(); std::fs::write(&p, safetensors(&json!({}))).unwrap();
    assert_eq!(read_local(&p).unwrap().format, FileFormat::Safetensors);
}
#[test] fn a_sharded_index_lists_its_shards() {
    let s = parse_shard_index(br#"{"weight_map": {"a": "m-00002-of-00002.safetensors", "b": "m-00001-of-00002.safetensors", "c": "m-00001-of-00002.safetensors"}}"#).unwrap();
    assert_eq!(s.shards, ["m-00001-of-00002.safetensors", "m-00002-of-00002.safetensors"]);
}
#[test] fn ggml_type_names_follow_the_pinned_ggml() {
    assert_eq!((ggml_type_name(0), ggml_type_name(1), ggml_type_name(2), ggml_type_name(12), ggml_type_name(30), ggml_type_name(999)),
        ("f32".into(), "f16".into(), "q4_0".into(), "q4_K".into(), "bf16".into(), "type999".into()));
}
```

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p models --lib sd::index gguf::`
  - Expected: FAIL to compile, with `unresolved import` in `sd::index`.

- [ ] **Step 3: Implement.**
  - `parse_prefix` sniffs the format.
  - **Safetensors:**
    - Needs `8 + len` bytes.
    - Refuses `len > SAFETENSORS_HEADER_CAP` before asking for more.
    - Parses with `serde_json` into a `Map`. The workspace does not enable `preserve_order`, so the map is a `BTreeMap` and `tensors` holds the names sorted, which is the engine's own order.
  - **GGUF:**
    - `tensor_table_from_prefix` reuses `read_checked_table` and `read_raw_tensors`.
    - It reports `Truncated { need }` with the byte count reached plus one table entry, or plus 1 MiB when that is unknown.
    - It refuses when the table would end past `GGUF_HEADER_CAP`.
    - `MAX_TENSORS` is raised from 65,536 to 262,144. Plan decision: LTX-2.3 has 4,444 tensors and LoRA files reach tens of thousands; the 64 MiB cap stays the real bound.
  - `read_local` opens the file, reads up to the cap with the same parser, and never reads tensor data.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p models --lib sd::index gguf::`
  - Expected: PASS. Existing `gguf::` tests are unchanged.

- [ ] **Step 5: Commit.**
```bash
git add crates/models/src/sd crates/models/src/gguf.rs
git commit -m "Studio: read safetensors and GGUF tensor tables, in the engine's order and shapes"
```

### Task R3: Bounded remote header reads

**Files:**
- Create: `crates/models/src/sd/remote.rs`
- Test: inline, with a local HTTP server modelled on `download.rs`'s `server(body, sha, cut_after)`.

**Interfaces:**
- Consumes: `HfClient::fetch_head(repo, commit, file, max_bytes) -> Result<Vec<u8>, HfError>` (`hf.rs:1073`). It does a Range read `bytes=0-N` through `resolve` (the guarded fetcher), and stops reading at `max_bytes` when a server ignores Range. `parse_prefix` comes from Task R2.
- Produces:
```rust
pub const FIRST_READ: u64 = 1 << 20;                 // Plan decision: 1 MiB covers most GGUF tables and safetensors headers in one request
pub const MAX_READS_PER_FILE: u32 = 4;
pub const MAX_FILES_PER_REPO: usize = 24;
#[derive(Debug, thiserror::Error)]
pub enum RemoteError { #[error(transparent)] Hf(#[from] HfError), #[error(transparent)] Index(#[from] IndexError) }
pub async fn read_remote(hf: &HfClient, repo: &str, commit: &str, path: &str, size: u64) -> Result<TensorIndex, RemoteError>;
```
- The algorithm:
  1. Read `min(FIRST_READ, size)` bytes and parse.
  2. On `Truncated { need }`, read again from byte 0 with `max_bytes = min(need, size)`. Plan decision: from 0, which keeps `fetch_head` unchanged and costs at most the prefix again.
  3. Stop after `MAX_READS_PER_FILE` reads, or when `need` passes the format's cap (`TooLarge`).
- A gated file's `HfError::NeedsToken` or `NeedsTerms` passes through unchanged. Task S4 turns it into a label.

**Steps**

- [ ] **Step 1: Write the failing tests.** The test server records each request's `Range` header and counts the bytes it managed to send.
```rust
#[tokio::test] async fn a_header_bigger_than_the_first_read_takes_one_more_request() {
    let file = safetensors(&big_header(2_000_000));
    let s = range_server(file.clone(), RangeMode::Honour).await;
    let ix = read_remote(&s.client(), "o/r", COMMIT, "m.safetensors", file.len() as u64).await.unwrap();
    assert_eq!(ix.format, FileFormat::Safetensors);
    assert_eq!(s.ranges(), ["bytes=0-1048575", "bytes=0-2000007"]);
}
#[tokio::test] async fn a_server_that_ignores_range_is_read_only_up_to_the_cap() {
    // a 300 MiB body whose header says 200 MiB: the client stops after the first 1 MiB and refuses
    let s = range_server(streamed_safetensors(200 << 20, 300 << 20), RangeMode::Ignore).await;
    let e = read_remote(&s.client(), "o/r", COMMIT, "m.safetensors", 300 << 20).await.unwrap_err();
    assert!(matches!(e, RemoteError::Index(IndexError::TooLarge { cap: SAFETENSORS_HEADER_CAP })));
    assert_eq!(s.ranges().len(), 1);
    assert!(s.bytes_sent() <= FIRST_READ + (4 << 20), "sent {}", s.bytes_sent());   // one socket buffer of slack
    // a 2 MB header behind a server that ignores Range still reads, in two bounded requests
    let file = safetensors(&big_header(2_000_000));
    let s = range_server(file.clone(), RangeMode::Ignore).await;
    assert!(read_remote(&s.client(), "o/r", COMMIT, "m.safetensors", file.len() as u64).await.is_ok());
}
#[tokio::test] async fn a_gguf_table_past_64_mib_is_refused() {
    let s = range_server(gguf_claiming_table_bytes(65 << 20), RangeMode::Honour).await;
    assert!(matches!(read_remote(&s.client(), "o/r", COMMIT, "m.gguf", 70 << 20).await,
        Err(RemoteError::Index(IndexError::TooLarge { cap: GGUF_HEADER_CAP }))));
}
#[tokio::test] async fn a_gated_file_passes_the_hub_error_through() {
    let s = status_server(401).await;
    assert!(matches!(read_remote(&s.client(), "o/r", COMMIT, "m.gguf", 10).await, Err(RemoteError::Hf(HfError::NeedsToken))));
}
#[tokio::test] async fn a_redirect_off_the_allowed_hosts_is_never_followed() {
    let s = redirect_server("https://evil.example/m.gguf").await;
    assert!(matches!(read_remote(&s.client(), "o/r", COMMIT, "m.gguf", 10).await, Err(RemoteError::Hf(HfError::BlockedHost(_)))));
}
```

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p models --features test-support --lib sd::remote`
  - Expected: FAIL to compile.

- [ ] **Step 3: Implement `read_remote`** as specified, with the test helpers in `sd/testing.rs`.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p models --features test-support --lib sd::remote`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add crates/models/src/sd
git commit -m "Studio: read a Hugging Face file's header with bounded Range requests"
```

### Task R4: Detection fixtures from real Hugging Face files

**Files:**
- Create:
  - `scripts/capture_sd_fixtures.py`: committed, standard library, run by hand, not in CI.
  - `crates/models/tests/fixtures/sd/<name>.json` (25 files) and `crates/models/tests/fixtures/sd/README.md`.
  - `fixture(name: &str) -> TensorIndex` in `crates/models/src/sd/testing.rs`.
- Test: inline in `sd/testing.rs`

**Interfaces:**
- Produces: `pub fn fixture(name: &str) -> TensorIndex` and `pub fn fixture_source(name: &str) -> FixtureSource { repo, revision, path, size, sha256, captured_on }`, both under `#[cfg(any(test, feature = "test-support"))]`.
- The fixture JSON format:
```json
{"source": {"repo": "…", "revision": "<40 hex>", "path": "…", "size": 0, "sha256": "<64 hex>", "capturedOn": "2026-11-24"},
 "format": "safetensors", "metadata": {"…": "…"}, "tensors": [["name", "dtype", [dims as written in the file]]], "parameters": 0}
```
- `parameters` is the full file's `TensorIndex::parameters()`, before trimming, so Task R6's parameter labels can be tested.
- **The trim rule:**
  - Keep every tensor whose name contains a marker from Task R5's detection table, Task R6's tables or Task R7's facts.
  - Keep the first tensor of each top-level name prefix (the text before the first `.`), and the first of each numbered block (`…blocks.N.` for the lowest and highest N).
  - In LoRA files, also keep every `attn2_to_k`, `lora_te2_` and `te2.` name.
  - Keep `__metadata__` keys: `modelspec.*`, `ss_base_model_version`, `ss_network_module`, `general.architecture` and `general.name`.

**The fixtures.** All are ungated, checked on 2026-11-24.

| Fixture | Repo @ revision | File | Size | SHA-256 |
|---|---|---|---|---|
| `sd15` | stable-diffusion-v1-5/stable-diffusion-v1-5 @ 451f4fe16113bff5a5d2269ed5ad43b0592e9a14 | v1-5-pruned-emaonly.safetensors | 4265146304 | 6ce0161689b3853acaa03779ec93eafe75a02f4ced659bee03f50797806fa2fa |
| `sdxl` | stabilityai/stable-diffusion-xl-base-1.0 @ 462165984030d82259a11f4367a4eed129e94a7b | sd_xl_base_1.0.safetensors | 6938078334 | 31e35c80fc4829d14f90153f4c74cd59c90b779f6afe05a74cd6120b893f7e5b |
| `sd35-medium` | second-state/stable-diffusion-3.5-medium-GGUF @ 58b78c305a43ddfcffe1ab54d7022995f61667ac | sd3.5_medium.safetensors | 5107104286 | 11fe06e22364b823dfeedc275912336b932b32a293a0b2f35ffac071990cc4de |
| `flux1-dev` | Comfy-Org/Flux-1-Dev @ 10cf201a3d49f7ff4de84d6a97d6a32cdfa8cb86 | flux1-dev.safetensors | 23802932552 | 4610115bb0c89560703c892c59ac2742fa821e60ef5871b33493ba544683abd7 |
| `flux1-kontext` | QuantStack/FLUX.1-Kontext-dev-GGUF @ 2d083027732f81e5548620b57ac47da22d30aa5a | flux1-kontext-dev-Q4_K_M.gguf | 6931817760 | ebd4e92c44c47f104ad64e4353692f37addf06eb99420c9b459334d871c8b750 |
| `chroma` | silveroxides/Chroma1-HD-GGUF @ d3b77bf4eb5b84c0b50e9a1e83f6e8074ed85f82 | Chroma1-HD-Q4_0.gguf | 5432053920 | 1aa9f3c059a415320bb9b8b6231ea14e118df26653b5bb937c92e00cf0cc127e |
| `qwen-image` | QuantStack/Qwen-Image-GGUF @ 257f261fa92593bed760aa6fa3f7921a49fea00f | Qwen_Image-Q4_0.gguf | 11852773920 | ab0f20d548f3bfb197ec0f0f61c15acc2f0f865495fee2941c658fef4391cd61 |
| `qwen-image-edit-2509` | QuantStack/Qwen-Image-Edit-2509-GGUF @ 84a3006979126011422eeeefe0c9485ddf431ef5 | Qwen-Image-Edit-2509-Q4_0.gguf | 11928271392 | 4f6cda402e1dbc36ee4b601b10b9ee0da2dbefedfbfa53eae3efb0ddff48c3e2 |
| `z-image-turbo` | Comfy-Org/Z-Image-Turbo @ 6fc90a3b1b653e935a0d175e260736de25b84df5 | split_files/diffusion_models/z_image_turbo_bf16.safetensors | 12309866400 | 2407613050b809ffdff18a4ac99af83ea6b95443ecebdf80e064a79c825574a6 |
| `flux2-klein-4b` | black-forest-labs/FLUX.2-klein-4B @ e7b7dc27f91deacad38e78976d1f2b499d76a294 | flux-2-klein-4b.safetensors | 7751105712 | ec3d4e733a771f61c052fb4856c48b336c55eaf2c65487c2a1faeb9bbda7a343 |
| `wan21-t2v-1.3b` | Comfy-Org/Wan-2.1 @ d6258323e18f14c60d37899da97ca555f3118259 | split_files/diffusion_models/wan2.1_t2v_1.3B_fp16.safetensors | 2838303560 | be531024cd9018cb5b48c40cfbb6a6191645b1c792eb8bf4f8c1c6e10f924dc5 |
| `wan21-i2v-14b` | city96/Wan2.1-I2V-14B-480P-gguf @ b4b6b3b3f0f64975fb33b1d5d1a46ca24e065f03 | wan2.1-i2v-14b-480p-Q4_0.gguf | 10247552384 | cc572fdf84417905550363f16c53f20fd8d5aee046e1562d2546fae7222c2a0a |
| `wan22-ti2v-5b` | QuantStack/Wan2.2-TI2V-5B-GGUF @ 57437632ddd08bdcbd1508c866aa22e126ed51d2 | Wan2.2-TI2V-5B-Q4_K_M.gguf | 3433116000 | 95b19697b7f98e65b0a543640e9ca7b4dfec32e2a6e3731e8e10708be52655e2 |
| `wan22-t2v-a14b-high` | QuantStack/Wan2.2-T2V-A14B-GGUF @ 73eafba53a1a8f29254e4c77f92e74ea27d7cd6f | HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf | 9650090496 | e0c490c6e316fd91ff52034e4ca66b825717e33ff11624585c0ccfcb5d410c59 |
| `wan22-t2v-a14b-low` | QuantStack/Wan2.2-T2V-A14B-GGUF @ 73eafba53a1a8f29254e4c77f92e74ea27d7cd6f | LowNoise/Wan2.2-T2V-A14B-LowNoise-Q4_K_M.gguf | 9650090496 | 091a5bae02e14aa016bc9b10a7892efda4c629346b81c5dcebbe30ea2ac8923a |
| `hunyuan-video-15` | Comfy-Org/Hunyuan-Video-1.5 @ 1405a2b738c29b890cf6d03fc038d8d3e92eeed7 | split_files/diffusion_models/hunyuanvideo1.5_720p_t2v_fp16.safetensors | 16653368128 | 4f05e2a31b51b7ddee3ef336d234e0932001cbdc85887da15451069fda3e1d05 |
| `ltx23` | unsloth/LTX-2.3-GGUF @ 96e8ed4925ead3db9ff4d0084f165ef6a74f28d0 | ltx-2.3-22b-dev-Q4_K_M.gguf | 14326856736 | e053e3d7827f3a69ecd00e55395d3a8f8616ab10d3a394e8d2b65ae204d490e0 |
| `sdxl-lora-pixel-art` | nerijs/pixel-art-xl @ 8bf4a4d9ea283e00a51fafda8e0539f8248ea037 | pixel-art-xl.safetensors | 170543052 | 4234637cb80c998f41e348e6a6cb6bc20d8d038b2b0f256b6129b3b5e353eef7 |
| `flux-lora-realism` | XLabs-AI/flux-lora-collection @ 9ddb42ad970b5d31f50ac02d223adea81413ba4c | realism_lora_comfy_converted.safetensors | 22433528 | 379e73dccfb57822ee3b12f374e141ce1c79a13b7ff19da4219ef2a2a610038e |
| `lumina2` | Comfy-Org/Lumina-Image-2 @ 5b072540ef86570fecb8249c505f23d5bdeb88cd | split_files/diffusion_models/lumina_2_model_bf16.safetensors | 5219585680 | 736e7a524186c943568dfe688d87ce3556cfbd97f7026125065b0aa44c351f6a |
| `sd14-diffusers-unet` | CompVis/stable-diffusion-v1-4 @ 133a221b8aa7292a167afc5127cb63fb5005638b | unet/diffusion_pytorch_model.safetensors | 3438167534 | 145a07e0f05ec5bbe6e2e9faf608bdb311caf708895cac8c8ed713c59864e1e8 |
| `sd14-diffusers-text-encoder` | CompVis/stable-diffusion-v1-4 @ 133a221b8aa7292a167afc5127cb63fb5005638b | text_encoder/model.safetensors | 492265879 | 7b3a12df205cb3c74dd4eae4354d93f606ae6b3bc29d5d06fd97921cb9ad8a81 |
| `ae-flux1` | lodestones/Chroma @ 2f3b2730d7b5edbd02cdff12f72610af5787300b | ae.safetensors | 335304388 | afc8e28272cd15db3919bacdb6918ce9c1ed22e96cb12c4d5ed0fba823529e38 |
| `clip-l` | comfyanonymous/flux_text_encoders @ 6af2a98e3f615bdfa612fbd85da93d1ed5f69ef5 | clip_l.safetensors | 246144152 | 660c6f5b1abae9dc498ac2d21e1347d2abdb0cf6c0c0c8576cd796491d9a6cdd |
| `umt5-xxl-q5km` | city96/umt5-xxl-encoder-gguf @ b535255bee98c2b0a59ea7c0ae2dcd0c6657b3b7 | umt5-xxl-encoder-Q5_K_M.gguf | 4145878880 | eaea358bb438c5d211721a4feecc162000e3636e9cb96f51e216f1f44ebd12ce |

The existing `crates/models/tests/fixtures/gguf/*.txt` files stay and are also used by Task R5:
- `klein4b`, `klein9b`, `zimage`, `wan22-ti2v-5b` (FastWan) and `wan21-t2v-1.3b`;
- `flux3-klein` and `qwen3-4b`.

**Steps**

- [ ] **Step 1: Write the failing test.**
```rust
#[test] fn every_fixture_names_its_pinned_source() {
    for name in FIXTURES {                                  // the 25 names of the table, in order
        let s = fixture_source(name);
        assert!(s.revision.len() == 40 && s.revision.bytes().all(|b| b.is_ascii_hexdigit()), "{name}");
        assert!(s.sha256.len() == 64 && s.size > 0, "{name}");
        assert!(!fixture(name).tensors.is_empty(), "{name}");
    }
    assert_eq!(fixture_source("lumina2").sha256, "736e7a524186c943568dfe688d87ce3556cfbd97f7026125065b0aa44c351f6a");
}
```

- [ ] **Step 2: Run it and see it fail.**
  - Run: `cargo test -p models --features test-support --lib sd::testing`
  - Expected: FAIL, because `fixture_source` is not defined.

- [ ] **Step 3: Write `scripts/capture_sd_fixtures.py`.**
  - Usage: `python3 -I scripts/capture_sd_fixtures.py [--only NAME] [--out DIR]`. `--out` defaults to `crates/models/tests/fixtures/sd`.
  - `FIXTURES` holds the table above as data.
  - For each fixture:
    1. Read `https://huggingface.co/api/models/<repo>/paths-info/<revision>` (POST `paths=<path>`, `expand=true`), and refuse unless the LFS `oid` and `size` equal the table's.
    2. Range-read the header from `https://huggingface.co/<repo>/resolve/<revision>/<path>` with the Task R2 caps. The safetensors header is length plus JSON; the GGUF header is read incrementally, as a port of the Rust reader.
    3. Trim by the rule, and write the JSON with `indent=1`.
  - It sends a generic `User-Agent: harnessui-fixture-capture/1` and no token.
  - It writes `README.md`: the table, the trim rule, and "Captured with scripts/capture_sd_fixtures.py on <date>; not run in CI."
  - The downloaded headers are untrusted data: parsed as JSON or bytes only.
  - Run it once:
    - Run: `python3 -I scripts/capture_sd_fixtures.py`
    - Expected: `25 fixtures written`. Each file is under 200 KB.

- [ ] **Step 4: Run the test and see it pass.**
  - Run: `cargo test -p models --features test-support --lib sd::testing`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add scripts/capture_sd_fixtures.py crates/models/tests/fixtures/sd crates/models/src/sd/testing.rs
git commit -m "Studio: tensor-list fixtures from 25 real Hugging Face files, and the script that captured them"
```

### Task R5: The engine's version detection, ported

**Files:**
- Create: `crates/models/src/sd/detect.rs`
- Test: inline. It uses the Task R4 fixtures and the 0.1.x `fixtures/gguf/*.txt`, through `gguf::testing::shapes`.

**Interfaces:**
- Consumes: `TensorIndex`, `engine_order` and `engine_ne` (Task R2); `SlotId` (Task R1).
- Produces:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SdVersion { /* the 60 below, each with #[serde(rename = "<word>")] */ }
impl SdVersion {
    pub const ALL: [SdVersion; 60];
    pub fn word(self) -> &'static str;
    pub fn from_word(word: &str) -> Option<SdVersion>;
    pub fn is_unet(self) -> bool;          // model.h:123-130: sd1*, sd2*, sdxl* (incl. sdxs-512-ds in sd1, sdxs-09 in sd2)
    pub fn is_flux(self) -> bool;          // model.h:139-147: flux, flux-fill, flux-controls, flex-2, ovis-image, chroma, chroma-radiance
    pub fn lora_family(self) -> SdVersion; // sd1* → sd1; sd2* → sd2; sdxl* → sdxl; flux, flex-2, chroma, chroma-radiance, flux-fill, flux-controls → flux;
                                           // wan2.2-i2v → wan2; z-image-l2p → z-image; qwen-image-layered, qwen-image-2.1 → qwen-image; others themselves
}
pub struct LoadedFile<'a> { pub slot: SlotId, pub index: &'a TensorIndex }
pub const LOAD_ORDER: [SlotId; 20];       // the order below (every slot but tokenizer)
pub fn slot_prefix(slot: SlotId) -> &'static str;
pub fn detect_version(files: &[LoadedFile<'_>]) -> Option<SdVersion>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum FolderPart { Unet, Vae, TextEncoder, TextEncoder2 }
pub fn detect_folder(parts: &[(FolderPart, &TensorIndex)]) -> Option<SdVersion>;   // prefixes "unet.", "vae.", "te.", "te.1." (model_loader.cpp:421-441)
```

**The version words.** Plan decision: kebab-case of the enum name, with the engine's dots kept. None collides with a 0.1.x `GenFamily` word.

- **SD and SDXL:** sd1, sd1-inpaint, sd1-pix2pix, sd1-tiny-unet, sd2, sd2-inpaint, sd2-tiny-unet, sdxs-512-ds, sdxs-09, sdxl, sdxl-inpaint, sdxl-pix2pix, sdxl-vega, sdxl-ssd1b, svd, sd3.
- **FLUX and Chroma:** flux, flux-fill, flux-controls, flex-2, chroma, chroma-radiance.
- **Video:** wan2, wan2.2-i2v, wan2.2-ti2v, wan2.2-s2v, lingbot-video, hunyuan-video, ltxav, minimax-h3.
- **Qwen:** qwen-image, qwen-image-layered, qwen-image-2.1.
- **Others:** anima, flux2, flux2-klein, hidream-o1, z-image, boogu-image, ovis-image, ernie-image, lens, minit2i, longcat, pid, ideogram4, sefi-image, krea2, mage-flow, sensenova-u1.5, llada-image, pixart, ming-image, z-image-l2p, iris, flux3, moss-image, fibo2, p-h3-image, minimax-h3-image.

**Load order and prefixes** (`src/pipeline/diffusion_engine.cpp:691-836`). `LOAD_ORDER` is this order. The tokenizer is not a tensor file and is never loaded into the catalog.

| # | Slot | Prefix |
|---|---|---|
| 1 | model | `""` |
| 2 | diffusion_model | `model.diffusion_model.` |
| 3 | high_noise_diffusion_model | `model.high_noise_diffusion_model.` |
| 4 | uncond_diffusion_model | `model.diffusion_model.uncond.` |
| 5 | clip_l | `clip_l.` |
| 6 | clip_g | `clip_g.` |
| 7 | clip_vision | `clip_vision.` |
| 8 | t5xxl | `text_encoders.t5xxl.transformer.` |
| 9 | pulid_weights | `model.diffusion_model.` |
| 10 | llm | `text_encoders.llm.` |
| 11 | llm_vision | `text_encoders.llm.visual.` |
| 12 | vae | `vae.` |
| 13 | tae | `tae.` |
| 14 | embeddings_connectors | `embeddings_connectors.` |
| 15 | audio_vae | `""` |
| 16 | audio_encoder | `wav2vec2.` |
| 17 | motion_module | `model.diffusion_model.motion_module.` |
| 18 | control_net | `""` |
| 19 | ip_adapter | `""` |
| 20 | photo_maker | `pmid.` |

Building the engine's catalog:
- A prefix is added only to a name that does not already start with it.
- The catalog is an insertion-ordered map. A later file's equal name replaces the value but keeps the first position (`model_loader_files.cpp:45-70`).
- Within a file, tensors come in `engine_order`.
- Shapes are `engine_ne`.
- `logical_ne0` is `ne[0]`. Plan decision: the engine doubles it only for its own w4 "convrot" weights, which a header read cannot tell apart, and no published file uses them.

**The two passes.**

The engine detects on the prefixed names, converts every name with that first answer, then detects again. The second answer is final:
- `rebuild_catalog`, `model_loader_files.cpp:45-70`;
- `convert_tensors_name`, `diffusion_engine.cpp:843`;
- `get_sd_version`, `diffusion_engine.cpp:977`.

The port:
```text
raw  = catalog(files)
v1   = signatures(raw)                 // the table below; None = VERSION_COUNT
conv = catalog of convert(name, v1) for each raw name, in raw order (equal names: later value, first position)
return signatures(conv)
```

`convert(name, v1)` (`src/name_conversion.cpp:1741-1778`):
1. **The connector map.** First match, in order:
   - `embeddings_connectors.text_encoders.` → `text_encoders.`;
   - `embeddings_connectors.` → `text_encoders.ph3_image_connector.` when v1 is minimax-h3, p-h3-image or minimax-h3-image, else `""`.
2. **The prefix map:**
   - `diffusion_model.`, `unet.` and `transformer.` → `model.diffusion_model.`;
   - `vae.` → `first_stage_model.`;
   - `text_encoders.llm.text_embedding_projection.` → `text_embedding_projection.`;
   - `text_encoders.llm.text_connector.` → `text_encoders.ph3_image_connector.`;
   - `text_encoder.` and `te.` → `cond_stage_model.transformer.`;
   - `text_encoder.2.` → `cond_stage_model.1.transformer.`;
   - `conditioner.embedders.0.open_clip.` and `conditioner.embedders.0.` → `cond_stage_model.`;
   - `conditioner.embedders.1.` → `cond_stage_model.1.`;
   - `te2.` → `cond_stage_model.1.transformer.`;
   - `te1.` → `cond_stage_model.transformer.`, or `text_encoders.clip_l.transformer.` when `v1.is_flux()`;
   - `te3.` → `text_encoders.t5xxl.transformer.`;
   - `clip_vision.` → `cond_stage_model.transformer.`;
   - `clip_l.` and `clip_g.` → `cond_stage_model.transformer.` and `cond_stage_model.1.transformer.` when `v1.is_unet()`, else `text_encoders.clip_l.transformer.` and `text_encoders.clip_g.transformer.`.
3. **Which entry applies.** The engine applies the first match of a `std::unordered_map`, whose order is unspecified. Plan decision: the longest matching prefix applies. The only overlaps are `text_encoder.` with `text_encoder.2.`, and `conditioner.embedders.0.` with `…0.open_clip.`.
4. **Not ported:** the per-family renames that run once v1 is known (diffusers names to original names), and the LoRA renames. The only effect is that a diffusers-layout DiT single file keeps its diffusers names in pass 2:
   - FLUX Fill, Controls and Flex.2 in that layout read as FLUX, because their input layer is `x_embedder`, not `img_in`.
   - Those recipes are out of scope (Task R7), or ship in the original layout.

**`signatures(catalog)`** (`src/model_loader.cpp:443-763`).
- `D.` is `model.diffusion_model.`.
- "has" is a substring match on the name (`find != npos`); "is" is equality; "key K" means the catalog contains exactly K.
- Rows run top to bottom: first the pre-loop rows, then for each tensor in catalog order all loop rows, then the post-loop rows.
- A row that returns ends detection.

| # | Where | Condition | Effect | Lines |
|---|---|---|---|---|
| 1 | pre | key `D.perceiver.latent_queries` | fibo2 | 446-448 |
| 2 | pre | keys `D.emb_in.image.weight` and `D.txt_mode_blocks.0.q_proj.weight` | flux3 | 450-453 |
| 3 | pre | keys `D.video_embedder.weight` and `D.shared_block_modulation.weight` | moss-image | 455-458 |
| 4 | loop | has `D.double_blocks.` or `D.single_transformer_blocks.` | is_flux | 479-482 |
| 5 | loop | has `D.net.lq_proj.latent_proj.0.weight` | pid | 483-485 |
| 6 | loop | has `embed_image_indicator.weight` | ideogram4 | 486-488 |
| 7 | loop | has `D.txtfusion.projector.weight` or `D.text_fusion.projector.weight` | krea2 | 489-492 |
| 8 | loop | has `D.nerf_final_layer_conv.` | chroma-radiance | 493-495 |
| 9 | loop | has `distilled_guidance_layer.in_proj.weight` | has_distilled_guidance | 496-498 |
| 10 | loop | has `D.joint_blocks.` | sd3 | 499-501 |
| 11 | loop | has `model.x_embedder.proj1.weight` and key `model.language_model.layers.0.self_attn.q_proj.weight` | hidream-o1 | 502-505 |
| 12 | loop | has `D.transformer_blocks.0.attn.norm_added_q.weight` and key `D.transformer_blocks.0.img_mlp.w1.weight` | lens | 506-509 |
| 13 | loop | has `net.img_embedder.proj1.weight` | minit2i | 510-512 |
| 14 | loop | has `language_model.model.layers.0.self_attn.q_proj_mot_gen.weight` | sensenova-u1.5 | 513-515 |
| 15 | loop | is `D.txt_in.text_norm.weight` | qwen-image-2.1 | 516-518 |
| 16 | loop | has `D.transformer_blocks.0.img_mod.1.weight` | mage-flow if key `D.img_in.weight` has `ne[0]` 128; else qwen-image-layered if key `D.time_text_embed.addition_t_embedding.weight`; else qwen-image | 519-528 |
| 17 | loop | has `D.txt_in.individual_token_refiner.blocks.0.adaLN_modulation.1.weight` | hunyuan-video | 529-531 |
| 18 | loop | has `llm_adapter.blocks.0.cross_attn.q_proj.weight` | anima | 532-534 |
| 19 | loop | has `D.double_stream_modulation_img.lin.weight` | is_flux2 | 535-537 |
| 20 | loop | has `dual_time_embed.semantic_embedder.linear_1.weight` | sefi-image | 538-540 |
| 21 | loop | has `single_blocks.47.linear1.weight` | has_single_block_47 | 541-543 |
| 22 | loop | has `D.double_blocks.0.img_mlp.gate_proj.weight` | ovis-image | 544-546 |
| 23 | loop | has `D.sigvq_embedder.1.weight` | llada-image | 547-549 |
| 24 | loop | has `D.cap_embedder.0.weight` | ming-image if key `text_encoders.llm.connector.layers.0.self_attn.q_proj.weight`; else z-image-l2p if key `D.local_decoder.out_conv.weight`; else z-image | 550-558 |
| 25 | loop | has `double_stream_layers.0.img_instruct_attn.processor.img_to_q.weight` | boogu-image | 559-561 |
| 26 | loop | has `D.layers.0.adaLN_sa_ln.weight` | ernie-image | 562-564 |
| 27 | loop | has `D.t_block.1.weight`, key `D.x_embedder.proj.weight`, no key `D.audio_patchify_proj.weight` | pixart | 565-569 |
| 28 | loop | has `D.adaln_single.emb.timestep_embedder.linear_1.bias` | pixart if keys `D.pos_embed.proj.weight` and `D.adaln_single.linear.weight` and no key `D.audio_patchify_proj.weight`; else ltxav | 570-578 |
| 29 | loop | has `D.video_patch_proj.weight` and key `D.condition_proj.weight` | minimax-h3-image if no key `D.audio_patch_proj.weight`; else p-h3-image if key `first_stage_model.decoder.proj_out.weight` (else `vae.decoder.proj_out.weight`) has `ne[1]` 768; else minimax-h3 | 579-593 |
| 30 | loop | is `D.y_embedder.layer_pool.weight` and key `D.modulation_cores.adaln_img.weight` | iris | 594-597 |
| 31 | loop | has `D.blocks.0.cross_attn.norm_k.weight` | is_wan | 598-600 |
| 32 | loop | has `casual_audio_encoder.weights` or `audio_injector.injector.0.q.weight` | is_s2v | 601-605 |
| 33 | loop | has `D.patch_embedder.weight` | lingbot-video | 606-608 |
| 34 | loop | has `D.patch_embedding.weight` | patch_channels = `ne[3]` | 609-611 |
| 35 | loop | has `D.img_emb` | has_img_emb | 612-614 |
| 36 | loop | has `D.input_blocks.`, `D.down_blocks.` or `unet.down_blocks.` | is_unet; is_xl if has_multiple_encoders | 615-622 |
| 37 | loop | has `conditioner.embedders.1`, `cond_stage_model.1` or `te.1` | has_multiple_encoders; is_xl if is_unet | 623-629 |
| 38 | loop | has `D.add_embedding.` or `D.label_emb.0.0.` | has_sdxl_add_embedding | 633-636 |
| 39 | loop | has `D.input_blocks.8.0.time_mixer.mix_factor` | svd | 637-639 |
| 40 | loop | has `D.middle_block.1.`, `D.mid_block.resnets.1.` or `unet.mid_block.resnets.1.` | has_middle_block_1 | 640-644 |
| 41 | loop | has `D.output_blocks.3.1.transformer_blocks.1`, `D.up_blocks.1.attentions.0.transformer_blocks.1` or `unet.up_blocks.1.attentions.0.transformer_blocks.1` | has_output_block_311 | 645-649 |
| 42 | loop | has `D.output_blocks.7.1`, `D.up_blocks.2.attentions.1` or `unet.up_blocks.2.attentions.1` | has_output_block_71; has_attn_1024 if it also has `D.output_blocks.7.1.transformer_blocks.0.attn1.to_k.weight` with `ne[0]` 1024 | 650-658 |
| 43 | loop | is one of `cond_stage_model.transformer.text_model.embeddings.token_embedding.weight`, `cond_stage_model.model.token_embedding.weight`, `text_model.embeddings.token_embedding.weight`, `te.text_model.embeddings.token_embedding.weight`, `conditioner.embedders.0.model.token_embedding.weight`, `conditioner.embedders.0.transformer.text_model.embeddings.token_embedding.weight` | token = this tensor | 659-667 |
| 44 | loop | is `D.input_blocks.0.0.weight`, `D.conv_in.weight`, `D.img_in.weight` or `unet.conv_in.weight` | input = this tensor | 668-673 |
| 45 | loop | is `D.txt_in.weight` or `D.context_embedder.weight` | context = this tensor | 674-676 |
| 46 | post | is_wan | wan2.2-s2v if is_s2v; wan2.2-i2v if patch_channels 184320 and not has_img_emb; wan2.2-ti2v if 147456 and not has_img_emb; else wan2 | 678-690 |
| 47 | post | — | inpaint = input `ne[2]` 9; ip2p = input `ne[2]` 8; is_xl also when is_unet and has_sdxl_add_embedding | 691-695 |
| 48 | post | is_xl | sdxl-inpaint if inpaint; sdxl-pix2pix if ip2p; if not has_middle_block_1: sdxl-vega if not has_output_block_311, else sdxl-ssd1b; else sdxl | 696-710 |
| 49 | post | is_flux and not is_flux2 | chroma if has_distilled_guidance; longcat if context `ne[0]` 3584; flux-fill if input `ne[0]` 384; flux-controls if 128; flex-2 if 196; else flux | 712-730 |
| 50 | post | is_flux2 | flux2 if has_single_block_47, else flux2-klein | 732-737 |
| 51 | post | token `ne[0]` 768 | sd1-inpaint if inpaint; sd1-pix2pix if ip2p; if not has_middle_block_1: sdxs-512-ds if not has_output_block_71, else sd1-tiny-unet; else sd1 | 739-752 |
| 52 | post | token `ne[0]` 1024 | sd2-inpaint if inpaint; if not has_middle_block_1: sdxs-09 if has_attn_1024, else sd2-tiny-unet; else sd2 | 753-761 |
| 53 | post | — | None (`VERSION_COUNT`) | 762 |

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
fn as_diffusion(name: &str) -> Option<SdVersion> { let ix = fixture(name); detect_version(&[LoadedFile { slot: SlotId::DiffusionModel, index: &ix }]) }
fn as_model(name: &str) -> Option<SdVersion> { let ix = fixture(name); detect_version(&[LoadedFile { slot: SlotId::Model, index: &ix }]) }
#[test] fn each_fixture_is_detected_as_the_engine_detects_it() {
    use SdVersion::*;
    for (name, v) in [("sd15", Sd1), ("sdxl", Sdxl), ("sd35-medium", Sd3), ("flux1-dev", Flux)] { assert_eq!(as_model(name), Some(v), "{name}"); }
    for (name, v) in [("flux1-kontext", Flux), ("chroma", Chroma), ("qwen-image", QwenImage), ("qwen-image-edit-2509", QwenImage),
        ("z-image-turbo", ZImage), ("flux2-klein-4b", Flux2Klein), ("wan21-t2v-1.3b", Wan2), ("wan21-i2v-14b", Wan2),
        ("wan22-ti2v-5b", Wan22Ti2v), ("wan22-t2v-a14b-high", Wan2), ("wan22-t2v-a14b-low", Wan2), ("hunyuan-video-15", HunyuanVideo),
        ("ltx23", Ltxav), ("lumina2", ZImage)] {                           // Lumina 2 is Z-Image to the engine (Task R6 refuses it)
        assert_eq!(as_diffusion(name), Some(v), "{name}");
    }
}
#[test] fn the_01x_family_files_detect_as_their_recipes_versions() {
    use SdVersion::*;
    for (txt, v) in [("klein4b", Flux2Klein), ("klein9b", Flux2Klein), ("zimage", ZImage), ("wan22-ti2v-5b", Wan22Ti2v),
        ("wan21-t2v-1.3b", Wan2), ("flux3-klein", Flux3)] {
        assert_eq!(detect_version(&[LoadedFile { slot: SlotId::DiffusionModel, index: &gguf_txt_fixture(txt) }]), Some(v), "{txt}");
    }
}
#[test] fn companions_and_loras_are_not_models() {
    for name in ["ae-flux1", "clip-l", "umt5-xxl-q5km", "sdxl-lora-pixel-art", "flux-lora-realism"] { assert_eq!(as_diffusion(name), None, "{name}"); assert_eq!(as_model(name), None, "{name}"); }
}
#[test] fn a_diffusers_folder_is_detected_from_its_parts() {
    assert_eq!(detect_folder(&[(FolderPart::Unet, &fixture("sd14-diffusers-unet")), (FolderPart::TextEncoder, &fixture("sd14-diffusers-text-encoder"))]), Some(SdVersion::Sd1));
}
#[test] fn the_row_order_decides_like_the_engine() {
    // a FLUX file that also carries the Chroma Radiance marker returns chroma-radiance at row 8, before row 49
    assert_eq!(detect_names(&[("D.double_blocks.0.img_attn.qkv.weight", [3072, 9216, 1, 1]), ("D.nerf_final_layer_conv.weight", [1, 1, 1, 1])]), Some(SdVersion::ChromaRadiance));
    // the Wan TI2V check reads ne[3] after folding (147456 = 48 × 3072)
    assert_eq!(detect_names(&[("D.blocks.0.cross_attn.norm_k.weight", [3072, 1, 1, 1]), ("D.patch_embedding.weight", [2, 2, 1, 147456])]), Some(SdVersion::Wan22Ti2v));
    // pass 2: a bare "diffusion_model." checkpoint becomes "model.diffusion_model."
    assert_eq!(detect_names_as_model(&[("diffusion_model.joint_blocks.0.x_block.attn.qkv.weight", [1536, 4608, 1, 1])]), Some(SdVersion::Sd3));
}
#[test] fn every_version_has_a_word_that_reads_back() {
    assert_eq!(SdVersion::ALL.len(), 60);
    for v in SdVersion::ALL { assert_eq!(SdVersion::from_word(v.word()), Some(v)); assert_eq!(serde_json::to_value(v).unwrap(), v.word()); }
    assert_eq!(SdVersion::Wan22Ti2v.word(), "wan2.2-ti2v");
    assert_eq!(SdVersion::SensenovaU15.word(), "sensenova-u1.5");
}
```
`detect_names` and `detect_names_as_model` are test helpers. They expand a leading `D.` to `model.diffusion_model.` and build a GGUF-format index with the given `ne`.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p models --features test-support --lib sd::detect`
  - Expected: FAIL to compile.

- [ ] **Step 3: Implement `detect.rs`** by the tables. The catalog is a `Vec<(String, [u64; 4])>` plus a `HashMap<String, usize>` for "key" lookups. Each row is one `if` in table order; the table is the specification, and comments cite the row numbers and engine lines.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p models --features test-support --lib sd::detect`
  - Expected: PASS. If a fixture disagrees, the table is checked against `model_loader.cpp` at the pin again. The expected version is never changed to fit the port.

- [ ] **Step 5: Commit.**
```bash
git add crates/models/src/sd/detect.rs crates/models/src/sd/mod.rs
git commit -m "Studio: port the engine's model-version detection, in its evaluation order, with its two passes"
```

### Task R6: What a file is (layout, embedded parts, LoRAs, extra files, guards)

**Files:**
- Create: `crates/models/src/sd/inspect.rs`
- Modify: `crates/models/src/sd/mod.rs` (`FileFacts`)
- Test: inline

**Interfaces:**
- Consumes: Task R5 (`detect_version`, `detect_folder`, `SdVersion`); Task R2 (`TensorIndex`).
- Produces:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub enum Layout { Checkpoint, DiffusionOnly }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "lowercase")]
pub enum LoraFormat { Kohya, Peft, Lycoris, Diff }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub enum CannotRun { Lumina2, LtxVideo09, HunyuanVideo1, Svd, UnknownWeights, Sharded, Pickle, NoSha }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub enum FileKind {
    Model { version: SdVersion, layout: Layout, embeds: Vec<SlotId> },
    Lora { format: LoraFormat, base: Option<SdVersion> },
    Extra { slot: SlotId },
    CannotRun { reason: CannotRun, version: Option<SdVersion> },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct MatchFacts { pub sd3_blocks: Option<u32>, pub flux2_width: Option<u64>, pub zero_cond: bool, pub img_emb_pos: bool, pub has_img_emb: bool }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct FileFacts { pub kind: FileKind, pub facts: MatchFacts, pub parameters: u64, pub format: FileFormat }   // in sd/mod.rs
pub fn inspect(index: &TensorIndex) -> FileFacts;
pub fn inspect_folder(parts: &[(FolderPart, &TensorIndex)]) -> FileFacts;
pub fn inspect_local(path: &Path) -> Result<FileFacts, IndexError>;   // read_local + inspect; Pickle → CannotRun::Pickle
```

**Layout and slot:**
- **Checkpoint** when any name starts with `model.diffusion_model.`, or the file has `first_stage_model.`, `cond_stage_model.` or `conditioner.` names. It is detected in the `model` slot.
- **Diffusion-only** otherwise. It is detected in the `diffusion_model` slot.
- Plan decision: this is the engine's own split, between `-m` with prefix `""` and `--diffusion-model` with prefix `model.diffusion_model.`. Detecting a checkpoint in the diffusion slot would wrongly prefix its encoder names.

**Embedded parts** of a checkpoint, by name prefix:

| Prefixes | Slot |
|---|---|
| `first_stage_model.`, `vae.` | `vae` |
| `cond_stage_model.` (not `.1`), `conditioner.embedders.0.`, `text_encoders.clip_l.` | `clip_l` |
| `conditioner.embedders.1.`, `cond_stage_model.1.`, `text_encoders.clip_g.` | `clip_g` |
| `text_encoders.t5xxl.` | `t5xxl` |
| `text_encoders.llm.` | `llm` |

**LoRA format.** First match over all names:
- names ending `.lora_down.weight`, `.lora_up.weight` or `.lora.down.weight` → Kohya;
- names ending `.lora_A.weight`, `.lora_B.weight` or `.lora_A.default.weight` → Peft;
- names containing `.hada_w1_a` or `.lokr_w1` → Lycoris;
- names ending `.diff` or `.diff_b` → Diff.

These are the engine's suffixes (`name_conversion.cpp`, the LoRA suffix map).

**LoRA base hint.** First match:

| Marker | Base |
|---|---|
| `lora_te2_` or `te2.` names, or a `…attn2_to_k.lora_down.weight` whose shape `[1]` is 2048 | sdxl |
| `…attn2_to_k.lora_down.weight` with shape `[1]` 1024 | sd2 |
| `…attn2_to_k.lora_down.weight` with shape `[1]` 768, or `lora_unet_` names with neither of the above | sd1 |
| `double_stream_modulation` | flux2 |
| `double_blocks` or `single_blocks` | flux |
| `joint_blocks` | sd3 |
| `transformer_blocks.N.img_mod` or `img_mlp` | qwen-image |
| `blocks.N.cross_attn` or `blocks.N.self_attn` | wan2 |
| `cap_embedder` or `layers.N.adaLN_modulation` | z-image |
| `__metadata__` `ss_base_model_version` starting `sdxl` / `sd_v1` / `sd_v2` / `flux` | sdxl / sd1 / sd2 / flux |
| none | None ("LoRA for an unknown model type") |

**Extra file kinds** (a file with no version and no LoRA marker):

| Marker | Slot |
|---|---|
| GGUF `general.architecture` `clip` with `mm.` or `v.blk.` names | `llm_vision` |
| GGUF `general.architecture` in `qwen2`, `qwen2vl`, `qwen3`, `qwen3vl`, `qwen35`, `mistral3`, `llama`, `gemma2`, `gemma3` | `llm` |
| `model.layers.0.self_attn.q_proj.weight` (safetensors language models) | `llm` |
| `encoder.block.0.layer.0.SelfAttention.q.weight`, or GGUF `enc.blk.0.` | `t5xxl` |
| `text_model.encoder.layers.0.` with hidden width 768 | `clip_l` |
| `text_model.encoder.layers.0.` with width 1280 | `clip_g` |
| `vision_model.encoder.layers.0.` | `clip_vision` |
| `decoder.conv_in.weight` or `first_stage_model.decoder.` without a diffusion marker | `vae` |
| `decoder.layers.` and `encoder.layers.` (TAESD) | `tae` |
| `wav2vec2.` or `feature_extractor.conv_layers.` | `audio_encoder` |
| `audio_vae.` or `vocoder.` | `audio_vae` |
| `connectors.` or `embeddings_connector` | `embeddings_connectors` |

**Guards**, applied after detection:

| Detected | Condition | Becomes |
|---|---|---|
| z-image | `model.diffusion_model.cap_embedder.0.weight` `ne[0]` is 2304 (Gemma-2-2B; Z-Image is 2560) | `CannotRun { Lumina2, Some(ZImage) }` |
| ltxav | no `model.diffusion_model.audio_patchify_proj.weight` | `CannotRun { LtxVideo09, Some(Ltxav) }` |
| hunyuan-video | no `byt5_in.fc1.weight` (HunyuanVideo 1.5 has it, v1 has not) | `CannotRun { HunyuanVideo1, Some(HunyuanVideo) }` |
| svd | — | `CannotRun { Svd, Some(Svd) }` |
| None | no LoRA or extra marker | `CannotRun { UnknownWeights, None }` |

**Facts for matching (Task R7):**
- `sd3_blocks`: the highest `joint_blocks.N` plus 1.
- `flux2_width`: `img_in.weight` `ne[1]`.
- `zero_cond`: the key `__index_timestep_zero__`, under any prefix (Qwen-Image Edit 2511; `diffusion_engine.cpp:1330`).
- `img_emb_pos`: a name contains `img_emb.emb_pos`.
- `has_img_emb`: a name contains `img_emb`.
- `parameters`: from the index.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[test] fn checkpoints_say_what_they_embed() {
    assert_eq!(inspect(&fixture("sd15")).kind, FileKind::Model { version: SdVersion::Sd1, layout: Layout::Checkpoint, embeds: vec![SlotId::ClipL, SlotId::Vae] });
    assert!(matches!(inspect(&fixture("sdxl")).kind, FileKind::Model { version: SdVersion::Sdxl, ref embeds, .. } if embeds.contains(&SlotId::ClipG)));
    assert!(matches!(inspect(&fixture("sd35-medium")).kind, FileKind::Model { ref embeds, .. } if embeds == &vec![SlotId::Vae]));
    assert!(matches!(inspect(&fixture("flux1-kontext")).kind, FileKind::Model { layout: Layout::DiffusionOnly, .. }));
}
#[test] fn lumina_2_is_refused_although_the_engine_calls_it_z_image() {
    assert_eq!(inspect(&fixture("lumina2")).kind, FileKind::CannotRun { reason: CannotRun::Lumina2, version: Some(SdVersion::ZImage) });
    assert!(matches!(inspect(&fixture("z-image-turbo")).kind, FileKind::Model { version: SdVersion::ZImage, .. }));
}
#[test] fn ltx_video_09_and_hunyuan_video_1_are_refused() {
    assert!(matches!(inspect(&fixture("ltx23")).kind, FileKind::Model { version: SdVersion::Ltxav, .. }));
    assert_eq!(inspect(&without(&fixture("ltx23"), "audio_patchify_proj")).kind, FileKind::CannotRun { reason: CannotRun::LtxVideo09, version: Some(SdVersion::Ltxav) });
    assert!(matches!(inspect(&fixture("hunyuan-video-15")).kind, FileKind::Model { version: SdVersion::HunyuanVideo, .. }));
    assert_eq!(inspect(&without(&fixture("hunyuan-video-15"), "byt5_in")).kind, FileKind::CannotRun { reason: CannotRun::HunyuanVideo1, version: Some(SdVersion::HunyuanVideo) });
}
#[test] fn loras_are_named_with_a_base_hint() {
    assert_eq!(inspect(&fixture("sdxl-lora-pixel-art")).kind, FileKind::Lora { format: LoraFormat::Kohya, base: Some(SdVersion::Sdxl) });
    assert_eq!(inspect(&fixture("flux-lora-realism")).kind, FileKind::Lora { format: LoraFormat::Kohya, base: Some(SdVersion::Flux) });
}
#[test] fn companions_are_extra_files_for_their_slot() {
    assert_eq!(inspect(&fixture("ae-flux1")).kind, FileKind::Extra { slot: SlotId::Vae });
    assert_eq!(inspect(&fixture("clip-l")).kind, FileKind::Extra { slot: SlotId::ClipL });
    assert_eq!(inspect(&fixture("umt5-xxl-q5km")).kind, FileKind::Extra { slot: SlotId::T5xxl });
    assert_eq!(inspect(&gguf_txt_fixture("qwen3-4b")).kind, FileKind::Extra { slot: SlotId::Llm });
}
#[test] fn facts_for_matching_and_labels() {
    assert_eq!(inspect(&fixture("sd35-medium")).facts.sd3_blocks, Some(24));
    assert_eq!(inspect(&fixture("flux2-klein-4b")).facts.flux2_width, Some(3072));
    assert!(inspect(&fixture("wan21-i2v-14b")).facts.has_img_emb);
    assert_eq!(inspect(&fixture("wan22-t2v-a14b-high")).parameters / 1_000_000_000, 14);
}
#[test] fn a_pickle_file_is_named_without_being_read() {
    assert_eq!(inspect_local(Path::new("/nope/m.ckpt")).unwrap().kind, FileKind::CannotRun { reason: CannotRun::Pickle, version: None });
}
```
`without(index, part)` drops every tensor whose name contains `part`. `parameters` reads the fixture's stored full count.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p models --features test-support --lib sd::inspect`
  - Expected: FAIL to compile.

- [ ] **Step 3: Implement `inspect.rs`** by the tables. Detection runs once, in the slot the layout names. The guards, LoRA and extra tables run in the order given.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p models --features test-support --lib sd::`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add crates/models/src/sd
git commit -m "Studio: say what a weights file is: model and layout, LoRA and base, extra file, or why it can't run"
```

### Task R7: Version → recipe, kinds, input pictures and the recipes left out

**Files:**
- Create: `crates/models/src/sd/matching.rs`
- Test: inline

**Interfaces:**
- Consumes: `SdVersion` (Task R5), `MatchFacts` (Task R6), `recipe()` (Task R1).
- Produces:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub enum RecipeKind { Picture, PictureEdit, Video }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub enum OutOfScope { Adapter, NeedsControlFrames, NeedsAudio, Upscaler, FolderModel, NotThisEngine, Ltx2 }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub enum InputRole { Reference, StartFrame, EndFrame, Source, Mask, Control }   // Source: the picture to change (-i)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")] pub struct RecipeInput { pub role: InputRole, pub min: u32, pub max: u32 }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub enum NoisePart { High, Low }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match { Recipe { id: &'static str, part: Option<NoisePart> }, Refused(OutOfScope) }
pub fn tokens(text: &str) -> Vec<String>;                   // lower case; "_" and " " read as "-"; split at chars other than ASCII letters, digits and '.'
pub fn match_recipe(version: SdVersion, facts: &MatchFacts, embeds: &[SlotId], name: &str) -> Match;   // name = "<repo>/<path>"
pub fn recipes_for(version: SdVersion) -> Vec<&'static str>;  // every in-scope recipe this version can take: the Model type menu
pub fn kind_of(recipe_id: &str) -> RecipeKind;
pub fn out_of_scope(recipe_id: &str) -> Option<OutOfScope>;
pub fn in_scope_recipes() -> Vec<&'static str>;              // 94, sorted
pub fn inputs_of(recipe_id: &str) -> Vec<RecipeInput>;     // the input pictures a recipe takes, by the table below
pub fn noise_part(name: &str) -> Option<NoisePart>;          // tokens high, highnoise, high-noise / low, lownoise, low-noise
```
- **Tokens** are whole words of `tokens(name)`. Multi-word tokens (`high-noise`, `ltx-2`) match as substrings of the normalised text.
- **Kinds:**
  - `Video` when `recipe(id).is_video()`;
  - `PictureEdit` for: instruct-pix2pix, cosxl-edit, flux1-kontext, qwen-image-edit, qwen-image-edit-2509, qwen-image-edit-2511, qwen-image-layered, longcat-image-edit, mage-flow-edit, anima-edit, boogu-image-edit, fibo2-edit-turbo, krea2-raw-edit, krea2-turbo-edit, minimax-h3-image and p-h3-image, and (design 11a) sd1-inpainting, sd2-inpainting, sdxl-inpainting and flux1-fill;
  - else `Picture`. flux1-canny, flux1-depth and flex2 are pictures made from a prompt plus their input pictures.
- **Input pictures (`inputs_of`).** Engine flags in brackets (`examples/common/common.cpp:1088-1107`):

  | Recipes | Inputs (role min–max) |
  |---|---|
  | sd1-inpainting, sd2-inpainting, sdxl-inpainting, flux1-fill | Source 1–1 (`-i`), Mask 1–1 (`--mask`) |
  | flux1-canny, flux1-depth | Control 1–1 (`--control-image`) |
  | flex2 | Source 0–1, Mask 0–1, Control 0–1. A mask needs a source (`src/pipeline/image.cpp:274-281` repaints everything otherwise); the control picture is read at `image.cpp:478-487` |
  | any other recipe with `feature("ref_images")` | Reference 1–4 for `PictureEdit` kinds, 0–4 otherwise (`-r`; 4 is 0.1.x's `MAX_REFERENCES`) |
  | wan21-i2v, wan22-i2v, hunyuan-video-15-i2v | StartFrame 1–1 (`-i`) |
  | wan21-flf2v | StartFrame 1–1, EndFrame 1–1 (`--end-img`) |
  | any other video recipe with `feature("init_image")` / `feature("end_image")` | StartFrame 0–1 / EndFrame 0–1 |
  | the rest | none |

  - Plan decision: a Mask is any picture. The engine resizes it to the output size and rounds each pixel to black or white (`image.cpp:262-264`), and white areas are the ones changed.
  - Painting a mask in the app is a later step (design 11a).

**The matching table.** The first rule that matches wins. The part for Wan comes from `noise_part`.

| Version | Default recipe | Rules |
|---|---|---|
| sd1, sd1-tiny-unet | sd1-checkpoint | — |
| sdxs-512-ds, sdxs-09 | sdxs | — |
| sd1-pix2pix | instruct-pix2pix | — |
| sd2, sd2-tiny-unet | sd2-512-checkpoint | turbo → sd-turbo; 768 or v → sd2-768-checkpoint |
| sdxl, sdxl-vega, sdxl-ssd1b | sdxl-checkpoint | turbo → sdxl-turbo; nitro → nitrosd-realism |
| sdxl-pix2pix | cosxl-edit | — |
| sd3 | sd35-large | 24 blocks and embeds clip_l, clip_g, t5xxl → sd3-medium; 24 → sd35-medium; 38 → sd35-large; blocks unknown: medium → sd35-medium, large → sd35-large |
| flux | flux1-dev | kontext → flux1-kontext; schnell → flux1-schnell; fill → flux1-fill; canny → flux1-canny; depth → flux1-depth (diffusers-layout files, which the port reads as flux, Task R5) |
| flux-fill | flux1-fill | — |
| flux-controls | flux1-canny | depth → flux1-depth |
| sd1-inpaint | sd1-inpainting | — |
| sd2-inpaint | sd2-inpainting | — |
| sdxl-inpaint | sdxl-inpainting | — |
| flex-2 | flex2 | — |
| chroma | chroma | — |
| chroma-radiance | chroma-radiance | — |
| ovis-image | ovis-image | — |
| longcat | longcat-image | edit → longcat-image-edit |
| flux2 | flux2-dev | — |
| flux2-klein | flux2-klein-4b | width 4096 or 9b: base → flux2-klein-base-9b, else flux2-klein-9b; base → flux2-klein-base-4b |
| flux3 | flux3-klein | base → flux3-klein-base |
| sefi-image | sefi-image | — |
| qwen-image | qwen-image | edit and (2511 or zero_cond) → qwen-image-edit-2511; edit and 2509 → qwen-image-edit-2509; edit → qwen-image-edit |
| qwen-image-layered | qwen-image-layered | — |
| qwen-image-2.1 | qwen-image-21 | turbo → qwen-image-21-turbo |
| mage-flow | mage-flow | edit → mage-flow-edit; turbo → mage-flow-turbo |
| z-image | z-image | turbo → z-image-turbo |
| z-image-l2p | z-image-l2p | — |
| ming-image | ming-image | — |
| anima | anima | edit → anima-edit |
| boogu-image | boogu-image | edit → boogu-image-edit |
| ernie-image | ernie-image | turbo → ernie-image-turbo |
| fibo2 | fibo2-dev | edit → fibo2-edit-turbo |
| hidream-o1 | hidream-o1-dev | — |
| ideogram4 | ideogram4 | — |
| iris | iris-3b | — |
| krea2 | krea2-raw | turbo and edit → krea2-turbo-edit; edit → krea2-raw-edit; turbo → krea2-turbo |
| lens | lens | turbo → lens-turbo |
| llada-image | llada-image | turbo → llada-image-turbo |
| minit2i | minit2i | — |
| moss-image | moss-image-base | sft → moss-image-sft |
| pixart | pixart-sigma | alpha and 512 → pixart-alpha-512; alpha → pixart-alpha-1024 |
| minimax-h3-image | minimax-h3-image | p-h3 or p-image → p-h3-image |
| p-h3-image | p-h3-image | — |
| wan2 | wan21-t2v | vace → Refused(NeedsControlFrames); img_emb_pos or flf2v → wan21-flf2v; has_img_emb → wan21-i2v; (wan2.2 or wan22) and a part → wan22-t2v |
| wan2.2-i2v | wan22-i2v | — |
| wan2.2-ti2v | wan22-ti2v-5b | — |
| lingbot-video | lingbot-video-dense | moe, 30b or a3b → lingbot-video-moe |
| hunyuan-video | hunyuan-video-15-t2v | i2v → hunyuan-video-15-i2v |
| ltxav | ltx23-dev | ltx-2 or ltx2 without 2.3, 23, 2.5 or 25 → Refused(Ltx2); 2.5 or ltx25 → ltx25-dev; distilled → ltx23-distilled |
| minimax-h3 | minimax-h3-fl2va | p-h3 or p-image → p-h3-image; hyperflow and ref2va → hyperflow-h3-flash-ref2va; hyperflow → hyperflow-h3-flash-fl2va; ref2va → minimax-h3-ref2va |

**Refusals by version:**
- svd → NotThisEngine;
- wan2.2-s2v → NeedsAudio;
- pid → Upscaler;
- sensenova-u1.5 → FolderModel.

**Out-of-scope recipes**, 18, leaving 94 in scope.
- Section 3 rules out adapter files, upscalers and audio-driven video.
- Design 11a brings inpainting, Fill, Canny, Depth and Flex.2 in, with Mask and Control picture inputs.
- Plan decision: VACE stays out. It needs a folder of control video frames, not a picture.

| Reason | Recipes |
|---|---|
| Adapter | sd1-controlnet, sdxl-controlnet, sd1-ip-adapter, sdxl-ip-adapter, sdxl-photomaker, animatediff-sd15, flux1-pulid |
| NeedsControlFrames | wan21-vace |
| NeedsAudio | wan22-s2v |
| Upscaler | pid, pid-15, pid-sd3, pid-15-sd3, pid-flux2, pid-15-flux2, pid-qwen-image, pid-15-qwen-image |
| FolderModel | sensenova-u15 |

Plan decision: `minimax-h3-ref2va` and `hyperflow-h3-flash-ref2va` stay in scope with reference pictures only. Their video and audio references are not offered.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
fn m(v: SdVersion, name: &str) -> Match { match_recipe(v, &MatchFacts::default(), &[], name) }
#[test] fn the_design_examples_match() {
    assert_eq!(m(SdVersion::Flux, "black-forest-labs/FLUX.1-dev/flux1-dev.safetensors"), Match::Recipe { id: "flux1-dev", part: None });
    assert_eq!(m(SdVersion::Flux, "o/FLUX.1-schnell-gguf/flux1-schnell-Q4_0.gguf"), Match::Recipe { id: "flux1-schnell", part: None });
    assert_eq!(m(SdVersion::Flux, "QuantStack/FLUX.1-Kontext-dev-GGUF/flux1-kontext-dev-Q4_K_M.gguf"), Match::Recipe { id: "flux1-kontext", part: None });
    assert_eq!(m(SdVersion::QwenImage, "o/Qwen-Image-Edit-2511-GGUF/x.gguf"), Match::Recipe { id: "qwen-image-edit-2511", part: None });
    assert_eq!(m(SdVersion::QwenImage, "QuantStack/Qwen-Image-Edit-2509-GGUF/Qwen-Image-Edit-2509-Q4_0.gguf"), Match::Recipe { id: "qwen-image-edit-2509", part: None });
    assert_eq!(m(SdVersion::QwenImage, "o/Qwen-Image-Edit/x.gguf"), Match::Recipe { id: "qwen-image-edit", part: None });
}
#[test] fn wan_parts_come_from_the_name() {
    assert_eq!(m(SdVersion::Wan2, "QuantStack/Wan2.2-T2V-A14B-GGUF/HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf"), Match::Recipe { id: "wan22-t2v", part: Some(NoisePart::High) });
    assert_eq!(m(SdVersion::Wan2, "QuantStack/Wan2.2-T2V-A14B-GGUF/LowNoise/Wan2.2-T2V-A14B-LowNoise-Q4_K_M.gguf"), Match::Recipe { id: "wan22-t2v", part: Some(NoisePart::Low) });
    assert_eq!(match_recipe(SdVersion::Wan2, &MatchFacts { has_img_emb: true, ..Default::default() }, &[], "city96/Wan2.1-I2V-14B-480P-gguf/w.gguf"), Match::Recipe { id: "wan21-i2v", part: None });
    assert_eq!(m(SdVersion::Wan2, "Comfy-Org/Wan-2.1/wan2.1_t2v_1.3B_fp16.safetensors"), Match::Recipe { id: "wan21-t2v", part: None });
    assert_eq!(m(SdVersion::Wan2, "o/Wan2.1-VACE-14B/x.gguf"), Match::Refused(OutOfScope::NeedsControlFrames));
}
#[test] fn sd3_and_klein_use_the_files_own_facts() {
    let f = |b| MatchFacts { sd3_blocks: Some(b), ..Default::default() };
    assert_eq!(match_recipe(SdVersion::Sd3, &f(24), &[SlotId::Vae], "o/r/sd3.5_medium.safetensors"), Match::Recipe { id: "sd35-medium", part: None });
    assert_eq!(match_recipe(SdVersion::Sd3, &f(24), &[SlotId::ClipL, SlotId::ClipG, SlotId::T5xxl, SlotId::Vae], "o/r/sd3_medium_incl_clips_t5xxlfp8.safetensors"), Match::Recipe { id: "sd3-medium", part: None });
    assert_eq!(match_recipe(SdVersion::Flux2Klein, &MatchFacts { flux2_width: Some(4096), ..Default::default() }, &[], "o/r/k.gguf"), Match::Recipe { id: "flux2-klein-9b", part: None });
}
#[test] fn ltx_2_is_refused_and_ltx_2_3_runs() {
    assert_eq!(m(SdVersion::Ltxav, "unsloth/LTX-2.3-GGUF/ltx-2.3-22b-dev-Q4_K_M.gguf"), Match::Recipe { id: "ltx23-dev", part: None });
    assert_eq!(m(SdVersion::Ltxav, "Lightricks/LTX-2/ltx-2-19b-dev.safetensors"), Match::Refused(OutOfScope::Ltx2));
}
#[test] fn every_in_scope_recipe_is_reachable_and_exists() {
    assert_eq!(in_scope_recipes().len(), 94);
    let reached: BTreeSet<_> = SdVersion::ALL.iter().flat_map(|v| recipes_for(*v)).collect();
    assert_eq!(reached, in_scope_recipes().into_iter().collect());
    assert!(recipes().iter().all(|r| out_of_scope(&r.id).is_some() != in_scope_recipes().contains(&r.id.as_str())));
    assert!(recipes().iter().filter(|r| r.slots.iter().any(|s| s.id.is_adapter())).all(|r| out_of_scope(&r.id) == Some(OutOfScope::Adapter)));
}
#[test] fn kinds() {
    assert_eq!((kind_of("flux1-kontext"), kind_of("wan22-t2v"), kind_of("sdxl-checkpoint")), (RecipeKind::PictureEdit, RecipeKind::Video, RecipeKind::Picture));
    assert_eq!((kind_of("sdxl-inpainting"), kind_of("flux1-fill"), kind_of("flux1-canny")), (RecipeKind::PictureEdit, RecipeKind::PictureEdit, RecipeKind::Picture));
}
#[test] fn inpainting_fill_and_control_recipes_are_offered_with_their_input_pictures() {
    assert_eq!(m(SdVersion::SdxlInpaint, "o/r/sdxl-inpainting.safetensors"), Match::Recipe { id: "sdxl-inpainting", part: None });
    assert_eq!(m(SdVersion::FluxFill, "o/r/flux1-fill-dev.safetensors"), Match::Recipe { id: "flux1-fill", part: None });
    assert_eq!(m(SdVersion::FluxControls, "o/FLUX.1-Depth-dev/x.safetensors"), Match::Recipe { id: "flux1-depth", part: None });
    let ins = |id| inputs_of(id).into_iter().map(|i| (i.role, i.min, i.max)).collect::<Vec<_>>();
    assert_eq!(ins("sdxl-inpainting"), [(InputRole::Source, 1, 1), (InputRole::Mask, 1, 1)]);
    assert_eq!(ins("flux1-canny"), [(InputRole::Control, 1, 1)]);
    assert_eq!(ins("flex2"), [(InputRole::Source, 0, 1), (InputRole::Mask, 0, 1), (InputRole::Control, 0, 1)]);
    assert_eq!(ins("flux1-kontext"), [(InputRole::Reference, 1, 4)]);
    assert_eq!(ins("wan21-flf2v"), [(InputRole::StartFrame, 1, 1), (InputRole::EndFrame, 1, 1)]);
    assert_eq!((out_of_scope("sd1-controlnet"), out_of_scope("flux1-fill")), (Some(OutOfScope::Adapter), None));
}
#[test] fn tokens_split_like_the_spec() {
    assert_eq!(tokens("Wan2.2_T2V-A14B HighNoise"), ["wan2.2", "t2v", "a14b", "highnoise"]);
}
```

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p models --features test-support --lib sd::matching`
  - Expected: FAIL to compile.

- [ ] **Step 3: Implement `matching.rs`** as data: one `const` table of `(version, default, &[(rule, recipe)])`, and the out-of-scope list.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p models --features test-support --lib sd::matching`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add crates/models/src/sd/matching.rs crates/models/src/sd/mod.rs
git commit -m "Studio: pick the engine recipe for a detected version, with name tokens and the recipes left out"
```

### Task R8: The companion table

**Files:**
- Create: `crates/models/src/sd/companions.rs`
- Modify:
  - `catalog/models.json`:
    - add `"companions"`;
    - set `sequence` 5 and `generated` `"2026-11-24"`, so the bundled catalog wins over a stored signed one of sequence 4.
  - `crates/models/src/catalog.rs`: keep `companions` raw, with `object_or_empty` as for `generators`.
- Test: inline in `companions.rs`; the existing `catalog.rs` test that parses the catalog the way 0.0.5 did stays green.

**Interfaces:**
- Consumes: `GenLicence` (`gen_catalog.rs:175`), `SlotId`, `SdVersion`, `check_licences`-style validation.
- Produces:
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct Companion { pub id: String, pub slot: SlotId, pub versions: Vec<SdVersion>, pub repo: String, pub revision: String,
    pub path: String, pub size: u64, pub sha256: String, pub license: String }
#[derive(Debug, Clone, PartialEq, Default, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct CompanionTable { pub licenses: BTreeMap<String, GenLicence>, pub files: Vec<Companion>,
    pub by_version: BTreeMap<SdVersion, BTreeMap<SlotId, String>>, pub by_recipe: BTreeMap<String, BTreeMap<SlotId, String>> }
impl CompanionTable {
    pub fn from_catalog(catalog: &Catalog) -> Result<CompanionTable, String>;   // validates as below
    pub fn default_for(&self, recipe_id: &str, version: SdVersion, slot: SlotId) -> Option<&Companion>;   // by_recipe wins over by_version
    pub fn by_sha(&self, sha256: &str) -> Option<&Companion>;
    pub fn licence(&self, id: &str) -> Option<&GenLicence>;
}
```
- **The JSON.** The companion files below go in `companions.files`. `companions.licenses` holds two licences:
  - `apache-2.0`:
    - text bundled; sha `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30`;
    - `commercialOutputs` `yes`, plain words "Free for any use, including commercial.";
    - url `https://www.apache.org/licenses/LICENSE-2.0.txt`.
  - `stabilityai-ai-community`:
    - `commercialOutputs` `underRevenue`, `revenueLimitUsd` 1000000, plain words "Commercial use only under $1,000,000 a year in revenue.";
    - url `https://huggingface.co/Comfy-Org/stable-diffusion-3.5/blob/934251d1e6168bcb45a069abc5bd5cdd5f8fea00/LICENSE.md`;
    - sha = SHA-256 of `Comfy-Org/stable-diffusion-3.5@934251d1e6168bcb45a069abc5bd5cdd5f8fea00:stabilityai-ai-community`, the 0.1.x rule for a licence with no bundled text.
- **Validation:**
  - every repo id is valid (`validate_repo_id`) and every path valid (`validate_repo_path`);
  - every revision is 40 hex and every sha256 64 hex;
  - every extension is in `ALLOWED_PART_EXTENSIONS`;
  - every licence id exists;
  - every default names a companion whose `slot` matches and whose `versions` include the version (for `by_recipe`, the recipe's version from Task R7's table);
  - every `by_recipe` key is an in-scope recipe.

**The companion files** (`companions.files`). They were verified ungated on 2026-11-24, with sizes and LFS SHA-256 read from the API.

| Id | Slot | Versions | Repo @ revision | Path | Size | SHA-256 | Licence |
|---|---|---|---|---|---|---|---|
| clip-l | clip_l | flux, flex-2, sd3 | comfyanonymous/flux_text_encoders @ 6af2a98e3f615bdfa612fbd85da93d1ed5f69ef5 | clip_l.safetensors | 246144152 | 660c6f5b1abae9dc498ac2d21e1347d2abdb0cf6c0c0c8576cd796491d9a6cdd | apache-2.0 |
| t5xxl-fp8 | t5xxl | flux, flex-2, chroma, chroma-radiance, sd3 | comfyanonymous/flux_text_encoders @ 6af2a98e3f615bdfa612fbd85da93d1ed5f69ef5 | t5xxl_fp8_e4m3fn.safetensors | 4893934904 | 7d330da4816157540d6bb7838bf63a0f02f573fc48ca4d8de34bb0cbfd514f09 | apache-2.0 |
| t5xxl-fp16 | t5xxl | flux, flex-2, chroma, chroma-radiance, sd3 | comfyanonymous/flux_text_encoders @ 6af2a98e3f615bdfa612fbd85da93d1ed5f69ef5 | t5xxl_fp16.safetensors | 9787841024 | 6e480b09fae049a72d2a8c5fbccb8d3e92febeb233bbe9dfe7256958a9167635 | apache-2.0 |
| clip-g | clip_g | sd3 | Comfy-Org/stable-diffusion-3.5 @ 934251d1e6168bcb45a069abc5bd5cdd5f8fea00 | text_encoders/clip_g.safetensors | 1389382176 | ec310df2af79c318e24d20511b601a591ca8cd4f1fce1d8dff822a356bcdb1f4 | stabilityai-ai-community |
| ae-flux1 | vae | flux, flex-2, chroma, z-image, ovis-image, longcat, boogu-image | lodestones/Chroma @ 2f3b2730d7b5edbd02cdff12f72610af5787300b | ae.safetensors | 335304388 | afc8e28272cd15db3919bacdb6918ce9c1ed22e96cb12c4d5ed0fba823529e38 | apache-2.0 |
| flux2-vae | vae | flux2, flux2-klein, flux3, ernie-image, fibo2, lens, moss-image, sefi-image, ideogram4, llada-image | Comfy-Org/Flux-2-Klein-4B @ 5b3abca5c0dc1ecd4eaf3340868d09063fc1c965 | split_files/vae/flux2-vae.safetensors | 336211292 | 868fe7b343cc8f3a19dbcfcafbc3d5f888802be3f89bd81b65b3621a066ce8f3 | apache-2.0 |
| qwen-image-vae | vae | qwen-image, qwen-image-layered, anima | Comfy-Org/Qwen-Image @ 1f12b17be14c89b026c51a91d67c32f84bb047bc | split_files/vae/qwen_image_vae.safetensors | 253806246 | a70580f0213e67967ee9c95f05bb400e8fb08307e017a924bf3441223e023d1f | apache-2.0 |
| qwen2.5-vl-7b-q8 | llm | qwen-image, qwen-image-layered, longcat, hunyuan-video | unsloth/Qwen2.5-VL-7B-Instruct-GGUF @ 68bb8bc4b7df5289c143aaec0ab477a7d4051aab | Qwen2.5-VL-7B-Instruct-Q8_0.gguf | 8098524032 | ee770c700d7429cc6f0c74d6c7ab3c063bf521312fc36e80776d1d79bc9fa4ad | apache-2.0 |
| qwen2.5-vl-7b-mmproj | llm_vision | qwen-image, longcat | unsloth/Qwen2.5-VL-7B-Instruct-GGUF @ 68bb8bc4b7df5289c143aaec0ab477a7d4051aab | mmproj-F16.gguf | 1354163040 | 987dd0733033fb5dd9b124d1ca926ae865572e432384eee7618b2eec3e735e17 | apache-2.0 |
| umt5-xxl-q8 | t5xxl | wan2, wan2.2-i2v, wan2.2-ti2v | city96/umt5-xxl-encoder-gguf @ b535255bee98c2b0a59ea7c0ae2dcd0c6657b3b7 | umt5-xxl-encoder-Q8_0.gguf | 6043068256 | 2521d4de0bf9e1cc6549866463ceae85e4ec3239bc6063f7488810be39033bbc | apache-2.0 |
| umt5-xxl-q5km | t5xxl | wan2, wan2.2-i2v, wan2.2-ti2v | city96/umt5-xxl-encoder-gguf @ b535255bee98c2b0a59ea7c0ae2dcd0c6657b3b7 | umt5-xxl-encoder-Q5_K_M.gguf | 4145878880 | eaea358bb438c5d211721a4feecc162000e3636e9cb96f51e216f1f44ebd12ce | apache-2.0 |
| wan21-vae | vae | wan2, wan2.2-i2v, krea2, lingbot-video | Comfy-Org/Wan-2.1 @ d6258323e18f14c60d37899da97ca555f3118259 | split_files/vae/wan_2.1_vae.safetensors | 253815318 | 2fc39d31359a4b0a64f55876d8ff7fa8d780956ae2cb13463b0223e15148976b | apache-2.0 |
| wan22-vae | vae | wan2.2-ti2v | Comfy-Org/Wan-2.2 @ 5df3a4a3ed0628002308022198249dbbbd045d09 | split_files/vae/wan2.2_vae.safetensors | 1409400960 | e40321bd36b9709991dae2530eb4ac303dd168276980d3e9bc4b6e2b75fed156 | apache-2.0 |
| clip-vision-h | clip_vision | wan2 | Comfy-Org/Wan-2.1 @ d6258323e18f14c60d37899da97ca555f3118259 | split_files/clip_vision/clip_vision_h.safetensors | 1264219396 | 64a7ef761bfccbadbaa3da77366aac4185a6c58fa5de5f589b42a65bcc21f161 | apache-2.0 |
| qwen3-4b-q8 | llm | flux2-klein | unsloth/Qwen3-4B-GGUF @ 22c9fc8a8c7700b76a1789366280a6a5a1ad1120 | Qwen3-4B-Q8_0.gguf | 4280405792 | eed555233267a33c7e8ee31682762cc7751b3f6d224039086e0e846f05fffa5d | apache-2.0 |
| qwen3-4b-q4km | llm | flux2-klein | unsloth/Qwen3-4B-GGUF @ 22c9fc8a8c7700b76a1789366280a6a5a1ad1120 | Qwen3-4B-Q4_K_M.gguf | 2497281312 | f6f851777709861056efcdad3af01da38b31223a3ba26e61a4f8bf3a2195813a | apache-2.0 |
| qwen3-8b-q8 | llm | flux2-klein | unsloth/Qwen3-8B-GGUF @ a6adef130ffb23ddaf1a62fec9dced968c9bc482 | Qwen3-8B-Q8_0.gguf | 8709519168 | 0cfbf745760f07a76ddeb358dd025a27f2e11d1ca9c9a4169a373d52990fe86e | apache-2.0 |
| qwen3-4b-2507-q8 | llm | z-image, z-image-l2p | unsloth/Qwen3-4B-Instruct-2507-GGUF @ a06e946bb6b655725eafa393f4a9745d460374c9 | Qwen3-4B-Instruct-2507-Q8_0.gguf | 4280405600 | 391c1e410fd9f4cf2de2b510273b56a84c19ce18f4fa3bfb3774031dac4ef068 | apache-2.0 |
| qwen3-4b-2507-q4km | llm | z-image, z-image-l2p | unsloth/Qwen3-4B-Instruct-2507-GGUF @ a06e946bb6b655725eafa393f4a9745d460374c9 | Qwen3-4B-Instruct-2507-Q4_K_M.gguf | 2497281120 | 3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597 | apache-2.0 |
| qwen3-vl-4b-q4km | llm | flux3, krea2, mage-flow, iris, lingbot-video, fibo2 | Qwen/Qwen3-VL-4B-Instruct-GGUF @ 1cd86afb9a95c410a6038ab3b40d8b578c892266 | Qwen3VL-4B-Instruct-Q4_K_M.gguf | 2497281664 | 66358cb18bb6b3b1b6675aa412c7a88ef01d228f481184d13668e5201c730a0a | apache-2.0 |
| mistral-small-3.2-24b-q4km | llm | flux2 | unsloth/Mistral-Small-3.2-24B-Instruct-2506-GGUF @ b750ec2299225e492f1bd27cab88a0a595fa848f | Mistral-Small-3.2-24B-Instruct-2506-Q4_K_M.gguf | 14333922848 | a3cc56310807ed0d145eaf9f018ccda9ae7ad8edb41ec870aa2454b0d4700b3c | apache-2.0 |
| ovis-2.5 | llm | ovis-image | Comfy-Org/Ovis-Image @ b7d0cb855e82448c28936c9907a143bf319c80a0 | split_files/text_encoders/ovis_2.5.safetensors | 5140950080 | f453ee5e7a25cb23cf2adf7aae3e5b405f22097cb67f2cfcca029688cb3f740d | apache-2.0 |

**Defaults.** Plan decision, preferring the smaller of equal-quality variants: fp8 or Q8 over fp16. `byVersion`:

| Version | Defaults |
|---|---|
| flux, flex-2 | clip_l clip-l, t5xxl t5xxl-fp8, vae ae-flux1 |
| chroma | t5xxl t5xxl-fp8, vae ae-flux1 |
| chroma-radiance | t5xxl t5xxl-fp8 |
| sd3 | clip_l clip-l, clip_g clip-g, t5xxl t5xxl-fp8 |
| qwen-image | llm qwen2.5-vl-7b-q8, vae qwen-image-vae, llm_vision qwen2.5-vl-7b-mmproj |
| qwen-image-layered | llm qwen2.5-vl-7b-q8, vae qwen-image-vae |
| longcat | llm qwen2.5-vl-7b-q8, vae ae-flux1, llm_vision qwen2.5-vl-7b-mmproj |
| hunyuan-video | llm qwen2.5-vl-7b-q8 |
| anima | vae qwen-image-vae |
| wan2 | t5xxl umt5-xxl-q8, vae wan21-vae, clip_vision clip-vision-h |
| wan2.2-i2v | t5xxl umt5-xxl-q8, vae wan21-vae |
| wan2.2-ti2v | t5xxl umt5-xxl-q8, vae wan22-vae |
| lingbot-video | llm qwen3-vl-4b-q4km, vae wan21-vae |
| z-image | llm qwen3-4b-2507-q8, vae ae-flux1 |
| z-image-l2p | llm qwen3-4b-2507-q8 |
| flux2-klein | llm qwen3-4b-q8, vae flux2-vae |
| flux2 | llm mistral-small-3.2-24b-q4km, vae flux2-vae |
| flux3, fibo2 | llm qwen3-vl-4b-q4km, vae flux2-vae |
| ovis-image | llm ovis-2.5, vae ae-flux1 |
| boogu-image | vae ae-flux1 |
| ernie-image, lens, moss-image, sefi-image, ideogram4, llada-image | vae flux2-vae |
| krea2 | llm qwen3-vl-4b-q4km, vae wan21-vae |
| mage-flow, iris | llm qwen3-vl-4b-q4km |

`byRecipe`: `flux2-klein-9b` and `flux2-klein-base-9b`, both llm qwen3-8b-q8.

Not in the table:
- HunyuanVideo 1.5's VAE and ByT5 (`t5xxl` slot);
- LTX-2.3's `embeddings_connectors`, VAE and audio VAE;
- every tokenizer.

These come from the same repo (Task S3: `Comfy-Org/Hunyuan-Video-1.5` and `unsloth/LTX-2.3-GGUF` ship them), or the person picks a file. A later signed catalog can add them.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[test] fn the_bundled_table_is_valid_and_complete() {
    let t = CompanionTable::from_catalog(&Catalog::bundled()).unwrap();
    assert_eq!(t.files.len(), 22);
    assert_eq!(t.default_for("flux1-dev", SdVersion::Flux, SlotId::T5xxl).unwrap().id, "t5xxl-fp8");
    assert_eq!(t.default_for("flux2-klein-9b", SdVersion::Flux2Klein, SlotId::Llm).unwrap().id, "qwen3-8b-q8");
    assert_eq!(t.default_for("flux2-klein-4b", SdVersion::Flux2Klein, SlotId::Llm).unwrap().id, "qwen3-4b-q8");
    assert_eq!(t.default_for("wan22-ti2v-5b", SdVersion::Wan22Ti2v, SlotId::Vae).unwrap().sha256, "e40321bd36b9709991dae2530eb4ac303dd168276980d3e9bc4b6e2b75fed156");
    assert_eq!(t.licence(&t.default_for("sd35-medium", SdVersion::Sd3, SlotId::ClipG).unwrap().license).unwrap().revenue_limit_usd, Some(1_000_000));
    assert!(t.default_for("hunyuan-video-15-t2v", SdVersion::HunyuanVideo, SlotId::Vae).is_none());
}
#[test] fn a_default_for_the_wrong_slot_or_version_is_refused() {
    let mut c = Catalog::bundled(); c.companions["byVersion"]["sd3"]["t5xxl"] = json!("umt5-xxl-q8");
    assert!(CompanionTable::from_catalog(&c).unwrap_err().contains("umt5-xxl-q8"));
}
#[test] fn the_01x_catalog_parts_are_known_companions_by_sha() {
    // so a generic model reuses what a 0.1.x model installed, even from another repo (Z-Image's ae.safetensors, FastWan's VAE)
    let t = CompanionTable::from_catalog(&Catalog::bundled()).unwrap();
    for (sha, id) in [("f6f851777709861056efcdad3af01da38b31223a3ba26e61a4f8bf3a2195813a", "qwen3-4b-q4km"),
        ("868fe7b343cc8f3a19dbcfcafbc3d5f888802be3f89bd81b65b3621a066ce8f3", "flux2-vae"),
        ("3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597", "qwen3-4b-2507-q4km"),
        ("afc8e28272cd15db3919bacdb6918ce9c1ed22e96cb12c4d5ed0fba823529e38", "ae-flux1"),
        ("eaea358bb438c5d211721a4feecc162000e3636e9cb96f51e216f1f44ebd12ce", "umt5-xxl-q5km"),
        ("e40321bd36b9709991dae2530eb4ac303dd168276980d3e9bc4b6e2b75fed156", "wan22-vae")] {
        assert_eq!(t.by_sha(sha).map(|c| c.id.as_str()), Some(id));
    }
    for e in gen_catalog(&Catalog::bundled(), 1106).entries { for p in &e.parts {
        if let Some(c) = t.by_sha(&p.sha256) { assert_eq!(c.size, p.size, "{}", c.id); } } }
}
```

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p models --lib sd::companions catalog::`
  - Expected: FAIL to compile.

- [ ] **Step 3: Implement.** Add the section to `catalog/models.json`, bump `sequence` to 5 and `generated` to `2026-11-24`, and write `companions.rs`. `release.yml` signs `catalog/models.json` as before (`.github/workflows/release.yml:257-266`), so nothing changes there.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p models --lib`
  - Expected: PASS, including the old-parser catalog test.

- [ ] **Step 5: Commit.**
```bash
git add catalog/models.json crates/models/src/sd/companions.rs crates/models/src/sd/mod.rs crates/models/src/catalog.rs
git commit -m "Studio: a signed table of 22 ungated companion files, with per-version defaults"
```

## Phase S: setting up and running any model

### Task S1: Schema 4 and the LoRA store

**Files:**
- Modify:
  - `crates/store/src/schema.rs` (`V4` appended to `MIGRATIONS`; V1–V3 untouched);
  - `crates/store/src/gen_models.rs` (`recipe_id`, `GenSource::Import`, `gen_model_files` counts LoRA files);
  - `crates/store/src/lib.rs`;
  - `crates/models/src/gen_catalog.rs` (`GenFamily::recipe_id`, `GenFamily::version`);
  - `src-tauri/src/downloads.rs` and `src-tauri/src/studio/local_models.rs`: callers pass `recipe_id`.
- Create: `crates/store/src/gen_loras.rs`
- Test: inline in `schema.rs`, `gen_models.rs`, `gen_loras.rs` and `gen_catalog.rs`

**Interfaces:**
- Produces:
```rust
// crates/store/src/gen_models.rs
sql_enum!(GenSource { Catalog => "catalog", Search => "search", Import => "import" });
pub struct NewGenModel { /* as now */ pub recipe_id: Option<String> }   // None only for a row that still needs Set it up yourself
pub struct GenModel    { /* as now */ pub recipe_id: Option<String> }
// crates/store/src/gen_loras.rs
sql_enum!(LoraFileFormat { Kohya => "kohya", Peft => "peft", Lycoris => "lycoris", Diff => "diff" });
sql_enum!(LoraSource { Search => "search", Import => "import" });
pub struct NewGenLora { pub id: String, pub name: String, pub file: String, pub rel_name: String, pub repo: Option<String>, pub revision: Option<String>,
    pub path: Option<String>, pub size_bytes: u64, pub sha256: String, pub base_hint: Option<String>, pub format: LoraFileFormat, pub source: LoraSource }
pub struct GenLora { /* the same fields */ pub added_at: i64 }
impl Store {
    pub async fn insert_gen_lora(&self, l: NewGenLora) -> Result<GenLora, StoreError>;    // INSERT OR REPLACE by id
    pub async fn list_gen_loras(&self) -> Result<Vec<GenLora>, StoreError>;               // newest first
    pub async fn get_gen_lora(&self, id: &str) -> Result<Option<GenLora>, StoreError>;
    pub async fn delete_gen_lora(&self, id: &str) -> Result<Option<GenLora>, StoreError>;
}
// crates/models/src/gen_catalog.rs
impl GenFamily { pub fn recipe_id(self) -> &'static str; pub fn version(self) -> SdVersion; }
```
- **`V4`**, in one transaction:
```sql
CREATE TABLE gen_models_v4 (
    id TEXT PRIMARY KEY, catalog_id TEXT, name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('image', 'image-edit', 'video')),
    -- a 0.1.x family word, an engine version word (models::sd::SdVersion), or 'unknown'
    family TEXT NOT NULL,
    -- an engine recipe id (catalog/sd-recipes.json); NULL: the model needs Set it up yourself
    recipe_id TEXT,
    parts_json TEXT NOT NULL CHECK (json_valid(parts_json)),
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    source TEXT NOT NULL CHECK (source IN ('catalog', 'search', 'import')),
    added_at INTEGER NOT NULL);
INSERT INTO gen_models_v4 SELECT id, catalog_id, name, kind, family,
    CASE family WHEN 'flux2-klein-4b' THEN 'flux2-klein-4b' WHEN 'flux3-image-klein' THEN 'flux3-klein'
        WHEN 'z-image-turbo' THEN 'z-image-turbo' WHEN 'wan2.2-ti2v-5b' THEN 'wan22-ti2v-5b'
        WHEN 'wan2.2-ti2v-5b-fast' THEN 'wan22-ti2v-5b' WHEN 'wan2.1-t2v-1.3b' THEN 'wan21-t2v' ELSE NULL END,
    parts_json, size_bytes, source, added_at FROM gen_models;
DROP TABLE gen_models;
ALTER TABLE gen_models_v4 RENAME TO gen_models;
CREATE TABLE gen_loras (
    id TEXT PRIMARY KEY,                -- "hf:<owner>/<repo>:<path>" or "local:<sha256>"
    name TEXT NOT NULL,
    file TEXT NOT NULL,                 -- absolute, under <models>/@loras
    rel_name TEXT NOT NULL UNIQUE,      -- relative to <models>/@loras, '/' separators, no extension
    repo TEXT, revision TEXT, path TEXT,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    sha256 TEXT NOT NULL,
    base_hint TEXT,                     -- an engine version word
    format TEXT NOT NULL CHECK (format IN ('kohya', 'peft', 'lycoris', 'diff')),
    source TEXT NOT NULL CHECK (source IN ('search', 'import')),
    added_at INTEGER NOT NULL);
```
- `creation_runs.gen_model_id` has no foreign key (`schema.rs:441`), so the rebuild breaks no reference. `to_latest` already checks `PRAGMA foreign_key_check` after migrating.
- `gen_model_files()` returns the union of every part's `file` and every `gen_loras.file`. Task S10 makes `hub::gen_files_used`, which deletes use (`hub.rs:1477`), count LoRA files too. Then a model delete never removes a LoRA's file, and a LoRA delete never removes a model's.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[test] fn v4_maps_each_01x_family_to_its_recipe_and_keeps_every_row() {
    let conn = at_v3_with_rows(&[("flux2-klein-4b-q4", "flux2-klein-4b", "catalog"), ("z-image-turbo-q4", "z-image-turbo", "catalog"),
        ("wan22-ti2v-5b-fast-q6", "wan2.2-ti2v-5b-fast", "catalog"), ("hf:o/r:w.gguf", "wan2.2-ti2v-5b", "search"),
        ("hf:o/r:w1.gguf", "wan2.1-t2v-1.3b", "search"), ("hf:o/r:f3.gguf", "flux3-image-klein", "search")]);
    let before = dump(&conn, "SELECT id, catalog_id, name, kind, family, parts_json, size_bytes, source, added_at FROM gen_models ORDER BY id");
    migrate_to_latest(&conn);
    assert_eq!(dump(&conn, "SELECT id, catalog_id, name, kind, family, parts_json, size_bytes, source, added_at FROM gen_models ORDER BY id"), before);
    assert_eq!(column(&conn, "SELECT recipe_id FROM gen_models ORDER BY id"),
        ["wan22-ti2v-5b", "flux3-klein", "wan21-t2v", "flux2-klein-4b", "wan22-ti2v-5b", "z-image-turbo"]);   // ids sorted
    assert!(table_exists(&conn, "gen_loras"));
}
#[test] fn import_is_a_source_and_others_are_refused() {
    let s = store(); insert(&s, model("local:ab", GenSource::Import, Some("sdxl-checkpoint")));
    assert!(raw_insert(&s, "source", "elsewhere").is_err());
}
#[test] fn a_database_newer_than_v4_is_refused() { /* user_version 5 → StoreError::TooNew, as for V3 */ }
#[tokio::test] async fn loras_round_trip_and_rel_names_are_unique() {
    let s = Store::open_in_memory().await.unwrap();
    s.insert_gen_lora(lora("local:1", "local/pixel art ü")).await.unwrap();
    assert!(s.insert_gen_lora(lora("local:2", "local/pixel art ü")).await.is_err());
    assert_eq!(s.list_gen_loras().await.unwrap()[0].rel_name, "local/pixel art ü");
}
#[tokio::test] async fn lora_files_count_as_used_files() {
    let s = Store::open_in_memory().await.unwrap(); s.insert_gen_lora(lora_at("/m/@loras/local/x.safetensors")).await.unwrap();
    assert!(s.gen_model_files().await.unwrap().contains("/m/@loras/local/x.safetensors"));
}
#[test] fn each_01x_family_names_its_recipe_and_version() {   // gen_catalog.rs
    use GenFamily::*;
    assert_eq!([Flux2Klein4b, Flux3ImageKlein, ZImageTurbo, Wan22Ti2v5b, Wan22Ti2v5bFast, Wan21T2v1_3b].map(|f| (f.recipe_id(), f.version())),
        [("flux2-klein-4b", SdVersion::Flux2Klein), ("flux3-klein", SdVersion::Flux3), ("z-image-turbo", SdVersion::ZImage),
         ("wan22-ti2v-5b", SdVersion::Wan22Ti2v), ("wan22-ti2v-5b", SdVersion::Wan22Ti2v), ("wan21-t2v", SdVersion::Wan2)]);
    for f in GenFamily::ALL { assert!(models::sd::recipe(f.recipe_id()).is_some()); }
}
```
`GenFamily::ALL` (the six variants, in declaration order) is added with this test.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p store && cargo test -p models --lib gen_catalog::`
  - Expected: FAIL to compile, with `no field recipe_id`.

- [ ] **Step 3: Implement.**
  - `register_gen_model` and the catalog download path set `recipe_id: Some(family.recipe_id())`.
  - The only search rows 0.1.x makes are of the six families, so they map too.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p store && cargo test -p models --lib && cargo test -p app --lib downloads::`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add crates/store crates/models/src/gen_catalog.rs src-tauri/src/downloads.rs src-tauri/src/studio/local_models.rs
git commit -m "Store: schema 4, with each Studio model's recipe, imported models and LoRAs"
```

### Task S2: Engine arguments from a recipe

**Files:**
- Modify:
  - `crates/runtime/src/sd/args.rs`, `crates/runtime/src/sd/run.rs` and `crates/runtime/src/sd/testing.rs`;
  - `crates/runtime/tests/sd_contract.rs`;
  - `src-tauri/src/studio/local.rs`: compile only, as below. Task S11 does the real change.
- Test: inline in `args.rs` and `run.rs`; `crates/runtime/tests/sd_contract.rs`

**Interfaces:**
- Produces (`args.rs`):
```rust
pub enum PartRole { Diffusion, HighNoiseDiffusion, UncondDiffusion, Llm, LlmVision, T5xxl, ClipL, ClipG, ClipVision, Tokenizer,
                    Vae, Tae, AudioVae, AudioEncoder, EmbeddingsConnectors, Single }
pub fn flag_for(role: PartRole) -> &'static str;   // 7.1's table (Single → "-m")
pub enum Sampler { Euler, EulerA, Heun, Dpm2, DpmPp2sA, DpmPp2m, DpmPp2mV2, Ipndm, IpndmV, Lcm, DdimTrailing, Tcd, ResMultistep, Res2s,
                   ErSde, EulerCfgPp, EulerACfgPp, EulerGe, DpmPp2mSde, DpmPp2mSdeBt, Lms }          // 21, `ALL` in this order
pub enum Scheduler { Discrete, Karras, Exponential, Ays, Gits, SgmUniform, Simple, Smoothstep, KlOptimal, Lcm, BongTangent, Ltx2,
                     LogitNormal, Flux2, Flux, Beta, LladaImage }                                       // 17, `ALL` in this order
pub enum VaeFormat { Flux, Sd3, Flux2, Wan }        // as_str "flux" | "sd3" | "flux2" | "wan"; "auto" is never sent
pub struct HighNoise { pub steps: i32, pub cfg: f32, pub sampler: Option<Sampler> }
pub struct LoraTag { pub name: String, pub weight: f32, pub high_noise: bool }
pub const MAX_LORAS: usize = 8;
pub const LORA_WEIGHT_RANGE: std::ops::RangeInclusive<f32> = -1.0..=2.0;
pub struct SdJob {
    /* unchanged fields, except: */
    // `memory_saving: bool` is replaced by:
    pub vae_tiling: bool, pub offload_to_cpu: bool,
    pub size_multiple: u32,                       // the recipe's (8 when omitted); replaces SIZE_MULTIPLE 16
    pub guidance: Option<f32>, pub img_cfg: Option<f32>, pub timestep_shift: Option<u32>, pub sigmas: Vec<f32>,
    pub high_noise: Option<HighNoise>, pub moe_boundary: Option<f32>, pub strength: Option<f32>, pub end_image: Option<PathBuf>,
    pub mask_image: Option<PathBuf>, pub control_image: Option<PathBuf>,       // --mask, --control-image (design 11a)
    pub vae_format: Option<VaeFormat>, pub model_args: Option<String>, pub lora_dir: Option<PathBuf>, pub loras: Vec<LoraTag>,
}
pub enum ArgsError { /* as now, with */ BadSize { multiple: u32 }, BadLoraName(String), TooManyLoras, LorasWithoutFolder, BadModelArgs, MaskWithoutPicture }
pub fn strip_lora_tags(prompt: &str) -> String;     // removes every span matching <lora:([^:>]+):([^>]+)>
pub fn lora_tag(tag: &LoraTag) -> String;           // "<lora:NAME:W>" or "<lora:|high_noise|NAME:W>", W by `{}` of f32
pub fn prompt_file_text(job: &SdJob) -> String;     // strip_lora_tags(prompt), then " " + lora_tag(t) for each LoRA
pub fn valid_lora_name(name: &str) -> bool;         // non-empty, relative, '/' separators only, no ':' '<' '>' '|' '\\', no control
                                                    // characters, no "." or ".." component
```
- **`build_args` order.** The new flags sit in this order:
  1. the parts (one fixed flag each), `-M vid_gen` when `frames` is set, then the prompt files;
  2. `-W`, `-H`, `--steps`, `--cfg-scale`;
  3. **`--guidance`, `--img-cfg-scale`**, `--flow-shift`, **`--timestep-shift`**, `--sampling-method`, `--scheduler`;
  4. **`--sigmas`** (comma-joined), **`--high-noise-steps`, `--high-noise-cfg-scale`, `--high-noise-sampling-method`, `--moe-boundary`, `--strength`**;
  5. `--seed`, `--batch-count`, `--video-frames`, `--fps`, `-i`, **`--end-img`, `--mask`, `--control-image`**, each `-r`;
  6. **`--vae-format`, `--model-args`, `--lora-model-dir`**;
  7. `--backend`, `--threads`, `--vae-tiling` (when `vae_tiling`), `--offload-to-cpu` (when `offload_to_cpu`), then `-o`, `--disable-image-metadata` and `--log-level info`.
- `FLAGS` lists every flag in that order, so the contract tests check each against the pinned help.
- **Checks:**
  - Sizes are multiples of `size_multiple`, from 64 to 2048.
  - `size_multiple` 0 is refused as `BadSize`.
  - `model_args` must be `key=value` pairs joined by `,`, matching `[a-z0-9_]+=[A-Za-z0-9_.]+`.
  - LoRAs need `lora_dir`, at most `MAX_LORAS`, a valid name each, and a weight in `LORA_WEIGHT_RANGE`.
- `run.rs`: `write_prompt_files` writes `prompt_file_text(job)`: UTF-8, no trailing newline. `with_retry` sets `vae_tiling = true` and `offload_to_cpu = true` on its one out-of-memory retry, which was `memory_saving = true`.
- `src-tauri/src/studio/local.rs::job_for` keeps compiling:
  - `vae_tiling` and `offload_to_cpu` both from `opts.memory_saving`;
  - `size_multiple: entry.limits.multiple_of`;
  - every new field empty.
- A `mask_image` without an `init_image` is refused (`MaskWithoutPicture`): the engine would repaint the whole picture (`src/pipeline/image.cpp:274-281`). `--control-strength` is never sent; it is for ControlNet files, which stay out.
- The full sampler and scheduler lists now reach `params_for` through `Sampler::ALL` and `Scheduler::ALL`. That is 7.6's change, and `local.rs`'s tests that count them are updated to 21 and 17.
- `local.rs`'s `sampler_label` and `scheduler_label` need a plain label for each new word, so their matches stay exhaustive.
  - samplers: "DPM++ 2S a", "iPNDM", "iPNDM v", "DDIM trailing", "TCD", "RES multistep", "RES 2S", "ER-SDE", "Euler CFG++", "Euler a CFG++", "Euler GE", "DPM++ 2M SDE", "DPM++ 2M SDE BT", "LMS";
  - schedulers: "AYS", "GITS", "SGM uniform", "Smoothstep", "KL optimal", "LCM", "Bong tangent", "LTX-2", "Logit normal", "FLUX.2", "FLUX", "Beta", "LLaDA-Image".

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[test] fn a_0_1_2_job_builds_the_same_arguments() {
    // the existing `the_arguments_come_in_their_fixed_order` vector for `job()` (args.rs:1059) stays byte for byte,
    // with every new field empty and vae_tiling = offload_to_cpu = false
    assert_eq!(build_args(&job()).unwrap(), os(&FIXED_ORDER_0_1_2));            // the literal of that test, unchanged
}
#[test] fn recipe_fields_reach_their_flags_in_order() {
    let mut j = wan_a14b_job();     // two diffusion parts, t5xxl, vae, frames 33, fps 16, size_multiple 16
    j.guidance = Some(3.5); j.high_noise = Some(HighNoise { steps: -1, cfg: 4.0, sampler: Some(Sampler::Euler) }); j.moe_boundary = Some(0.875);
    j.sigmas = vec![1.0, 0.5, 0.0]; j.vae_format = Some(VaeFormat::Wan); j.model_args = Some("qwen_image_zero_cond_t=true".into());
    let a = strings(build_args(&j).unwrap());
    assert_eq!(after(&a, "--high-noise-steps"), "-1"); assert_eq!(after(&a, "--sigmas"), "1,0.5,0");
    assert!(index(&a, "--guidance") < index(&a, "--flow-shift") && index(&a, "--moe-boundary") < index(&a, "--seed"));
    assert_eq!(after(&a, "-M"), "vid_gen");
}
#[test] fn the_size_follows_the_recipes_multiple() {
    let mut j = klein_job(); j.size_multiple = 64; j.width = 1000;
    assert_eq!(build_args(&j), Err(ArgsError::BadSize { multiple: 64 }));
    j.size_multiple = 8; assert!(build_args(&j).is_ok());
}
#[test] fn every_sampler_and_scheduler_is_the_engines_word() {
    assert_eq!(Sampler::ALL.len(), 21); assert_eq!(Scheduler::ALL.len(), 17);
    let help = include_str!("../../tests/fixtures/sd_help.txt");
    for s in Sampler::ALL { assert!(help.contains(s.as_str()), "{}", s.as_str()); }
    for s in Scheduler::ALL { assert!(help.contains(s.as_str()), "{}", s.as_str()); }
}
#[test] fn typed_lora_tags_are_removed_and_attached_ones_appended() {
    let mut j = klein_job(); j.prompt = "a cat <lora:evil:3> on a mat".into(); j.lora_dir = Some("/m/@loras".into());
    j.loras = vec![LoraTag { name: "local/pixel art ü".into(), weight: 0.8, high_noise: false }];
    assert_eq!(prompt_file_text(&j), "a cat  on a mat <lora:local/pixel art ü:0.8>");
    assert_eq!(after(&strings(build_args(&j).unwrap()), "--lora-model-dir"), "/m/@loras");
}
#[test] fn mask_and_control_pictures_reach_their_flags() {
    let mut j = klein_job(); j.init_image = Some("/r/in.png".into()); j.mask_image = Some("/r/mask.png".into()); j.control_image = Some("/r/edges.png".into());
    let a = strings(build_args(&j).unwrap());
    assert_eq!((after(&a, "-i"), after(&a, "--mask"), after(&a, "--control-image")), ("/r/in.png", "/r/mask.png", "/r/edges.png"));
    j.init_image = None; assert_eq!(build_args(&j), Err(ArgsError::MaskWithoutPicture));
}
#[test] fn lora_names_the_engine_could_misread_are_refused() {
    for bad in ["", "C:/x", "a>b", "a<b", "|high_noise|x", "../x", "a\\b", "a/./b"] { assert!(!valid_lora_name(bad), "{bad}"); }
    for good in ["local/pixel art ü", "o/r/日本語 スタイル", "x"] { assert!(valid_lora_name(good), "{good}"); }
    let mut j = klein_job(); j.loras = vec![LoraTag { name: "x".into(), weight: 1.0, high_noise: false }];
    assert_eq!(build_args(&j), Err(ArgsError::LorasWithoutFolder));
    j.lora_dir = Some("/m/@loras".into()); j.loras[0].weight = 2.5;
    assert_eq!(build_args(&j), Err(ArgsError::NumberOutOfRange("lora weight")));
}
// run.rs
#[test] fn lora_tags_reach_the_prompt_file_with_unicode_names() {
    let dir = tempfile::tempdir().unwrap(); let mut j = klein_job_in(dir.path()); j.lora_dir = Some(dir.path().join("@loras"));
    j.loras = vec![LoraTag { name: "local/pixel art ü".into(), weight: 0.8, high_noise: false },
                   LoraTag { name: "o/r/日本語 スタイル".into(), weight: 1.0, high_noise: true }];
    write_prompt_files(&j).unwrap();
    let text = std::fs::read(dir.path().join("prompt.txt")).unwrap();
    assert_eq!(String::from_utf8(text.clone()).unwrap(), "a red fox <lora:local/pixel art ü:0.8> <lora:|high_noise|o/r/日本語 スタイル:1>");
    // the engine's pattern <lora:([^:>]+):([^>]+)> (common.cpp:2356) finds both, names intact
    assert_eq!(engine_lora_spans(&text), [("local/pixel art ü".into(), "0.8".into()), ("|high_noise|o/r/日本語 スタイル".into(), "1".into())]);
}
#[tokio::test] async fn the_out_of_memory_retry_turns_on_tiling_and_offload() { /* fake engine: first run OOM; the second's args hold --vae-tiling and --offload-to-cpu */ }
```
In `sd_contract.rs`, which needs `SD_CLI_BIN` like its neighbours:
- `the_pinned_engine_parses_recipe_arguments`: builds `wan_a14b_job()` with every new field set (a LoRA tag and a tiny `@loras` folder included), and a picture job with `-i`, `--mask` and `--control-image` (tiny PNGs). It runs the engine in metadata mode (`-M metadata --image png`) as `the_pinned_engine_parses_the_exact_arguments_studio_builds` does, and asserts exit 0 and no `unknown argument` or `invalid` line.
- `the_pinned_engine_knows_every_flag_studio_sends` keeps covering the new `FLAGS` unchanged.

`engine_lora_spans` is a test helper that applies the engine's pattern by hand (runtime has no `regex` dependency).

**Record the 0.1.2 klein vector first.** Before any change, add `an_01x_klein_picture_keeps_its_0_1_2_arguments` (its body is in Task S11) to `src-tauri/src/studio/local.rs`'s tests:
- run it once against the unchanged code;
- paste the printed vector as the constant `KLEIN_0_1_2_ARGS` in that test module;
- see it pass.

It must pass at the end of this task and of every later one (Review Focus 5).

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p runtime --lib sd::`
  - Expected: FAIL to compile, with `no field vae_tiling`.

- [ ] **Step 3: Implement** the signatures and the order above. `with_retry` is as described. `local.rs` gets the compile changes only.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p runtime && cargo test -p app --lib studio::local`
  - Expected: PASS. In CI, `sd_contract` runs against the Linux `sd-cli` built by `scripts/fetch_sd.py`.

- [ ] **Step 5: Commit.**
```bash
git add crates/runtime src-tauri/src/studio/local.rs
git commit -m "Studio engine: arguments for any recipe: 16 part flags, the full sampler and scheduler lists, LoRAs by file"
```

### Task S3: Generic models: 16 roles, the recipe on every entry, fit by kind

**Files:**
- Modify:
  - `crates/models/src/gen_catalog.rs`;
  - `crates/models/src/gen_fit.rs`: tests only. Generic entries go through the same `Need::of`.
  - `src-tauri/src/downloads.rs`: `check_diffusion_file` is replaced by `check_main_file`.
  - `src-tauri/src/studio/local_models.rs`: `entry_from_installed` builds generic entries.
  - `src-tauri/src/studio/local.rs`: `part_role` covers 16 roles; `check_files` uses `check_main_file`.
  - `crates/models/src/gen_search.rs`: remove `TensorSignature`, `SIGNATURES` and `family_from_tensors`; their only callers move to `check_main_file`.
- Test: inline

**Interfaces:**
- Consumes: Tasks R1, R5, R6 and R7; `GenFamily::recipe_id` (Task S1); `runtime::sd::args::PartRole` (Task S2).
- Produces:
```rust
// gen_catalog.rs
pub enum GenRole { Diffusion, HighNoiseDiffusion, UncondDiffusion, Llm, LlmVision, T5xxl, ClipL, ClipG, ClipVision, Tokenizer, Vae, Tae,
                   AudioVae, AudioEncoder, EmbeddingsConnectors, Model }   // kebab words: …, "uncond-diffusion", "clip-g", "llm-vision",
                                                                           // "tokenizer", "tae", "audio-vae", "audio-encoder", "embeddings-connectors"
impl GenRole { pub fn from_slot(slot: SlotId) -> Option<GenRole>; pub fn slot(self) -> SlotId; }   // Model ↔ model, Diffusion ↔ diffusion_model, …
pub struct GenEntry { /* … */ pub family: Option<GenFamily>, #[serde(default)] pub recipe: String, #[serde(default)] pub version: Option<SdVersion>,
                      #[serde(default)] pub untested: bool, /* … */ }   // catalog entries: recipe and version filled from family after parse
pub struct HighNoiseDefaults { pub steps: i32, pub cfg: f32, pub sampler: Option<String> }
pub struct GenDefaults { /* as now, plus, all #[serde(default)] */ pub guidance: Option<f32>, pub img_cfg: Option<f32>, pub timestep_shift: Option<u32>,
                         pub sigmas: Vec<f32>, pub high_noise: Option<HighNoiseDefaults>, pub moe_boundary: Option<f32>, pub strength: Option<f32> }
pub struct GenLimits { /* as now, plus */ #[serde(default)] pub video: Option<VideoRule> }
impl GenLimits { pub fn video_rule(&self) -> Option<VideoRule>; }   // `video`, else frame_rule 4n+1 → step 4 offset 1 floor, 8n+1 → step 8 offset 1
pub struct GenPart { /* as now, plus */ #[serde(default)] pub local: Option<PathBuf> }   // a file from disk: repo and revision empty
pub const ALLOWED_PART_EXTENSIONS: [&str; 3] = [".gguf", ".safetensors", ".sft"];     // and ".json" for the tokenizer role only
pub struct GenericSpec<'a> { pub id: String, pub name: String, pub publisher: String, pub recipe: &'a Recipe, pub version: Option<SdVersion>,
    pub parts: Vec<GenPart>, pub licenses: BTreeMap<String, GenLicence>, pub gate: GenGate, pub untested: bool }
pub fn generic_entry(spec: GenericSpec<'_>) -> GenEntry;
pub fn fallback_for(recipe: &Recipe) -> FitFallback;
// src-tauri/src/downloads.rs
pub(crate) fn check_main_file(entry: &GenEntry, path: &Path) -> AppResult<()>;   // inspect_local: a Model of entry.version (or, untested, any readable weights)
```
**`generic_entry` rules:**
- **kind:** `kind_of(recipe)`. Picture → `image`, PictureEdit → `image-edit`, Video → `video`.
- **inputs:** one `GenInput` per `inputs_of(recipe)` row (Task R7). `GenInput` gains `Source`, `Mask` and `Control` (serde `source`, `mask`, `control`) beside `Reference`, `StartFrame` and `EndFrame`, and `GenEntry` gains `#[serde(default)] pub input_counts: Vec<RecipeInput>` with each row's min and max. Catalog entries keep their own `inputs`, with counts as in 0.1.2.
- **defaults** come from the recipe, field for field:
  - `sampling_method` → `sampler`, `distilled_guidance` → `guidance`, `img_cfg_scale` → `img_cfg`;
  - `shifted_timestep` → `timestep_shift`, `custom_sigmas` → `sigmas`;
  - the `high_noise_*` fields → `high_noise`, `video_frames` → `frames`.
  - When the recipe omits them: steps 20 and cfg 7.0 (the engine's own defaults), and 512×512 for sd1 and sd2 versions, else 1024×1024. Plan decision: `instruct-pix2pix`, `cosxl-edit` and `flux1-kontext` give no size.
- **limits:** `multiple_of = recipe.size_multiple()`, `max_side` 2048, `video = recipe.video()`, `max_frames = video.max_frames`.
- **quirks:** `[MetalTensorOff]` for z-image and z-image-l2p, the 0.1.x Z-Image quirk; otherwise none.
- **the rest:** `min_build` 1106, `runs_on_apple` true, no `memory` or `speed` points, `tested` None.
- **`fallback_for`.** Plan decision: the 0.1.x factors of the nearest catalog entry, rounded up.
  - pictures: `act_base_bytes` 500_000_000, `act_bytes_per_token` 100_000, `vae_bytes_per_pixel_frame` 300, `vae_tile_cap_bytes` 500_000_000; `token_div` 32 when `size_multiple ≥ 32`, else 16; `frame_div` and `vae_chunk_frames` None.
  - video: 1_000_000_000, 250_000, 400 and 2_500_000_000; `token_div` as for pictures; `frame_div = Some(video.frame_step)`, `vae_chunk_frames = Some(4)`.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[test] fn catalog_entries_carry_their_recipe_and_version() {
    let c = gen_catalog(&Catalog::bundled(), 1106);
    let klein = c.entries.iter().find(|e| e.id == "flux2-klein-4b-q4").unwrap();
    assert_eq!((klein.recipe.as_str(), klein.version), ("flux2-klein-4b", Some(SdVersion::Flux2Klein)));
    assert_eq!(klein.defaults.steps, 4);   // the catalog's tested defaults stay; recipe defaults are for generic models only
}
#[test] fn a_generic_sdxl_entry_takes_the_recipes_defaults_and_limits() {
    let e = generic_entry(spec(recipe("sdxl-checkpoint").unwrap(), Some(SdVersion::Sdxl), vec![part(GenRole::Model, "x.safetensors", 6_938_078_334)]));
    assert_eq!((e.kind, e.defaults.steps, e.defaults.cfg, e.defaults.width, e.limits.multiple_of), (GenKind::Image, 20, 5.0, 1024, 64));
    assert_eq!(e.fallback.token_div, 32);
}
#[test] fn a_generic_wan_a14b_entry_is_a_video_with_two_diffusion_parts() {
    let e = generic_entry(spec(recipe("wan22-t2v").unwrap(), Some(SdVersion::Wan2), wan_a14b_parts()));
    assert_eq!(e.kind, GenKind::Video);
    assert_eq!(e.limits.video_rule().unwrap().frame_step, 4);
    assert_eq!(e.defaults.high_noise.as_ref().unwrap().cfg, 4.0);
    assert_eq!(e.defaults.moe_boundary, Some(0.875));
}
#[test] fn the_tokenizer_role_takes_a_json_file_and_no_other_role_does() {
    assert!(check_parts(&[part(GenRole::Tokenizer, "tokenizer.json", 1)]).is_ok());
    assert!(check_parts(&[part(GenRole::Vae, "vae.json", 1)]).is_err());
    assert!(check_parts(&[part(GenRole::Diffusion, "m.sft", 1)]).is_ok());
}
#[test] fn the_01x_family_signature_check_now_uses_detection() {      // downloads.rs
    let klein = catalog_entry("flux2-klein-4b-q4");
    assert!(check_main_file(&klein, &write_gguf_txt_fixture("klein4b")).is_ok());
    assert_eq!(check_main_file(&klein, &write_gguf_txt_fixture("zimage")).unwrap_err().code, errors::NOT_THAT_MODEL);
}
#[test] fn every_slot_has_a_role_and_back() { for s in SlotId::RUNNABLE { assert_eq!(GenRole::from_slot(s).unwrap().slot(), s); } }
#[test] fn an_inpainting_entry_takes_a_picture_to_change_and_a_mask() {
    let e = generic_entry(spec(recipe("sdxl-inpainting").unwrap(), Some(SdVersion::SdxlInpaint), vec![part(GenRole::Model, "x.safetensors", 1)]));
    assert_eq!((e.kind, e.inputs.clone()), (GenKind::ImageEdit, vec![GenInput::Source, GenInput::Mask]));
}
```
`studio_not_that_model` is the existing tensor-check refusal (`errors::NOT_THAT_MODEL`, inventory S25).

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p models --lib gen_catalog:: && cargo test -p app --lib downloads::`
  - Expected: FAIL to compile.

- [ ] **Step 3: Implement.**
  - `entry_from_installed` uses the catalog entry when `catalog_id` is set.
  - Otherwise it uses `generic_entry` with `recipe(record.recipe_id)` and `SdVersion::from_word(&record.family)`, and `untested` when `family == "unknown"` or the recipe is not in `recipes_for(version)`.
  - It returns `None` when `recipe_id` is NULL. Such a model is listed with *Set it up yourself* (Task S9).

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p models && cargo test -p app --lib`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add crates/models src-tauri/src/downloads.rs src-tauri/src/studio
git commit -m "Studio: generic models from a recipe: 16 part roles, recipe defaults and limits, a fit estimate by kind"
```

### Task S4: The set-up planner

**Files:**
- Create: `crates/models/src/sd/setup.rs`
- Test: inline, with Task R4 fixtures as the files' facts.

**Interfaces:**
- Consumes: `Recipe` (Task R1), `FileFacts` (Task R6), `noise_part` (Task R7), `CompanionTable` (Task R8), `RepoFile` (`files.rs:19`), `quant_label` (`files.rs:95`).
- Produces:
```rust
#[derive(Debug, Clone, PartialEq)] pub enum PlannedFile {
    Hub { repo: String, revision: String, path: String, size: u64, sha256: String, licence: LicenceRef },
    Local { path: PathBuf, size: u64, sha256: Option<String> } }
#[derive(Debug, Clone, PartialEq, Eq)] pub enum LicenceRef { Repo, Companion(String) }   // the main repo's licence, or a companion licence id
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub enum PartSource { Main, Sibling, Installed, Repo, Companion, Chosen, Embedded, Missing }
#[derive(Debug, Clone, PartialEq)] pub struct PlannedPart { pub slot: SlotId, pub required: bool, pub source: PartSource, pub file: Option<PlannedFile> }
#[derive(Debug, Clone, PartialEq)] pub struct SetupPlan { pub recipe_id: String, pub parts: Vec<PlannedPart>, pub untested: bool }
impl SetupPlan { pub fn missing(&self) -> Vec<SlotId>; pub fn can_run(&self) -> bool; pub fn to_fetch(&self) -> Vec<&PlannedFile>; }
pub struct InstalledFile { pub slot: SlotId, pub path: PathBuf, pub sha256: String, pub size: u64 }
pub enum SlotChoice { Hub { repo: String, revision: String, file: RepoFile }, Local { path: PathBuf, size: u64, sha256: Option<String> }, Empty }
pub enum MainFile<'a> { Hub { repo: &'a str, revision: &'a str, file: &'a RepoFile }, Local { path: &'a Path, size: u64, sha256: Option<&'a str> } }
pub struct SetupInput<'a> {
    pub main: MainFile<'a>, pub facts: &'a FileFacts, pub recipe: &'a Recipe, pub version: Option<SdVersion>, pub part: Option<NoisePart>,
    pub listing: &'a [RepoFile], pub listing_facts: &'a BTreeMap<String, FileFacts>,     // the same repo; empty for a local file
    pub installed: &'a [InstalledFile], pub companions: &'a CompanionTable, pub overrides: &'a [(SlotId, SlotChoice)], pub untested: bool }
pub fn plan_setup(input: &SetupInput<'_>) -> SetupPlan;
```
**The rules, in 7.4's order:**
1. **The main file.**
   - `Checkpoint` layout: it goes into `model`. If the recipe's main slot is `diffusion_model`, `model` takes its place. Plan decision: the engine's `-m` takes a full checkpoint for any version.
   - `DiffusionOnly`: it goes into the recipe's main slot.
   - For a high-noise part (`part == Some(High)`), it goes into `high_noise_diffusion_model`.
2. **High/low siblings.**
   - When the recipe has `high_noise_diffusion_model`, the other slot takes the listing file of the other `noise_part` with the same `quant_label` and the same folder depth.
   - If none has the same quant, it takes the smallest of the other part. That is the `Sibling` source.
3. **Every other slot that is required, plus `vae` for `required_any [[vae, tae]]` when neither is filled,** is filled from the first of:
   - **Embedded:** the checkpoint embeds it.
   - **Installed:** an installed file with a companion's SHA-256 that suits the version, or with the SHA-256 the next two sources would pick. A slot's default or any variant of it counts, so 0.1.x installs are reused.
   - **Repo:** a listing file whose facts say `Extra { slot }`, or whose name says so (`companion_by_name`, Task S5). The smallest wins; on a tie, the same quant label as the main file. Files without a SHA-256 are skipped.
   - **Companion:** `companions.default_for(recipe, version, slot)`.
   - **Missing.**
4. **Optional slots.**
   - `llm_vision` and `clip_vision` are filled by the same order when the recipe takes an input picture (`feature("ref_images")` or `feature("init_image")`). Plan decision: the engine reads the reference through them.
   - Every other optional slot is filled only by Embedded or an override.
5. **Overrides** replace the slot's choice with source `Chosen`. `Empty` clears an optional slot, or makes a required one `Missing`.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[test] fn a_flux_diffusion_file_gets_its_three_companions() {
    let p = plan(main_hub("QuantStack/FLUX.1-Kontext-dev-GGUF", "flux1-kontext-dev-Q4_K_M.gguf", "flux1-kontext"), "flux1-kontext", &[], &[]);
    assert_eq!(sources(&p), [(SlotId::DiffusionModel, PartSource::Main), (SlotId::ClipL, PartSource::Companion),
                             (SlotId::T5xxl, PartSource::Companion), (SlotId::Vae, PartSource::Companion)]);
    assert_eq!(hub_path(&p, SlotId::T5xxl), "t5xxl_fp8_e4m3fn.safetensors");
    assert!(p.can_run() && !p.untested);
}
#[test] fn an_sd15_checkpoint_needs_nothing_else() {
    let p = plan(main_hub("stable-diffusion-v1-5/stable-diffusion-v1-5", "v1-5-pruned-emaonly.safetensors", "sd15"), "sd1-checkpoint", &[], &[]);
    assert_eq!(sources(&p), [(SlotId::Model, PartSource::Main)]);
}
#[test] fn sd35_medium_embeds_its_vae_and_fetches_three_encoders() {
    let p = plan(main_hub("second-state/stable-diffusion-3.5-medium-GGUF", "sd3.5_medium.safetensors", "sd35-medium"), "sd35-medium", &[], &[]);
    assert_eq!(sources(&p), [(SlotId::Model, PartSource::Main), (SlotId::ClipL, PartSource::Companion), (SlotId::ClipG, PartSource::Companion),
                             (SlotId::T5xxl, PartSource::Companion), (SlotId::Vae, PartSource::Embedded)]);
}
#[test] fn a_wan_a14b_high_noise_file_brings_its_low_noise_sibling_of_the_same_quant() {
    let listing = wan_a14b_listing(&["Q4_K_M", "Q8_0"]);   // HighNoise/… and LowNoise/… for each
    let p = plan_with(main_hub_part("QuantStack/Wan2.2-T2V-A14B-GGUF", "HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf", "wan22-t2v-a14b-high", NoisePart::High),
                      "wan22-t2v", &listing, &[]);
    assert_eq!(hub_path(&p, SlotId::HighNoiseDiffusionModel), "HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf");
    assert_eq!(hub_path(&p, SlotId::DiffusionModel), "LowNoise/Wan2.2-T2V-A14B-LowNoise-Q4_K_M.gguf");
    assert_eq!(source(&p, SlotId::DiffusionModel), PartSource::Sibling);
}
#[test] fn an_installed_01x_encoder_is_reused_instead_of_a_download() {
    let installed = [installed_file(SlotId::Llm, "f6f851777709861056efcdad3af01da38b31223a3ba26e61a4f8bf3a2195813a")];   // klein's Qwen3-4B Q4_K_M
    let p = plan(main_hub("o/FLUX.2-klein-4B-GGUF", "flux-2-klein-4b-Q8_0.gguf", "flux2-klein-4b"), "flux2-klein-4b", &[], &installed);
    assert_eq!(source(&p, SlotId::Llm), PartSource::Installed);
    assert!(p.to_fetch().iter().all(|f| !matches!(f, PlannedFile::Hub { path, .. } if path.contains("Qwen3"))));
}
#[test] fn a_file_in_the_same_repo_beats_a_companion() {
    // Comfy-Org/Hunyuan-Video-1.5 ships its VAE and ByT5: both come from the repo, the llm from the table
    let p = plan_with(main_hub("Comfy-Org/Hunyuan-Video-1.5", "split_files/diffusion_models/hunyuanvideo1.5_720p_t2v_fp16.safetensors", "hunyuan-video-15"),
                      "hunyuan-video-15-t2v", &hunyuan_listing(), &[]);
    assert_eq!((source(&p, SlotId::Vae), source(&p, SlotId::T5xxl), source(&p, SlotId::Llm)), (PartSource::Repo, PartSource::Repo, PartSource::Companion));
}
#[test] fn a_slot_nothing_can_fill_is_missing_and_the_plan_cannot_run() {
    let p = plan(main_hub("o/HunyuanVideo-1.5-GGUF", "h-Q4_0.gguf", "hunyuan-video-15"), "hunyuan-video-15-t2v", &[], &[]);
    assert_eq!(p.missing(), [SlotId::T5xxl, SlotId::Vae]);
    assert!(!p.can_run());
}
#[test] fn the_vision_encoder_is_added_for_an_edit_recipe_only() {
    assert_eq!(source(&plan(main_hub("o/r", "q.gguf", "qwen-image-edit-2509"), "qwen-image-edit-2509", &[], &[]), SlotId::LlmVision), PartSource::Companion);
    assert!(slot(&plan(main_hub("o/r", "q.gguf", "qwen-image"), "qwen-image", &[], &[]), SlotId::LlmVision).is_none());
}
#[test] fn an_override_wins_and_empty_makes_a_required_slot_missing() {
    let p = plan_overriding(SlotId::Vae, SlotChoice::Local { path: "/d/ae.safetensors".into(), size: 1, sha256: None });
    assert_eq!(source(&p, SlotId::Vae), PartSource::Chosen);
    assert_eq!(plan_overriding(SlotId::T5xxl, SlotChoice::Empty).missing(), [SlotId::T5xxl]);
}
```

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p models --features test-support --lib sd::setup`
  - Expected: FAIL to compile.

- [ ] **Step 3: Implement `plan_setup`** by the rules. Parts are listed in the recipe's slot order, the slots of the plan only.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p models --features test-support --lib sd::setup`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add crates/models/src/sd
git commit -m "Studio: plan a model's set-up: main file, high/low sibling, installed files, the same repo, companions"
```

### Task S5: Reading a repository when it opens, and search verdicts

**Files:**
- Create:
  - `crates/models/src/sd/candidates.rs`;
  - `src-tauri/src/studio/detect.rs`: `DetectService`, kept in Tauri state beside `StudioManager`.
- Modify:
  - `crates/models/src/sd/mod.rs`: `FileLabel`, `CannotRunReason`, `NotRead`, `Verdict`, `label_for`.
  - `crates/models/src/gen_search.rs`: `verdict_hint` replaces `recognise` and `family_of_name`; the token table is below.
  - `crates/models/src/hf.rs`: `GenSearchKind::Loras` and its three URLs; ranking by `verdict_hint`.
  - `crates/models/src/files.rs`: `quant_label` also strips `.safetensors` and `.sft`.
  - `src-tauri/src/lib.rs`: manage the `DetectService`.
- Test: inline in `candidates.rs`, `detect.rs`, `gen_search.rs` and `hf.rs`

**Interfaces:**
- Consumes: `read_remote` and `MAX_FILES_PER_REPO` (Task R3); `inspect` (Task R6); `match_recipe` and `noise_part` (Task R7); `RepoDetail` (`hf.rs:177`).
- Produces:
```rust
// sd/mod.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub enum CannotRunReason { Lumina2, LtxVideo09, HunyuanVideo1, Svd, UnknownWeights, Sharded, Pickle, NoSha, Adapter,
                           NeedsControlFrames, NeedsAudio, Upscaler, FolderModel, NotThisEngine, Ltx2 }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub enum NotRead { NeedsToken, NeedsTerms, Budget, Failed }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub enum FileLabel { Works { version: SdVersion, recipe: String, kind: RecipeKind, part: Option<NoisePart>, parameters: u64 },
                     Lora { base: Option<SdVersion> }, Extra { role: GenRole }, CannotRun { reason: CannotRunReason, version: Option<SdVersion> },
                     NotRead { reason: NotRead } }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub enum Verdict { Works { version: SdVersion, kind: RecipeKind }, Lora { base: Option<SdVersion> },
                   CannotRun { reason: CannotRunReason, version: Option<SdVersion> }, Unknown }
pub fn label_for(facts: &FileFacts, name: &str) -> FileLabel;
pub fn verdict_of(labels: &[FileLabel]) -> Verdict;   // the Works label with the most parameters; else a Lora; else the first CannotRun; else Unknown
// sd/candidates.rs
pub const MIN_CANDIDATE_BYTES: u64 = 10_000_000;
pub struct QuantGroup { pub key: String, pub read: String, pub members: Vec<String> }   // `read`: the smallest member
pub struct Candidates { pub groups: Vec<QuantGroup>, pub by_name: BTreeMap<String, FileLabel> }
pub fn candidates(files: &[RepoFile]) -> Candidates;
pub fn group_key(path: &str) -> String;               // the path lower-cased, with quant_label's match removed
pub fn companion_by_name(path: &str) -> Option<SlotId>;
pub fn looks_like_lora(path: &str) -> bool;            // a "lora" token in the path
// gen_search.rs
pub fn verdict_hint(repo_id: &str, pipeline_tag: Option<&str>) -> Verdict;
// hf.rs
pub enum GenSearchKind { Pictures, Video, Loras }
// src-tauri/src/studio/detect.rs
pub const CACHE_ENTRIES: usize = 512;
pub const PARALLEL_READS: usize = 4;
#[derive(Debug, Clone, PartialEq, Eq, Hash)] pub struct CacheKey { pub repo: String, pub revision: String, pub path: String, pub sha256: String }
pub struct RepoReading { pub labels: BTreeMap<String, FileLabel>, pub facts: BTreeMap<String, FileFacts>, pub read: u32, pub unread: u32 }
pub struct DetectService { /* Mutex<{ map: HashMap<CacheKey, FileFacts>, order: VecDeque<CacheKey> }>, verdicts: Mutex<HashMap<String, Verdict>> */ }
impl DetectService {
    pub fn new() -> Self;
    pub async fn read_repo(&self, hf: &HfClient, detail: &RepoDetail) -> RepoReading;
    pub fn verdict(&self, repo: &str) -> Option<Verdict>;   // the last read_repo's verdict for `repo`
}
```
**`candidates`.** Each weight file gets one of:
- **Pickle extensions:** `by_name` `CannotRun { Pickle }`.
- **`*.safetensors.index.json` with shards:** the index and every shard are `CannotRun { Sharded }`, and the first shard is read for the version.
- **No SHA-256:** `CannotRun { NoSha }`.
- **`companion_by_name`:** `Extra { role }`. Plan decision: such files are read only with budget left after every model group, so the budget goes to models first. Read or not, the name label stands unless the header says otherwise.
- **Below `MIN_CANDIDATE_BYTES`**, unless `looks_like_lora`: not listed.
- **Otherwise**, a member of the group of its `group_key`.

`companion_by_name` matches tokens, substrings of the normalised name, and folders. A file whose name also has a model token (`image`, `edit`, `diffusion`, `unet`, `transformer`, `dit`, `checkpoint`, `pruned`, `ema`, `baked`, `merged`, `with`) is never a companion by name.

| Tokens, substrings or folder | Slot |
|---|---|
| `vae`, `ae` (whole tokens), folder `vae/`; only for files under 2 GB (the largest VAE, Wan 2.2's, is 1.4 GB) | `vae` |
| `taesd`, `tae` | `tae` |
| `umt5`, `t5xxl`, `t5`, `byt5` | `t5xxl` |
| `clip-l` | `clip_l` |
| `clip-g` | `clip_g` |
| `clip-vision` | `clip_vision` |
| `mmproj` | `llm_vision` |
| `connector`, `connectors` | `embeddings_connectors` |
| `audio-vae` | `audio_vae` |
| `wav2vec2` | `audio_encoder` |
| substrings `qwen2.5-vl`, `qwen-2.5-vl`, `qwen3-`, `qwen3vl`, `qwen-3-`; tokens `gemma`, `mistral`, `llama`, `text-encoder`, `text-encoders`; folder `text_encoders/` | `llm` |
| `tokenizer*.json` | `tokenizer` |

**`read_repo`:**
1. When the repo is gated and the client has no token, every candidate gets `NotRead { NeedsToken }`, with no request.
2. Otherwise the first `MAX_FILES_PER_REPO` groups are read, `PARALLEL_READS` at a time, each through the cache first.
   - A `NeedsTerms` or `NeedsToken` answer labels that group `NotRead` accordingly.
   - Other errors label it `NotRead { Failed }`.
   - Groups past the budget are `NotRead { Budget }`.
3. Every member of a group gets the label of the file that was read, with `part` from its own name.

**`verdict_hint`.** A token table, first match:
- `lora` → `Lora { base by the version tokens }`;
- `lumina`, `neta` → `CannotRun { Lumina2 }`;
- `ltx-video`, `ltxv` → `CannotRun { LtxVideo09 }`;
- `hidream-i1` → `CannotRun { UnknownWeights }`;
- `svd`, `stable-video` → `CannotRun { Svd }`;
- then the version tokens:

| Tokens | Version |
|---|---|
| `sdxl` | sdxl |
| `sd1.5`, `v1-5`, `sd15` | sd1 |
| `sd2`, `v2-1` | sd2 |
| `sd3`, `sd3.5` | sd3 |
| `flux.1`, `flux1`, `kontext`, `schnell` | flux |
| `flux.2` with `klein` | flux2-klein |
| `flux.2` | flux2 |
| `flux.3` | flux3 |
| `chroma` | chroma |
| `qwen-image` | qwen-image |
| `z-image` | z-image |
| `wan2.1`, `wan2.2`, `wan` | wan2 (`ti2v` → wan2.2-ti2v) |
| `hunyuanvideo-1.5`, `hunyuan-video-1.5` | hunyuan-video |
| `ltx-2.3`, `ltx-2.5` | ltxav |
| `ovis` | ovis-image |
| `longcat` | longcat |
| `pixart` | pixart |
| `anima` | anima |

  The kind comes from the pipeline tag; otherwise `Unknown`.

**`GenSearchKind::Loras`** asks three URLs, with the same `search`, `sort=downloads`, `limit=30` and `expand[]` as now:
- `pipeline_tag=text-to-image&filter=lora`;
- `pipeline_tag=image-to-video&filter=lora`;
- `filter=lora`.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[tokio::test] async fn one_header_read_labels_every_quantisation_of_a_model() {
    let quants = ["Q2_K", "Q3_K_S", "Q3_K_M", "Q4_0", "Q4_1", "Q4_K_S", "Q4_K_M", "Q5_0", "Q5_1", "Q5_K_S", "Q5_K_M", "Q6_K", "Q8_0", "F16", "BF16"];
    let s = hub_server_with(wan_a14b_repo(&quants)).await;        // serves both fixtures' headers for every name
    let reading = DetectService::new().read_repo(&s.client(), &s.detail()).await;
    assert_eq!(s.header_requests(), 2);
    assert_eq!(reading.labels.len(), 30);
    assert!(reading.labels.iter().all(|(path, l)| matches!(l, FileLabel::Works { recipe, part: Some(p), .. }
        if recipe == "wan22-t2v" && (*p == NoisePart::High) == path.starts_with("HighNoise/"))));
    assert_eq!(reading.unread, 0);
}
#[tokio::test] async fn a_gated_repo_without_a_token_labels_files_not_read() {
    let s = hub_server_with(gated(flux_dev_repo())).await;         // client without a token
    let reading = DetectService::new().read_repo(&s.client(), &s.detail()).await;
    assert_eq!(s.requests(), 0);
    assert!(reading.labels.values().all(|l| *l == FileLabel::NotRead { reason: NotRead::NeedsToken } || matches!(l, FileLabel::Extra { .. })));
}
#[tokio::test] async fn a_file_read_once_is_not_read_again() {
    let s = hub_server_with(sdxl_repo()).await; let d = DetectService::new();
    d.read_repo(&s.client(), &s.detail()).await; d.read_repo(&s.client(), &s.detail()).await;
    assert_eq!(s.header_requests(), 1);
}
#[tokio::test] async fn past_24_files_the_rest_are_labelled_not_read() {
    let s = hub_server_with(repo_of_distinct_models(30)).await;
    let r = DetectService::new().read_repo(&s.client(), &s.detail()).await;
    assert_eq!((s.header_requests(), r.read, r.unread), (24, 24, 6));
    assert_eq!(r.labels.values().filter(|l| **l == FileLabel::NotRead { reason: NotRead::Budget }).count(), 6);
}
#[test] fn candidates_name_what_they_skip() {
    let c = candidates(&[f("m.ckpt", 4e9), f("t5xxl_fp8_e4m3fn.safetensors", 4.8e9), f("tiny.safetensors", 1e6), f("lora/style.safetensors", 2e6),
                         f("m-Q4_0.gguf", 7e9), f("m-Q8_0.gguf", 12e9), f_no_sha("x.safetensors", 3e9)]);
    assert_eq!(c.by_name["m.ckpt"], FileLabel::CannotRun { reason: CannotRunReason::Pickle, version: None });
    assert_eq!(c.by_name["t5xxl_fp8_e4m3fn.safetensors"], FileLabel::Extra { role: GenRole::T5xxl });
    assert_eq!(c.by_name["x.safetensors"], FileLabel::CannotRun { reason: CannotRunReason::NoSha, version: None });
    assert_eq!(c.groups.iter().map(|g| g.read.as_str()).collect::<Vec<_>>(), ["lora/style.safetensors", "m-Q4_0.gguf"]);
}
#[test] fn model_files_with_companion_words_are_still_read() {
    let c = candidates(&[f("Qwen_Image-Q4_0.gguf", 11.8e9), f("sd15-baked-vae.safetensors", 4.2e9), f("Qwen3-4B-Q8_0.gguf", 4.3e9)]);
    assert_eq!(c.groups.iter().map(|g| g.read.as_str()).collect::<Vec<_>>(), ["Qwen_Image-Q4_0.gguf", "sd15-baked-vae.safetensors"]);
    assert_eq!(c.by_name["Qwen3-4B-Q8_0.gguf"], FileLabel::Extra { role: GenRole::Llm });
}
#[test] fn lumina_reads_as_cant_run_and_kontext_as_a_picture_edit() {
    assert_eq!(label_for(&inspect(&fixture("lumina2")), "Comfy-Org/Lumina-Image-2/x"), FileLabel::CannotRun { reason: CannotRunReason::Lumina2, version: Some(SdVersion::ZImage) });
    assert!(matches!(label_for(&inspect(&fixture("flux1-kontext")), "QuantStack/FLUX.1-Kontext-dev-GGUF/flux1-kontext-dev-Q4_K_M.gguf"),
        FileLabel::Works { kind: RecipeKind::PictureEdit, .. }));
}
#[test] fn search_hints_from_names() {
    assert_eq!(verdict_hint("nerijs/pixel-art-xl-lora", Some("text-to-image")), Verdict::Lora { base: Some(SdVersion::Sdxl) });
    assert_eq!(verdict_hint("Alpha-VLLM/Lumina-Image-2.0", Some("text-to-image")), Verdict::CannotRun { reason: CannotRunReason::Lumina2, version: None });
    assert_eq!(verdict_hint("QuantStack/Wan2.2-T2V-A14B-GGUF", Some("text-to-video")), Verdict::Works { version: SdVersion::Wan2, kind: RecipeKind::Video });
}
#[test] fn the_lora_search_asks_for_lora_repositories() {     // hf.rs
    let urls = client().generator_search_urls("style", GenSearchKind::Loras);
    assert!(urls.iter().all(|u| u.query().unwrap().contains("filter=lora")));
}
```

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p models --features test-support --lib sd:: gen_search:: hf:: && cargo test -p app --lib studio::detect`
  - Expected: FAIL to compile.

- [ ] **Step 3: Implement** as specified. The LRU is a hand-written `HashMap` plus `VecDeque`. Plan decision: no crate for 512 entries.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p models --features test-support && cargo test -p app --lib studio::`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add crates/models src-tauri/src/studio/detect.rs src-tauri/src/lib.rs
git commit -m "Studio: read a repository's files when it opens (one read per quantisation, at most 24, cached) and label each"
```

### Task S6: The repository view, the set-up plan command and their TypeScript types

**Files:**
- Modify:
  - `crates/models/src/gen_catalog.rs` (`Commercial::WithUseRules`) and `crates/models/src/gen_search.rs` (`KNOWN_LABELS`, `repo_licence`): the OpenRAIL licences (design 11a);
  - `src-tauri/src/dto.rs` (lines 1239–1421);
  - `src-tauri/src/studio/local_cmd.rs`: `repo_dto`, `local_search`, the new `studio_setup_plan`, `parts_dto`, `fit_dto`;
  - `src-tauri/src/studio/local_models.rs`: `LocalSource::Disk`; `entry_from_listing` is replaced by the plan;
  - `src-tauri/src/studio/errors.rs`;
  - command registration: `src-tauri/commands.list`, `capabilities/main.json`, `src-tauri/src/lib.rs`;
  - `src/components/studio/types.ts`, `src/components/studio/api.ts`;
  - `src/components/studio/LocalSearch.tsx` and `localOffers.ts`: kept working, as below;
  - `src/lib/words.ts`.
- Test: inline in `local_cmd.rs`; `src/components/studio/LocalModels.test.tsx` (the search part)

**Interfaces:**
- Consumes: `DetectService::read_repo` and `verdict` (Task S5); `plan_setup` (Task S4); `generic_entry` (Task S3); `GenFit` and `gen_fit`.
- Produces, Rust (`dto.rs`, all `#[serde(rename_all = "camelCase")]`):
```rust
pub struct RecipeChoiceDto { pub id: String, pub name: String, pub kind: RecipeKind }      // name: the recipe's own "name"
pub struct PartDto { pub role: GenRole, pub repo: String, pub path: String, pub size_bytes: u64, pub licence_id: String, pub already_here: bool, pub local: bool }  // `label` removed
pub struct PlannedSlotDto { pub role: GenRole, pub required: bool, pub source: PartSource, pub part: Option<PartDto> }
pub struct SetupPlanDto { pub recipe: RecipeChoiceDto, pub recipes: Vec<RecipeChoiceDto>, pub slots: Vec<PlannedSlotDto>, pub licences: Vec<LicenceDto>,
    pub total_bytes: u64, pub to_fetch_bytes: u64, pub fit: LocalFitDto, pub can_download: bool, pub low_disk: bool, pub needs_engine: bool, pub untested: bool }
pub struct RepoFileDto { pub path: String, pub size_bytes: u64, pub quant: Option<String>, pub label: FileLabel }
pub struct LocalFileChoiceDto { pub file: String, pub quant: String, pub label: FileLabel, pub plan: SetupPlanDto, pub best: bool }
pub struct LocalRepoDto { pub repo: String, pub commit: String, pub author: String, pub summary: Option<String>, pub verdict: Verdict,
    pub files: Vec<LocalFileChoiceDto>, pub loras: Vec<RepoFileDto>, pub file_views: Vec<RepoFileDto>, pub licence: Option<LicenceDto>,
    pub licences: Vec<LicenceDto>, pub label_mismatch: Option<String>, pub gate: GateDto, pub read_files: u32, pub unread_files: u32 }
pub struct LocalSearchHitDto { pub repo: String, pub author: String, pub downloads: u64, pub likes: u64, pub updated_at: Option<String>,
    pub gated: String, pub license_label: Option<String>, pub verdict: Verdict, pub exact: bool }
pub struct SetupChoice { pub recipe: Option<String>, #[serde(default)] pub overrides: Vec<SlotOverride> }      // Deserialize
pub struct SlotOverride { pub role: GenRole, pub file: SlotFile }
#[serde(rename_all = "camelCase")] pub enum SlotFile { Hub { repo: String, commit: String, path: String }, Installed { gen_model_id: String, role: GenRole },
                                                       Disk { path: String }, None }
// local_models.rs
pub enum LocalSource { Catalog { entry_id: String }, Search { repo: String, commit: String, file: String }, Installed { gen_model_id: String }, Disk { path: String } }
// local_cmd.rs
#[tauri::command] pub async fn studio_local_search(window, app, studio, text: String, filter: SearchFilter) -> AppResult<LocalSearchPage>;   // SearchFilter: pictures|video|loras
#[tauri::command] pub async fn studio_setup_plan(window, app, studio, source: LocalSource, choice: SetupChoice) -> AppResult<SetupPlanDto>;
```
- **`files`** holds one entry per `Works` main file. For an A14B pair, that means one per high/low quantisation; the low-noise half is folded into its high-noise sibling's plan.
- They are sorted by total size, largest first.
- **`best`** marks the largest whose fit word is `fits` or `afterUnload`. Plan decision: "fits this computer" means it runs on the device.
- **`summary`** is the model card's first paragraph after the front matter, through `hf::uploader_text(text, 400)`. It is read with the listing, and `None` when that fails.
- **`exact`** on a hit is true when `DetectService::verdict(repo)` exists. The hit's verdict is that one; otherwise `verdict_hint`.
- **Licences of searched files** (design 11a). This task owns licence classification for searched files.
  - `Commercial` gains `WithUseRules` (serde `withUseRules`), whose badge is `Badge::Amber`.
  - `KNOWN_LABELS` gains four labels, each `Commercial::WithUseRules` with plain words "Commercial use allowed, with use rules":
    - `creativeml-openrail-m` "CreativeML Open RAIL-M";
    - `openrail` "Open RAIL";
    - `openrail++` "Open RAIL++-M";
    - `bigscience-openrail-m` "BigScience Open RAIL-M".
  - `pub fn repo_licence(detail: &RepoDetail) -> GenLicence` replaces `searched_licence` for generic models, which have no family upstream: the repo's label, through `KNOWN_LABELS`.
  - Its `url` is the repo's `LICENSE`, `LICENSE.md` or `LICENSE.txt` at the pinned commit when the listing has one, else the repo page at that commit. Plan decision: that is the licence text itself where the publisher ships it.
  - Unknown labels stay red "Check the licence", as in 0.1.x.
  - Companion files keep their table licence.
- Produces, TypeScript (`types.ts`):
```ts
export type SdVersion = "sd1" | "sd1-inpaint" | … ;                       // the 60 words of Task R5, written out
export type PartRole = "model" | "diffusion" | "high-noise-diffusion" | "uncond-diffusion" | "clip-l" | "clip-g" | "t5xxl" | "llm"
  | "llm-vision" | "clip-vision" | "tokenizer" | "vae" | "tae" | "audio-vae" | "audio-encoder" | "embeddings-connectors";
export type RecipeKind = "picture" | "pictureEdit" | "video";
export interface RecipeChoice { id: string; name: string; kind: RecipeKind }
export type CannotRunReason = "lumina2" | "ltxVideo09" | "hunyuanVideo1" | "svd" | "unknownWeights" | "sharded" | "pickle" | "noSha" | "adapter"
  | "needsControlFrames" | "needsAudio" | "upscaler" | "folderModel" | "notThisEngine" | "ltx2";
export type NotReadReason = "needsToken" | "needsTerms" | "budget" | "failed";
export type FileLabel =
  | { works: { version: SdVersion; recipe: string; kind: RecipeKind; part: "high" | "low" | null; parameters: number } }
  | { lora: { base: SdVersion | null } }
  | { extra: { role: PartRole } }
  | { cannotRun: { reason: CannotRunReason; version: SdVersion | null } }
  | { notRead: { reason: NotReadReason } };
export type Verdict = { works: { version: SdVersion; kind: RecipeKind } } | { lora: { base: SdVersion | null } }
  | { cannotRun: { reason: CannotRunReason; version: SdVersion | null } } | "unknown";
export interface Part { role: PartRole; repo: string; path: string; sizeBytes: number; licenceId: string; alreadyHere: boolean; local: boolean }
// Licence.commercial gains "withUseRules" (amber).
export type PartSource = "main" | "sibling" | "installed" | "repo" | "companion" | "chosen" | "embedded" | "missing";
export interface PlannedSlot { role: PartRole; required: boolean; source: PartSource; part: Part | null }
export interface SetupPlan { recipe: RecipeChoice; recipes: RecipeChoice[]; slots: PlannedSlot[]; licences: Licence[]; totalBytes: number;
  toFetchBytes: number; fit: LocalFit; canDownload: boolean; lowDisk: boolean; needsEngine: boolean; untested: boolean }
export interface RepoFileView { path: string; sizeBytes: number; quant: string | null; label: FileLabel }
export interface LocalFileChoice { file: string; quant: string; label: FileLabel; plan: SetupPlan; best: boolean }
export interface LocalRepo { repo: string; commit: string; author: string; summary: string | null; verdict: Verdict; files: LocalFileChoice[];
  loras: RepoFileView[]; fileViews: RepoFileView[]; licence: Licence | null; licences: Licence[]; labelMismatch: string | null; gate: Gate;
  readFiles: number; unreadFiles: number }
export type SearchFilter = "pictures" | "video" | "loras";
export interface LocalSearchHit { repo: string; author: string; downloads: number; likes: number; updatedAt: string | null; gated: GateMode;
  licenseLabel: string | null; verdict: Verdict; exact: boolean }
export type SlotFile = { hub: { repo: string; commit: string; path: string } } | { installed: { genModelId: string; role: PartRole } }
  | { disk: { path: string } } | "none";
export interface SlotOverride { role: PartRole; file: SlotFile }
export interface SetupChoice { recipe: string | null; overrides: SlotOverride[] }
export type LocalSource = { catalog: { entryId: string } } | { search: { repo: string; commit: string; file: string } }
  | { installed: { genModelId: string } } | { disk: { path: string } };
```
- `api.ts`:
  - `studioLocalSearch(text: string, filter: SearchFilter)`;
  - `studioSetupPlan(source: LocalSource, choice: SetupChoice) => invoke<SetupPlan>("studio_setup_plan", { source, choice })`.
- `words.ts` gains:
  - `VERSION_NAME: Record<SdVersion, string>`, for example `"sdxl": "SDXL"`, `"flux": "FLUX.1"`, `"wan2": "Wan 2.1"`, `"wan2.2-ti2v": "Wan 2.2 5B"`, `"ltxav": "LTX-2.3"`;
  - `RECIPE_TYPE_NAME: Record<string, string>` for the recipes whose type differs from their version: `wan22-t2v` and `wan22-i2v` "Wan 2.2", `flux1-kontext` "FLUX.1 Kontext", `qwen-image-edit*` "Qwen-Image Edit", `ltx25-dev` "LTX-2.5";
  - `typeName(version, recipe)`;
  - `KIND_WORD`: "picture", "picture edit", "video";
  - `SLOT_LABEL: Record<PartRole, string>`, for example `"t5xxl": "Text encoder (T5-XXL)"`, `"vae": "Decoder (VAE)"`, `"llm": "Text encoder (language model)"`, `"tokenizer": "Tokenizer"`;
  - `FILE_LABEL`:
    - `works(type, kind, part, parameters)` → "Works in Studio · SDXL picture model", or "Works in Studio · Wan 2.2 video model, 14B, high-noise part";
    - `lora(type)` → "LoRA for FLUX.1", or "LoRA for an unknown model type";
    - `extra(role)` → "Extra file · Decoder (VAE)";
    - `cannotRun(reason, ext)`:
      - `lumina2` → "Can't run in Studio: This is a Lumina 2 model, which the Studio engine can't run";
      - `pickle` → "Can't run in Studio: Studio can't read .ckpt files";
      - one sentence for each other reason;
    - `notRead(reason)`: "Add a Hugging Face token in Settings to read this file", "Accept the model's terms on Hugging Face first", "Not read: Studio reads 24 files per repository", "Couldn't read this file".
- **Kept working until Phase U:**
  - `LocalSearch.tsx` shows the verdict through `FILE_LABEL` in place of `status`/`family`.
  - It lists `fileViews` in place of `standaloneFiles`, each with the existing plain download (`studioLocalDownloadFile`) as "Download file anyway".
  - `FileChoice` shows `SLOT_LABEL[p.role]` in place of `p.label`, and reads its numbers from `choice.plan`.
  - `localOffers.offerForFile` takes a `LocalFileChoice` and its `plan`.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[tokio::test] async fn a_flux_repo_shows_one_choice_per_quantisation_with_the_best_marked() {
    let deps = deps_with(hub(flux_kontext_repo(&["Q4_K_M", "Q8_0", "F16"])), gpu_free_gb(12));
    let r = local_repo(&deps, "QuantStack/FLUX.1-Kontext-dev-GGUF").await.unwrap();
    assert_eq!(r.files.iter().map(|f| f.quant.as_str()).collect::<Vec<_>>(), ["F16", "Q8_0", "Q4_K_M"]);
    assert_eq!(r.files.iter().filter(|f| f.best).map(|f| f.quant.as_str()).collect::<Vec<_>>(), ["Q8_0"]);
    let p = &r.files[1].plan;
    assert_eq!(p.recipe.id, "flux1-kontext"); assert_eq!(p.recipe.kind, RecipeKind::PictureEdit);
    assert_eq!(p.slots.iter().map(|s| s.role).collect::<Vec<_>>(), [GenRole::Diffusion, GenRole::ClipL, GenRole::T5xxl, GenRole::Vae]);
    assert_eq!(p.to_fetch_bytes, p.total_bytes);
    assert!(matches!(r.verdict, Verdict::Works { version: SdVersion::Flux, .. }));
}
#[tokio::test] async fn a_file_studio_cant_run_is_listed_with_its_reason() {
    let r = local_repo(&deps_with(hub(lumina_repo()), gpu_free_gb(12)), "Comfy-Org/Lumina-Image-2").await.unwrap();
    assert!(r.files.is_empty());
    assert!(r.file_views.iter().any(|f| f.label == FileLabel::CannotRun { reason: CannotRunReason::Lumina2, version: Some(SdVersion::ZImage) }));
}
#[tokio::test] async fn the_setup_plan_command_takes_a_recipe_and_an_override() {
    let deps = deps_with(hub(flux_kontext_repo(&["Q4_K_M"])), gpu_free_gb(12));
    let p = setup_plan(&deps, &search_source("flux1-kontext-dev-Q4_K_M.gguf"),
        &SetupChoice { recipe: Some("flux1-dev".into()), overrides: vec![SlotOverride { role: GenRole::T5xxl, file: SlotFile::Hub { repo: "comfyanonymous/flux_text_encoders".into(), commit: FLUX_TE.into(), path: "t5xxl_fp16.safetensors".into() } }] }).await.unwrap();
    assert_eq!(p.recipe.id, "flux1-dev");
    assert!(!p.untested);                                                   // flux1-dev is among the version's recipes
    assert_eq!(p.slots[2].source, PartSource::Chosen);
}
#[test] fn an_openrail_label_reads_amber_with_use_rules_and_links_its_licence() {      // gen_search.rs
    for label in ["creativeml-openrail-m", "openrail", "openrail++", "bigscience-openrail-m"] {
        let l = repo_licence(&detail_labelled(label, &["LICENSE.md", "v1-5-pruned-emaonly.safetensors"]));
        assert_eq!((l.badge(), l.commercial_outputs, l.plain_words.as_str()), (Badge::Amber, Commercial::WithUseRules, "Commercial use allowed, with use rules"), "{label}");
        assert!(l.url.ends_with(&format!("/blob/{COMMIT}/LICENSE.md")), "{label}");
    }
    assert_eq!(repo_licence(&detail_labelled("other", &[])).badge(), Badge::Red);
}
#[test] fn hits_carry_author_update_and_a_verdict() {
    let page = search_page(vec![summary("QuantStack/Wan2.2-T2V-A14B-GGUF", Some("text-to-video"), Some("2026-10-01T00:00:00.000Z"))], &DetectService::new());
    assert_eq!((page.items[0].author.as_str(), page.items[0].exact), ("QuantStack", false));
    assert!(matches!(page.items[0].verdict, Verdict::Works { kind: RecipeKind::Video, .. }));
}
#[test] fn the_new_commands_are_registered() { /* commands_list_matches_capabilities covers studio_setup_plan */ }
```
In `LocalModels.test.tsx`:
- `a search result shows its verdict in words`: a hit with `{ works: { version: "sdxl", kind: "picture" } }` shows "Works in Studio · SDXL picture model".
- `a file Studio can't run offers the plain download`: "Download file anyway" calls `studioLocalDownloadFile`.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p app --lib studio::local_cmd && npx vitest run src/components/studio/LocalModels.test.tsx`
  - Expected: FAIL.

- [ ] **Step 3: Implement.** Register `studio_setup_plan` the four ways (Global Constraints). Regenerate the permission file with `cargo build -p app`, as for 0.1.x commands.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p app && npm run typecheck && npm test`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add src-tauri src/components/studio src/lib/words.ts
git commit -m "Studio: a repository shows every file's label, one choice per quantisation with its set-up plan, and the best for this computer"
```

### Task S7: Download and register a generic model

**Files:**
- Modify:
  - `src-tauri/src/studio/local_models.rs`: `start_local_download` takes the choice; `plan_download` takes a `SetupPlan`;
  - `src-tauri/src/downloads.rs`: `GenGroup` and `GenGroupPart` additions, `gen_model_from`, `register_gen_model`;
  - `src-tauri/src/studio/local_cmd.rs`: `studio_local_download(source, choice)`;
  - `src-tauri/src/studio/errors.rs`;
  - `src/components/studio/api.ts`, `src/components/studio/localOffers.ts`, `src/components/studio/DownloadLocalModel.tsx` (pass the choice through).
- Test: inline in `local_models.rs` and `downloads.rs`, with the module's existing fake deps and Hub server.

**Interfaces:**
- Consumes: `setup_plan` (Task S6); `plan_setup` (Task S4); `DetectService` (Task S5); `GenFamily::recipe_id` (Task S1).
- Produces:
```rust
pub struct GenGroup { /* as now */ #[serde(default)] pub recipe_id: Option<String>, #[serde(default)] pub untested: bool }
pub struct GenGroupPart { /* as now */ #[serde(default, skip_serializing_if = "Option::is_none")] pub present: Option<String> }   // already on disk: no row
pub async fn start_local_download(app: &AppHandle, source: LocalSource, choice: Option<SetupChoice>) -> AppResult<String>;
#[tauri::command] pub async fn studio_local_download(window, app, source: LocalSource, choice: Option<SetupChoice>) -> AppResult<String>;
// errors.rs
pub fn setup_incomplete(missing: &[GenRole]) -> AppError;   // code "studio_setup_incomplete": "Choose a file for each part first." / "Open the model's files and pick the missing parts."
```
- `api.ts`: `studioLocalDownload(source: LocalSource, choice: SetupChoice | null)`.
- **`start_local_download` for `Search`:**
  1. It refuses until the content notice is accepted (as now).
  2. It lists the repo at `commit`, reads it through `DetectService` (cached from opening), plans with `choice`, and refuses `setup_incomplete` when `!plan.can_run()`.
  3. It refuses with the Hub's own `NeedsToken`/`NeedsTerms` error when any planned Hub file is in a gated repo and the client has no token. This happens **before** any group or row is created.
  4. It refuses `low_disk` as now.
  5. It creates one group whose parts are exactly the plan's parts: Hub files to fetch, plus `present` for installed and local ones.
  6. It downloads each Hub file to `<models>/<owner>/<repo>/<path>`, or `@<commit12>` for another revision, as `plan_download` does now.
- **`register_gen_model`** writes:
  - id `hf:<owner>/<repo>:<file>`;
  - `recipe_id` from the plan;
  - `family` = the version word, or `unknown` when untested;
  - `kind` from `kind_of`;
  - `source` `search`;
  - one part per planned slot with its `GenRole`, and `file` = the row's destination or the `present` path.
- `Catalog` and `Installed` sources work as in 0.1.2, with `recipe_id` from the family.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[tokio::test] async fn a_kontext_file_downloads_its_listed_companions_and_registers_its_recipe() {
    let t = harness(hub(flux_kontext_repo(&["Q4_K_M"]))).await;
    let group = start_local_download(&t.app, search_source("flux1-kontext-dev-Q4_K_M.gguf"), None).await.unwrap();
    assert_eq!(t.group_files(&group), ["flux1-kontext-dev-Q4_K_M.gguf", "clip_l.safetensors", "t5xxl_fp8_e4m3fn.safetensors", "ae.safetensors"]);
    t.complete(&group).await;
    let row = t.store.get_gen_model("hf:QuantStack/FLUX.1-Kontext-dev-GGUF:flux1-kontext-dev-Q4_K_M.gguf").await.unwrap().unwrap();
    assert_eq!((row.recipe_id.as_deref(), row.family.as_str(), row.kind.as_str()), (Some("flux1-kontext"), "flux", "image-edit"));
}
#[tokio::test] async fn nothing_downloads_that_the_plan_does_not_list() {
    let t = harness(hub(flux_kontext_repo(&["Q4_K_M"]))).await;
    let plan = t.setup_plan(search_source("flux1-kontext-dev-Q4_K_M.gguf"), SetupChoice::default()).await;
    let group = start_local_download(&t.app, search_source("flux1-kontext-dev-Q4_K_M.gguf"), None).await.unwrap();
    assert_eq!(t.group_bytes(&group), plan.to_fetch_bytes);
}
#[tokio::test] async fn a_set_up_with_a_missing_required_file_is_refused() {
    let t = harness(hub(bare_hunyuan_gguf_repo())).await;
    let e = start_local_download(&t.app, search_source("h-Q4_0.gguf"), None).await.unwrap_err();
    assert_eq!(e.code, "studio_setup_incomplete"); assert!(t.groups().is_empty());
}
#[tokio::test] async fn a_gated_download_without_a_token_is_refused_before_anything_starts() {
    let t = harness_without_token(hub(gated(flux_dev_repo()))).await;
    let e = start_local_download(&t.app, search_source("flux1-dev.safetensors"), None).await.unwrap_err();
    assert_eq!(e.code, "hf_needs_token");                      // the existing code of hub::hf_error(HfError::NeedsToken)
    assert!(t.groups().is_empty() && t.download_rows().is_empty() && t.files_on_disk().is_empty());
}
#[tokio::test] async fn a_wan_a14b_download_fetches_both_parts() {
    let t = harness(hub(wan_a14b_repo(&["Q4_K_M"]))).await;
    let g = start_local_download(&t.app, search_source("HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf"), None).await.unwrap();
    assert!(t.group_files(&g).contains(&"LowNoise/Wan2.2-T2V-A14B-LowNoise-Q4_K_M.gguf".to_string()));
}
#[tokio::test] async fn deleting_one_model_keeps_a_companion_a_generic_model_reuses() {
    let t = harness(hub(chroma_repo())).await;
    t.install_catalog("z-image-turbo-q4").await;              // owns <models>/Comfy-Org/Z-Image-Turbo/split_files/vae/ae.safetensors
    let g = start_local_download(&t.app, search_source("Chroma1-HD-Q4_0.gguf"), None).await.unwrap();
    assert!(!t.group_files(&g).iter().any(|f| f.ends_with("ae.safetensors")));   // reused by SHA-256: present, not fetched
    t.complete(&g).await;
    let ae = t.models_dir.join("Comfy-Org/Z-Image-Turbo/split_files/vae/ae.safetensors");
    delete_local_model(&t.app, "z-image-turbo-q4").await.unwrap();
    assert!(ae.exists());
    delete_local_model(&t.app, "hf:silveroxides/Chroma1-HD-GGUF:Chroma1-HD-Q4_0.gguf").await.unwrap();
    assert!(!ae.exists());
}
```

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p app --lib studio::local_models downloads::`
  - Expected: FAIL to compile.

- [ ] **Step 3: Implement** as specified. `delete_unless_in_use` and `remove_unused_files` stay as they are. Their `hub::gen_files_used` sees the generic row's `present` file, because it is a part's `file`.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p app --lib && npm run typecheck && npm test`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add src-tauri src/components/studio
git commit -m "Studio: one Download fetches a model and the files its plan lists, and registers it with its recipe"
```

### Task S8: Add a model file from the computer

**Files:**
- Create: `src-tauri/src/studio/import.rs`
- Modify:
  - `src-tauri/src/studio/prefs.rs` (`last_import_mode`);
  - `src-tauri/src/studio/local_cmd.rs` (commands);
  - `src-tauri/src/downloads.rs`: a group can carry a `present` main part;
  - `src-tauri/src/studio/errors.rs`;
  - command registration;
  - `src/components/studio/types.ts`, `src/components/studio/api.ts`.
- Test: inline in `import.rs`

**Interfaces:**
- Consumes: `inspect_local` (Task R6); `label_for` (Task S5); `plan_setup` (Task S4); `start_local_download` group machinery (Task S7).
- Produces:
```rust
pub const IMPORT_PROGRESS_EVENT: &str = "studio-import-progress";
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)] #[serde(rename_all = "camelCase")] pub enum ImportMode { Copy, InPlace }
#[derive(Serialize)] #[serde(rename_all = "camelCase")] pub enum ImportStage { Hashing, Copying }
#[derive(Serialize)] #[serde(rename_all = "camelCase")] pub struct ImportProgress { pub path: String, pub stage: ImportStage, pub done_bytes: u64, pub total_bytes: u64 }
#[derive(Serialize)] #[serde(rename_all = "camelCase")] pub struct ImportInspectionDto { pub path: String, pub name: String, pub size_bytes: u64,
    pub label: FileLabel, pub plan: Option<SetupPlanDto>, pub last_mode: ImportMode, pub inside_models: bool }
#[derive(Serialize)] #[serde(rename_all = "camelCase")] pub struct ImportResultDto { pub gen_model_id: Option<String>, pub download_group: Option<String> }
pub async fn inspect_import(deps: &LocalCmdDeps, path: &Path) -> AppResult<ImportInspectionDto>;            // header only, no hashing
pub async fn import_model(app: &AppHandle, path: &Path, mode: ImportMode, choice: Option<SetupChoice>) -> AppResult<ImportResultDto>;
#[tauri::command] pub async fn studio_import_inspect(window, app, studio, path: String) -> AppResult<ImportInspectionDto>;
#[tauri::command] pub async fn studio_import_model(window, app, path: String, mode: ImportMode, choice: Option<SetupChoice>) -> AppResult<ImportResultDto>;
// prefs.rs
pub struct StudioPrefs { /* … */ #[serde(default)] pub last_import_mode: Option<ImportMode> }
```
- **The path is untrusted input from the webview:**
  - canonicalised;
  - a regular file, not a symlink to a directory;
  - its extension in `READ_EXTENSIONS`. A pickle extension is refused with copy "Studio can't read .ckpt files" (the actual extension).
- **The mode:**
  - A file already inside the models folder is used in place, without asking (`inside_models: true`).
  - Otherwise `InPlace` keeps the file where it is.
  - `Copy` writes `<models>/@imported/<sha256[..16]>/<file name>` through a `.partial` file, then renames. An existing destination with the same SHA-256 is reused.
  - Moving is never done.
  - `last_import_mode` is saved with each import. Plan decision: the person is offered the choice once per import, with the last choice preselected.
- **Hashing:**
  - The file is hashed with SHA-256 while it is read, or copied.
  - An `ImportProgress` event goes to the main window at most every 250 ms, plus a final one with `done_bytes == total_bytes`.
- **The row:**
  - id `local:<sha256[..16]>`, source `import`;
  - the main part is `GenPart { repo: "", revision: "", path: <file name>, size, sha256, license: "", local: Some(path) }`;
  - companions come from the plan. When some must be fetched, one group starts with the main part `present`, and the row is registered when it completes (as in Task S7). Otherwise the row is registered at once.
- A LoRA file is refused here with code `studio_import_is_lora`, so the webview routes it to `studio_lora_import`.
- `types.ts` gains `ImportMode`, `ImportInspection`, `ImportResult` and `ImportProgress` (the fields above, camelCase).
- `api.ts` gains:
  - `studioImportInspect(path)`;
  - `studioImportModel(path, mode, choice)`;
  - `onImportProgress(cb)` through `listen("studio-import-progress")`.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[tokio::test] async fn a_copied_import_lands_in_the_imported_folder_and_registers_with_its_sha() {
    let t = harness().await; let src = t.outside_file("ü dir/my sd15.safetensors", fixture_bytes("sd15"));
    let r = import_model(&t.app, &src, ImportMode::Copy, None).await.unwrap();
    let row = t.store.get_gen_model(r.gen_model_id.as_deref().unwrap()).await.unwrap().unwrap();
    let sha = sha256_file(&src);
    assert_eq!(row.id, format!("local:{}", &sha[..16]));
    assert_eq!((row.recipe_id.as_deref(), row.source), (Some("sd1-checkpoint"), GenSource::Import));
    assert!(t.models_dir.join(format!("@imported/{}/my sd15.safetensors", &sha[..16])).exists());
    assert!(src.exists());                                                     // copied, never moved
}
#[tokio::test] async fn an_in_place_import_keeps_the_file_where_it_is_and_never_deletes_it() {
    let t = harness().await; let src = t.outside_file("m.safetensors", fixture_bytes("sd15"));
    let id = import_model(&t.app, &src, ImportMode::InPlace, None).await.unwrap().gen_model_id.unwrap();
    delete_local_model(&t.app, &id).await.unwrap();
    assert!(src.exists());
}
#[tokio::test] async fn a_file_inside_the_models_folder_is_used_in_place_without_asking() {
    let t = harness().await; let f = t.models_file("@files/abc/m.safetensors", fixture_bytes("sd15"));
    assert!(inspect_import(&t.deps(), &f).await.unwrap().inside_models);
}
#[tokio::test] async fn a_pickle_file_is_refused_with_the_extension_named() {
    let t = harness().await; let e = inspect_import(&t.deps(), &t.outside_file("m.ckpt", vec![0x80, 2])).await.unwrap_err();
    assert!(e.message.contains(".ckpt"));
}
#[tokio::test] async fn an_import_that_needs_companions_starts_one_download_and_registers_on_completion() {
    let t = harness_with_hub().await; let src = t.outside_file("chroma.gguf", fixture_bytes("chroma"));
    let r = import_model(&t.app, &src, ImportMode::InPlace, None).await.unwrap();
    assert!(r.gen_model_id.is_none());
    t.complete(r.download_group.as_deref().unwrap()).await;
    assert_eq!(t.store.list_gen_models().await.unwrap()[0].recipe_id.as_deref(), Some("chroma"));
}
#[tokio::test] async fn progress_is_sent_while_hashing_and_ends_complete() {
    let t = harness().await; let src = t.outside_file("m.safetensors", fixture_bytes("sd15"));
    import_model(&t.app, &src, ImportMode::Copy, None).await.unwrap();
    let last = t.events::<ImportProgress>(IMPORT_PROGRESS_EVENT).last().cloned().unwrap();
    assert_eq!(last.done_bytes, last.total_bytes);
}
```
`fixture_bytes(name)` writes a file whose header is the fixture's tensor list, with zero-length data. Detection reads only the header.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p app --lib studio::import`
  - Expected: FAIL to compile.

- [ ] **Step 3: Implement**, and register both commands the four ways.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p app --lib && npm run typecheck`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add src-tauri src/components/studio
git commit -m "Studio: add a model file from the computer, copied or used where it is, set up like a download"
```

### Task S9: Set it up yourself

**Files:**
- Create: `src-tauri/src/studio/setup_cmd.rs` (`studio_setup_plan` moves here from `local_cmd.rs`)
- Modify:
  - `src-tauri/src/dto.rs`: `InstalledLocalDto` additions, `ModelFilesDto`;
  - `src-tauri/src/studio/local_cmd.rs`: installed rows without a recipe are listed;
  - command registration;
  - `src/components/studio/types.ts`, `src/components/studio/api.ts`.
- Test: inline in `setup_cmd.rs`

**Interfaces:**
- Consumes: Tasks S3, S4, S6 and S7.
- Produces:
```rust
#[serde(rename_all = "camelCase")] pub struct ModelFilesDto { pub gen_model_id: String, pub recipe: Option<RecipeChoiceDto>, pub recipes: Vec<RecipeChoiceDto>,
    pub slots: Vec<PlannedSlotDto>, pub untested: bool, pub can_run: bool }
#[serde(rename_all = "camelCase")] pub struct SetModelFilesDto { pub files: ModelFilesDto, pub download_group: Option<String> }
pub struct InstalledLocalDto { /* as now */ pub recipe: Option<RecipeChoiceDto>, pub version: Option<SdVersion>, pub untested: bool, pub needs_setup: bool,
    pub high_noise: bool, pub takes_loras: bool, pub lora_base: Option<SdVersion>, pub source: String, pub in_place: bool }
#[tauri::command] pub async fn studio_model_files(window, app, gen_model_id: String) -> AppResult<ModelFilesDto>;
#[tauri::command] pub async fn studio_set_model_files(window, app, studio, gen_model_id: String, choice: SetupChoice) -> AppResult<SetModelFilesDto>;
```
- **`recipes`:** `recipes_for(version)` for a detected version. For an untested or unplaced model, every in-scope recipe, sorted by kind and then name.
- **`untested`** is true when the chosen recipe is not in `recipes_for(version)`, or the version is unknown.
- **`needs_setup`** is `recipe_id IS NULL`.
- **`high_noise`:** the recipe has `feature("high_noise")`.
- **`takes_loras`:** `feature("lora")`.
- **`lora_base`:** `version.lora_family()`.
- **`studio_set_model_files`:**
  - It refuses with `studio_model_in_use` (existing, inventory S16) while a run uses the model.
  - It plans with the model's own files as `present` and the overrides on top.
  - When a Hub file must be fetched, it starts a group whose completion replaces the row (same id, `INSERT OR REPLACE`). Otherwise it rewrites the row at once.
  - A file the old set-up used and nothing uses now is removed by `remove_unused_files` (inside the models folder only).
- **Set it up yourself from a repo.** `studio_setup_plan` with an explicit `recipe` is accepted for a file labelled `CannotRun { UnknownWeights }` or `NotRead { Budget | Failed }`, and gives `untested: true`.
  - Plan decision: never for pickle files, sharded files or the named can't-run models. The engine is known to refuse or misrun those.
- Untested runs are marked in the run's settings line by the webview, from `InstalledLocal.untested` ("untested set-up").
- `types.ts`: `ModelFiles`, `SetModelFilesResult`, and the `InstalledLocal` fields above.
- `api.ts`: `studioModelFiles(genModelId)` and `studioSetModelFiles(genModelId, choice)`.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[tokio::test] async fn files_show_each_slot_of_the_recipe_with_its_file() {
    let t = harness_with(installed_generic_flux()).await;
    let f = model_files(&t.deps(), "hf:o/r:f.gguf").await.unwrap();
    assert_eq!(f.slots.iter().map(|s| (s.role, s.source)).collect::<Vec<_>>(), [(GenRole::Diffusion, PartSource::Main), (GenRole::ClipL, PartSource::Installed),
        (GenRole::T5xxl, PartSource::Installed), (GenRole::Vae, PartSource::Installed)]);
    assert!(f.can_run && !f.untested);
}
#[tokio::test] async fn changing_the_vae_to_a_disk_file_keeps_the_model_and_marks_it_local() {
    let t = harness_with(installed_generic_flux()).await; let ae = t.outside_file("my ae.safetensors", fixture_bytes("ae-flux1"));
    let r = set_model_files(&t.app, "hf:o/r:f.gguf", choice_override(GenRole::Vae, SlotFile::Disk { path: ae.display().to_string() })).await.unwrap();
    assert!(r.download_group.is_none());
    assert!(r.files.slots.iter().any(|s| s.role == GenRole::Vae && s.part.as_ref().unwrap().local));
}
#[tokio::test] async fn choosing_a_recipe_the_detection_did_not_pick_marks_the_setup_untested() {
    let t = harness_with(installed_generic_flux()).await;
    let r = set_model_files(&t.app, "hf:o/r:f.gguf", SetupChoice { recipe: Some("z-image".into()), overrides: vec![] }).await.unwrap();
    assert!(r.files.untested);
}
#[tokio::test] async fn an_unknown_file_can_be_set_up_by_hand_and_is_untested() {
    let t = harness(hub(unknown_weights_repo())).await;
    let p = t.setup_plan(search_source("mystery.safetensors"), SetupChoice { recipe: Some("sdxl-checkpoint".into()), overrides: vec![] }).await;
    assert!(p.untested && p.can_download);
    assert!(t.setup_plan_err(search_source("lumina_2.safetensors"), SetupChoice { recipe: Some("z-image".into()), overrides: vec![] }).await.code == "studio_cannot_run");
}
#[tokio::test] async fn a_model_in_use_is_not_changed() {
    let t = harness_with(installed_generic_flux()).await; t.hold_run_on("hf:o/r:f.gguf");
    assert_eq!(set_model_files(&t.app, "hf:o/r:f.gguf", SetupChoice::default()).await.unwrap_err().code, "studio_model_in_use");
}
#[tokio::test] async fn a_row_without_a_recipe_is_listed_as_needing_set_up() {
    let t = harness_with(row_without_recipe()).await;
    assert!(t.local_models().await.installed[0].needs_setup);
}
```

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p app --lib studio::setup_cmd`
  - Expected: FAIL to compile.

- [ ] **Step 3: Implement**, and register both commands.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p app --lib && npm run typecheck`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add src-tauri src/components/studio
git commit -m "Studio: Set it up yourself: change a model's type and any of its files, with an untested set-up note"
```

### Task S10: LoRAs: the store, download, import and delete

**Files:**
- Create: `src-tauri/src/studio/loras.rs`
- Modify:
  - `src-tauri/src/dto.rs` (`LoraDto`, `LocalModelsDto.loras`);
  - `src-tauri/src/downloads.rs` (`GroupMeta.lora`; registration);
  - `src-tauri/src/hub.rs` (`gen_files_used` includes LoRA files);
  - `src-tauri/src/studio/manager/lane.rs`: the LoRA files of the running job are in use;
  - `src-tauri/src/studio/errors.rs`;
  - command registration;
  - `src/components/studio/types.ts`, `src/components/studio/api.ts`, `src/lib/types.ts`.
- Test: inline in `loras.rs`

**Interfaces:**
- Consumes: `store::gen_loras` (Task S1); `inspect`/`inspect_local` (Task R6); `DetectService` (Task S5).
- Produces:
```rust
pub const LORA_FOLDER: &str = "@loras";
pub fn lora_folder(models_dir: &Path) -> PathBuf;
pub fn sanitize_lora_component(s: &str) -> String;   // drops ':' '<' '>' '|' '\\' and control characters; trims spaces and dots at both ends; "" → "lora"
pub fn rel_name_for_download(repo: &str, path: &str) -> String;                       // "<owner>/<repo>/<path without extension>", each component sanitised
pub fn rel_name_for_import(file_name: &str, taken: &BTreeSet<String>) -> String;     // "local/<stem>", " (2)", " (3)" … while taken
pub fn stored_extension(path: &str) -> &'static str;                                 // ".safetensors" for .safetensors and .sft (the engine tries
                                                                                     // .gguf, .safetensors, .pt, .ckpt only), ".gguf" for .gguf
#[serde(rename_all = "camelCase")] pub struct LoraDto { pub id: String, pub name: String, pub base: Option<SdVersion>, pub format: LoraFormat,
    pub size_bytes: u64, pub source: String, pub repo: Option<String>, pub missing: bool }
#[tauri::command] pub async fn studio_loras(window, app) -> AppResult<Vec<LoraDto>>;
#[tauri::command] pub async fn studio_lora_download(window, app, repo: String, commit: String, file: String) -> AppResult<String>;   // group id
#[tauri::command] pub async fn studio_lora_import(window, app, path: String) -> AppResult<LoraDto>;
#[tauri::command] pub async fn studio_lora_delete(window, app, studio, id: String) -> AppResult<()>;
```
- **The name** is the file stem, as written, with spaces and Unicode kept.
- **The file:**
  - A LoRA's file is `<models>/@loras/<rel_name><stored_extension>`.
  - Downloads are verified by SHA-256, and imports are copied and hashed with `studio-import-progress` events.
  - Plan decision: imports always copy, because the engine's LoRA names must be relative to one folder.
- **The checks:**
  - A file whose header is not `FileKind::Lora` is refused (`studio_not_a_lora`: "This file isn't a LoRA.").
  - A delete is refused while a run uses it (`studio_lora_in_use`: "A run on this computer is using this LoRA." / "Stop the run, then delete it.").
- **`missing`** is true when the file is gone.
- **The id:** `hf:<owner>/<repo>:<path>` for downloads, and `local:<sha256>` for imports. Importing the same file twice gives the same row.
- `DownloadGroupDto` gains `lora_id: Option<String>`, like its `gen_model_id`, so a finished LoRA download can be offered for use.
- `types.ts` gains `Lora`, `LocalModels.loras: Lora[]` and `DownloadGroup.loraId?: string | null` (`src/lib/types.ts`). `api.ts` gains `studioLoras`, `studioLoraDownload`, `studioLoraImport` and `studioLoraDelete`.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[test] fn lora_names_keep_spaces_and_unicode_and_drop_colons_and_angles() {
    let none = BTreeSet::new();
    assert_eq!(rel_name_for_import("pixel art ü.safetensors", &none), "local/pixel art ü");
    assert_eq!(rel_name_for_import("日本語 スタイル.safetensors", &none), "local/日本語 スタイル");
    assert_eq!(rel_name_for_import("a:b<c>d|e.safetensors", &none), "local/abcde");
    assert_eq!(rel_name_for_import(" . .safetensors", &none), "local/lora");
    assert_eq!(rel_name_for_download("XLabs-AI/flux-lora-collection", "realism_lora_comfy_converted.safetensors"), "XLabs-AI/flux-lora-collection/realism_lora_comfy_converted");
    for n in [rel_name_for_import("a:b<c>d|e.safetensors", &none), rel_name_for_download("o/r", "dir/x y.sft")] { assert!(runtime::sd::args::valid_lora_name(&n), "{n}"); }
}
#[test] fn two_imports_with_the_same_name_get_distinct_names() {
    assert_eq!(rel_name_for_import("style.safetensors", &BTreeSet::from(["local/style".to_string()])), "local/style (2)");
}
#[tokio::test] async fn an_imported_lora_is_copied_into_the_lora_folder_and_registered_with_its_base() {
    let t = harness().await; let src = t.outside_file("Ü dir/pixel art xl.safetensors", fixture_bytes("sdxl-lora-pixel-art"));
    let l = import_lora(&t.app, &src).await.unwrap();
    assert_eq!((l.name.as_str(), l.base, l.format), ("pixel art xl", Some(SdVersion::Sdxl), LoraFormat::Kohya));
    assert!(t.models_dir.join("@loras/local/pixel art xl.safetensors").exists() && src.exists());
}
#[tokio::test] async fn a_sft_lora_is_saved_as_safetensors_so_the_engine_finds_it() {
    let t = harness().await; import_lora(&t.app, &t.outside_file("s.sft", fixture_bytes("flux-lora-realism"))).await.unwrap();
    assert!(t.models_dir.join("@loras/local/s.safetensors").exists());
}
#[tokio::test] async fn a_lora_download_is_refused_for_a_file_that_is_not_a_lora() {
    let t = harness(hub(sdxl_repo())).await;
    assert_eq!(download_lora(&t.app, "stabilityai/stable-diffusion-xl-base-1.0", SDXL_REV, "sd_xl_base_1.0.safetensors").await.unwrap_err().code, "studio_not_a_lora");
}
#[tokio::test] async fn deleting_a_lora_in_use_is_refused_and_otherwise_removes_its_file() {
    let t = harness().await; let l = import_lora(&t.app, &t.outside_file("s.safetensors", fixture_bytes("flux-lora-realism"))).await.unwrap();
    t.hold_run_with_lora(&l.id);
    assert_eq!(delete_lora(&t.app, &l.id).await.unwrap_err().code, "studio_lora_in_use");
    t.end_run(); delete_lora(&t.app, &l.id).await.unwrap();
    assert!(!t.models_dir.join("@loras/local/s.safetensors").exists());
}
```

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p app --lib studio::loras`
  - Expected: FAIL to compile.

- [ ] **Step 3: Implement**, and register the four commands.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p app --lib && npm run typecheck`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add src-tauri src/components/studio
git commit -m "Studio: LoRAs from Hugging Face or the computer, kept in one folder under names the engine can read"
```

### Task S11: Local runs from recipes, with fit as a warning

**Files:**
- Modify:
  - `src-tauri/src/studio/local.rs`: `params_for`, `job_for`, `frames_for`, `size_of`, `check_files`;
  - `src-tauri/src/studio/manager.rs`: `job_options` passes the `MemoryChoice` instead of deciding `memory_saving`;
  - `src-tauri/src/studio/manager/lane.rs`: remove the `NotHere` refusals at 416–422 and 789–793;
  - `crates/models/src/gen_fit.rs`: video judged by memory; `video_on_processor`;
  - `crates/agent/src/media/spec.rs` (`ParamKind::Loras` and its validation);
  - `src-tauri/src/dto.rs`: `LocalFitDto.video_on_processor`; `LocalModelsDto.video_possible` is replaced by `video_on_processor`;
  - `src-tauri/src/studio/local_cmd.rs`;
  - `src-tauri/src/studio/errors.rs`: remove the unused "does not fit" refusal copy at 1100–1109; add `studio_lora_missing`;
  - `src/components/studio/types.ts`.
- Test: inline in `local.rs`, `manager.rs`, `gen_fit.rs` and `spec.rs`

**Interfaces:**
- Consumes: `SdJob` (Task S2), `GenEntry` (Task S3), `store::gen_loras` (Task S1), `lora_folder` (Task S10).
- Produces:
```rust
// agent/src/media/spec.rs
pub enum ParamKind { /* … */ Loras { max: u32, high_noise: bool, weight_min: f64, weight_max: f64, weight_step: f64, weight_default: f64 } }
// the value is [{"id": string, "weight": number, "highNoise": bool}], at most `max`; weight within [min, max]; highNoise only when `high_noise`
pub enum RefRole { Reference, StartFrame, EndFrame, Source, Mask, Control }   // gains Source, Mask, Control (serde "source", "mask", "control"; design 11a)
// local.rs
pub mod key { /* … */ pub const GUIDANCE: &str = "guidance"; pub const IMG_CFG: &str = "img_cfg_scale"; pub const HIGH_NOISE_STEPS: &str = "high_noise_steps";
              pub const HIGH_NOISE_CFG: &str = "high_noise_cfg_scale"; pub const LORAS: &str = "loras";
              pub const SOURCE: &str = "source_picture"; pub const MASK: &str = "mask_picture"; pub const CONTROL: &str = "control_picture"; }
pub const LORA_WEIGHT: (f64, f64, f64, f64) = (-1.0, 2.0, 0.05, 1.0);   // min, max, step, default
pub struct LocalJobOptions { pub processor_only: bool, pub memory: MemoryChoice, pub threads: Option<u32>, pub device_description: Option<String> }
pub fn job_for(entry: &GenEntry, model: &GenModel, req: &MediaRequest, opts: &LocalJobOptions, loras: &[GenLora], lora_dir: &Path,
               backend: Option<&str>, out_dir: &Path, run_id: &str) -> AppResult<SdJob>;
pub fn memory_flags(entry: &GenEntry, recipe: Option<&Recipe>, width: u32, height: u32, video: bool, memory: MemoryChoice) -> (bool, bool);  // (vae_tiling, offload_to_cpu)
// gen_fit.rs
pub struct GenFitResult { /* … */ pub video_on_processor: bool }
```
- **`params_for`, for every entry:**
  - the sampler enum lists `Sampler::ALL` and the scheduler enum `Scheduler::ALL`, defaulting to the entry's;
  - sizes are offered as multiples of `limits.multiple_of`;
  - frames follow `limits.video_rule()`;
  - `fps` is fixed (no field) when `video_rule().fixed_fps` is set;
  - `GUIDANCE` (range 0.5–10, step 0.5) when the recipe has `feature("distilled_guidance")`;
  - `IMG_CFG` (range 1–20, step 0.5) for `feature("img_cfg")`;
  - `HIGH_NOISE_STEPS` (range −1–50; −1 reads "Automatic") and `HIGH_NOISE_CFG` (`GUIDANCE_RANGE`), both under Advanced, for `feature("high_noise")`;
  - `LORAS` with `max` `MAX_LORAS` and `high_noise` from the recipe, for `feature("lora")`;
  - one `ParamKind::Images { min, max, role }` per `entry.input_counts` row (Task S3), so inpainting asks for a picture to change and a mask, Canny and Depth for a control picture, and Flex.2 offers all three.
  - Catalog entries keep their tested defaults and limits.
  - The guidance and negative prompt are hidden when the default cfg is 1, the 0.1.x rule (inventory S46).
- **`job_for`:**
  - each part becomes its `PartRole`;
  - the recipe's `custom_sigmas`, `shifted_timestep`, `strength`, `moe_boundary` and `context` (`vae_format`, `model_args`) are passed as given;
  - `guidance`, `img_cfg` and `high_noise` come from params over defaults;
  - `end_image` comes from the `endFrame` reference;
  - `init_image` comes from `source` (or `startFrame` for video), `mask_image` from `mask`, and `control_image` from `control`;
  - a mask without a picture to change is refused with `studio_mask_needs_picture` ("Add the picture to change before its mask." / "Add it, or remove the mask.");
  - with a picture to change and no size chosen, the size follows the picture: its aspect, at the recipe default's pixel count, rounded to the multiple.
  - `size_multiple` comes from the limits.
  - **LoRAs:** each attached id is looked up in `gen_loras`. A missing row or file is refused with `studio_lora_missing` ("A LoRA's file is missing: <name>." / "Add it again, or remove it from the settings."). Otherwise `lora_dir = lora_folder(models_dir)` and `LoraTag { name: rel_name, weight, high_noise }`. With no LoRAs attached, `lora_dir` is `None`.
- **`memory_flags`:**
  - `On`: (true, true).
  - `Off`: (false, false).
  - `Automatic`: tiling when `video || width * height > 1024 * 1024`, unless the recipe disables `vae_tiling`; never offload.
  - Spec correction 4: Automatic no longer adds `--offload-to-cpu` on a tight fit, because the engine's auto-fit does that.
  - The out-of-memory retry (Task S2) still turns both on.
- **`gen_fit`:**
  - `NotFor::VideoNeedsGpu` and `NotFor::VideoNotOnThisMac` are removed. A video on `Target::Cpu` is judged by memory like a picture, and gets `video_on_processor: true`.
  - On Apple silicon, a video whose catalog entry says `runs_on_apple: false` is judged by memory too.
  - The fit word, reason and suggestion stay; they now only warn.
- `LocalModelsDto.video_on_processor` is true when this computer runs models on the processor. `types.ts`: `LocalFit.videoOnProcessor: boolean`; `LocalModels.videoOnProcessor` replaces `videoPossible`; `RefRole` gains `"source" | "mask" | "control"`; `ParamKind` gains `{ type: "loras"; max: number; highNoise: boolean; weightMin: number; weightMax: number; weightStep: number; weightDefault: number }`; and `export interface LoraUse { id: string; weight: number; highNoise: boolean }` is the value of the `loras` param.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[test] fn an_01x_klein_picture_keeps_its_0_1_2_arguments() {
    // the klein catalog entry, 1024×1024, 4 steps, seed 7, memory automatic, green fit, Vulkan0
    let job = job_for(&catalog_entry("flux2-klein-4b-q4"), &klein_row(), &klein_request(), &opts(MemoryChoice::Automatic), &[], LORAS, Some("Vulkan0"), OUT, "r1").unwrap();
    assert_eq!(build_args(&job).unwrap(), os(&KLEIN_0_1_2_ARGS));
}
#[test] fn a_generic_sdxl_picture_runs_with_m_and_the_recipe_defaults() {
    let a = strings(build_args(&job_for(&generic("sdxl-checkpoint", SdVersion::Sdxl), &sdxl_row(), &req(json!({})), &opts(MemoryChoice::Automatic), &[], LORAS, None, OUT, "r").unwrap()).unwrap());
    assert_eq!((after(&a, "-m").ends_with("sd_xl_base_1.0.safetensors"), after(&a, "--steps"), after(&a, "--cfg-scale"), after(&a, "-W")), (true, "20", "5", "1024"));
    assert!(!a.contains(&"--diffusion-model".into()) && !a.contains(&"--vae-tiling".into()));
}
#[test] fn video_and_big_pictures_tile_the_decoder_unless_the_recipe_says_no() {
    assert_eq!(memory_flags(&e("wan22-ti2v-5b"), Some(r("wan22-ti2v-5b")), 832, 480, true, MemoryChoice::Automatic), (true, false));
    assert_eq!(memory_flags(&e("sdxl-checkpoint"), Some(r("sdxl-checkpoint")), 1024, 1024, false, MemoryChoice::Automatic), (false, false));
    assert_eq!(memory_flags(&e("sdxl-checkpoint"), Some(r("sdxl-checkpoint")), 1088, 1024, false, MemoryChoice::Automatic), (true, false));
    assert_eq!(memory_flags(&e("z-image-l2p"), Some(r("z-image-l2p")), 2048, 2048, false, MemoryChoice::Automatic), (false, false));
    assert_eq!(memory_flags(&e("z-image-l2p"), Some(r("z-image-l2p")), 512, 512, false, MemoryChoice::On), (true, true));
}
#[test] fn attached_loras_reach_the_job_and_a_missing_one_is_refused() {
    let l = lora_row("local/pixel art ü", existing_file());
    let job = job_for(&generic("sdxl-checkpoint", SdVersion::Sdxl), &sdxl_row(), &req(json!({"loras": [{"id": l.id, "weight": 0.8, "highNoise": false}]})),
                      &opts(MemoryChoice::Automatic), &[l.clone()], LORAS, None, OUT, "r").unwrap();
    assert_eq!(job.loras, [LoraTag { name: "local/pixel art ü".into(), weight: 0.8, high_noise: false }]);
    assert_eq!(job.lora_dir.as_deref(), Some(LORAS));
    let gone = lora_row("local/x", PathBuf::from("/nope/x.safetensors"));
    assert_eq!(job_for(&generic("sdxl-checkpoint", SdVersion::Sdxl), &sdxl_row(), &req(json!({"loras": [{"id": gone.id, "weight": 1.0, "highNoise": false}]})),
                       &opts(MemoryChoice::Automatic), &[gone], LORAS, None, OUT, "r").unwrap_err().code, "studio_lora_missing");
}
#[test] fn a_mask_and_a_control_picture_reach_the_job() {
    let inpaint = generic("sdxl-inpainting", SdVersion::SdxlInpaint);
    assert!(matches!(spec(&params_for(&inpaint), key::MASK).kind, ParamKind::Images { min: 1, max: 1, role: RefRole::Mask }));
    let job = job_for(&inpaint, &row(), &req_with_refs(&[(RefRole::Source, "in.png"), (RefRole::Mask, "mask.png")]), &opts(MemoryChoice::Automatic), &[], LORAS, None, OUT, "r").unwrap();
    assert!(job.init_image.unwrap().ends_with("in.png") && job.mask_image.unwrap().ends_with("mask.png"));
    let flex = generic("flex2", SdVersion::Flex2);
    assert_eq!(job_for(&flex, &row(), &req_with_refs(&[(RefRole::Mask, "mask.png")]), &opts(MemoryChoice::Automatic), &[], LORAS, None, OUT, "r").unwrap_err().code, "studio_mask_needs_picture");
    let canny = job_for(&generic("flux1-canny", SdVersion::FluxControls), &row(), &req_with_refs(&[(RefRole::Control, "edges.png")]), &opts(MemoryChoice::Automatic), &[], LORAS, None, OUT, "r").unwrap();
    assert!(canny.control_image.unwrap().ends_with("edges.png"));
}
#[test] fn a_wan_a14b_model_offers_high_noise_settings_and_high_noise_loras() {
    let p = params_for(&generic("wan22-t2v", SdVersion::Wan2));
    assert!(matches!(spec(&p, key::LORAS).kind, ParamKind::Loras { max: 8, high_noise: true, weight_min, weight_max, weight_step, weight_default }
        if (weight_min, weight_max, weight_step, weight_default) == (-1.0, 2.0, 0.05, 1.0)));
    assert!(has(&p, key::HIGH_NOISE_STEPS) && has(&p, key::HIGH_NOISE_CFG));
}
#[test] fn the_loras_value_is_validated() {       // spec.rs
    let k = ParamKind::Loras { max: 8, high_noise: false, weight_min: -1.0, weight_max: 2.0, weight_step: 0.05, weight_default: 1.0 };
    assert!(validate_value(&k, &json!([{"id": "a", "weight": 2.5, "highNoise": false}])).is_err());
    assert!(validate_value(&k, &json!([{"id": "a", "weight": 1.0, "highNoise": true}])).is_err());
    assert!(validate_value(&k, &json!((0..9).map(|i| json!({"id": i.to_string(), "weight": 1.0, "highNoise": false})).collect::<Vec<_>>())).is_err());
}
#[test] fn video_on_the_processor_is_judged_and_flagged_not_blocked() {      // gen_fit.rs
    let r = gen_fit(&wan_entry(), GenSize::video(832, 480, 49), &Target::Cpu { free_bytes: 64 << 30 }, None);
    assert!(r.video_on_processor && r.not_for.is_none() && r.fit != GenFit::Red);
}
#[tokio::test] async fn a_model_that_does_not_fit_still_runs() {          // manager.rs
    let t = lane_harness(red_fit()).await;
    assert!(t.start_local(klein_request()).await.is_ok());
}
```
`an_01x_klein_picture_keeps_its_0_1_2_arguments` and `KLEIN_0_1_2_ARGS` were recorded from the unchanged 0.1.2 code in Task S2. Here they must stay green while `job_for` changes; only the call changes to the new signature, never the vector.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p app --lib studio:: && cargo test -p models --lib gen_fit:: && cargo test -p agent --lib media::spec`
  - Expected: FAIL to compile.

- [ ] **Step 3: Implement.** The lane's prompt check and every other refusal stay.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test --workspace && npm run typecheck && npm test`
  - Expected: PASS. Update the `LocalModels.test.tsx` cases that read `videoPossible`.

- [ ] **Step 5: Commit.**
```bash
git add src-tauri crates/models/src/gen_fit.rs crates/agent/src/media/spec.rs src/components/studio/types.ts src/components/studio/LocalModels.test.tsx
git commit -m "Studio: run any model from its recipe, with LoRAs; a model that may not fit is a warning, never a refusal"
```

### Task S12: The webview's store actions, words and fixture

**Files:**
- Modify:
  - `src/store/studioLocal.ts`, `src/store/studio.ts` (draft LoRA actions, the settings line);
  - `src/lib/words.ts`;
  - `src/components/studio/ParamField.tsx`: `loras` renders nothing until Phase U's LoRA section; no crash on the new kind;
  - `src/components/studio/RunCard.tsx`: the settings line;
  - `src-tauri/src/studio/commands.rs`: the webview fixture gains a generic model, an untested one, one needing set-up, and a LoRA;
  - `src/components/studio/fixtures/models.ts`: regenerated.
- Create: `src/store/studioLocal.test.ts`
- Test: `src/store/studioLocal.test.ts`, `src/store/studio.test.ts`

**Interfaces:**
- Consumes: every `api.ts` wrapper from Tasks S6–S11.
- Produces (`StudioLocalState` additions; what Phase U's screens call):
```ts
search(text: string, filter: SearchFilter): Promise<void>;               // replaces search(text, tab); "" lists the most-downloaded (Popular)
localSearch: { text: string; filter: SearchFilter; hits: LocalSearchHit[] } | null;
openRepo(repo: string): Promise<LocalRepo>;                              // also refreshes the hit's verdict (exact) in localSearch
repos: Record<string, LocalRepo>;                                        // repositories opened this session: the Fits switch and Use it read them
setupPlan(source: LocalSource, choice: SetupChoice): Promise<SetupPlan>;
downloadSetup(source: LocalSource, choice: SetupChoice | null): Promise<string>;   // group id; refreshes local models and downloads
chooseFile(kind: "model" | "lora"): Promise<string | null>;              // @tauri-apps/plugin-dialog open(), filters .safetensors .sft .gguf
importInspect(path: string): Promise<ImportInspection>;
importModel(path: string, mode: ImportMode, choice: SetupChoice | null): Promise<ImportResult>;
importProgress: ImportProgress | null;                                   // set from studio-import-progress, cleared when done
modelFiles(genModelId: string): Promise<ModelFiles>;
setModelFiles(genModelId: string, choice: SetupChoice): Promise<SetModelFilesResult>;
loras: Lora[];
loadLoras(): Promise<void>;
downloadLora(repo: string, commit: string, file: string): Promise<string>;
importLora(path: string): Promise<Lora>;
deleteLora(id: string): Promise<void>;
// studio.ts (draft)
attachLora(id: string): void;                       // weight 1, highNoise false; at most the param's max
setLoraWeight(id: string, weight: number): void;    // clamped to [weightMin, weightMax], rounded to weightStep
setLoraHighNoise(id: string, on: boolean): void;
detachLora(id: string): void;
lorasFor(model: InstalledLocal): { fits: Lora[]; others: Lora[] };   // base === model.loraBase first; others marked "may not fit this model"
```
- **`words.ts`:**
  - `LORAS`: "LoRAs", "Add", "Remove", "Strength", "High-noise part", "may not fit this model", "No LoRAs yet";
  - `IMPORT`: "Add a model file", "Add a LoRA file", "Copy it into Studio's models folder", "Use it where it is", "Reading the file…", "Copying…";
  - `SETUP`: "Model type", "Files", "Change", "untested set-up", "Set it up yourself", "Download file anyway";
  - the run settings line's LoRA part: "LoRAs: pixel art ü 0.8, style 1";
  - `LOCAL_WARNING.mayNotFit(reason)` = "May not fit this computer: <reason>. It may fail or be very slow." and `LOCAL_WARNING.videoOnProcessor` = "Video on the processor is very slow". They are defined here because Phase U's model sheet, composer and cards use them; Task G2 removes the old refusal wording;
  - `INPUT_ROLE`: "Picture to change", "Mask", "Control picture" (beside the existing reference, start-frame and end-frame words), with the hints "A black-and-white picture: white areas are changed, black areas are kept." and "An edge map for Canny models, a depth map for Depth models.".
  - Every `SdVersion`, `PartRole`, `CannotRunReason` and `NotReadReason` has wording, enforced by `Record<…, string>` types.

**Steps**

- [ ] **Step 1: Write the failing tests** (vitest, with `api.ts` mocked as `studio.test.ts` does):
  - `search with a filter keeps only the last answer`: two searches resolve out of order, and `localSearch.filter === "loras"` and hits are the second's.
  - `opening a repo makes its hit's verdict exact`.
  - `downloadSetup passes the choice and refreshes models and downloads`.
  - `import progress events update importProgress and clear at the end`.
  - `attachLora adds weight 1 and setLoraWeight clamps to -1..2 in steps of 0.05`: `setLoraWeight("a", 2.37)` gives 2, and `setLoraWeight("a", 0.83)` gives 0.85.
  - `lorasFor lists LoRAs for the model's base first`.
  - `the settings line names attached LoRAs`.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `npx vitest run src/store/studioLocal.test.ts src/store/studio.test.ts`
  - Expected: FAIL.

- [ ] **Step 3: Implement**, and regenerate the fixture.
  - Run: `STUDIO_WRITE_FIXTURE=1 cargo test -p app --lib the_webview_fixture_is_current`

- [ ] **Step 4: Run them and see them pass.**
  - Run: `npm run lint && npm run typecheck && npm test && cargo test -p app --lib the_webview_fixture_is_current`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add src/store src/lib/words.ts src/components/studio src-tauri/src/studio/commands.rs
git commit -m "Studio: store actions and words for getting, importing and setting up models, and for LoRAs"
```

## Phase U: Studio's look, switching and getting models (section 10)

Inputs:
- design 10;
- the 2026-11-24 screenshot critique (session scratch, `studio-before/critique.md`), whose numbered defects and code causes are cited as "critique §n";
- the "before" screenshots;
- the preview harness.

The rules for every Phase U task:
- **Stay inside the app's stack:**
  - Tailwind 4 tokens in `src/styles.css` (`bg`, `bg-subtle`, `bg-muted`, `surface`, `fg`, `fg-muted`, `border`, `border-strong`, `accent`, `accent-2`, `warning`, `success`, `danger`, `focus`; `shadow-soft|float|pop`);
  - Radix through the `radix-ui` package, which already exports Select, Switch, Checkbox, Slider, Tooltip, ToggleGroup, Popover, Dialog and DropdownMenu;
  - `lucide-react` icons and `clsx`;
  - no shadcn and no new package (design 10.1).
- **Screenshots.** A task's last step takes the "after" screenshots of the screens it touches, with the harness from Task U1, in both themes. It keeps the file names of the "before" set so pairs line up, and adds a step to `e2e/preview/shoot.mjs` for each new screen.
- **Wording** goes in `words.ts`, under the group each task names. Every control has an accessible name; dialogs, sheets and panels trap focus and close on Esc, with focus returning to their opener.

### Task U1: The preview harness in the repo

**Files:**
- Create, from the scratch harness (`studio-preview-harness/`):
  - `e2e/preview/README.md`, `index.html`, `main.tsx`, `vite.config.ts`;
  - `mock/ipc.ts`, `mock/data.ts`, `mock/pictures.ts`;
  - `shoot.mjs` (was `shoot.cjs`) and a new `pairs.mjs`.
- Modify:
  - `eslint.config.js`: a block `{ files: ["e2e/preview/**/*.mjs"], languageOptions: { globals: globals.node } }`;
  - `package.json` scripts only: `"preview:serve": "vite --config e2e/preview/vite.config.ts"`, `"preview:shoot": "node e2e/preview/shoot.mjs"`, `"preview:pairs": "node e2e/preview/pairs.mjs"`;
  - `e2e/README.md`: one line on the harness.
- Test: the lint, typecheck and build gates, and a harness run

**Interfaces:**
- **Vite.** `vite.config.ts` has `root = new URL("../..", import.meta.url)`, the page `/e2e/preview/index.html` and port 5199 (`PREVIEW_PORT`).
- **Out of the app build.** `tsconfig.json` (`include: ["src", "vite.config.ts"]`) and the app's `index.html` don't reach it, and vitest finds no test files in it.
- **Playwright** is used as now: a global install, or `PLAYWRIGHT_PATH`. It is not a dependency.
- **The mock** answers every Phase S command in its `case` list:
  - `studio_local_search` (with `filter`), `studio_local_repo` (the new `LocalRepo`) and `studio_setup_plan`;
  - `studio_import_inspect`, `studio_import_model`, `studio_model_files` and `studio_set_model_files`;
  - `studio_loras`, `studio_lora_download`, `studio_lora_import` and `studio_lora_delete`;
  - `plugin:dialog|open`, which returns `/home/me/Downloads/my model.safetensors`.
  - Its records add a generic SDXL model, an untested model, a model needing set-up, an SDXL inpainting model, two LoRAs, an OpenRAIL repo, a Lumina 2 repo and a Wan 2.2 A14B repo with 15 quantisations.
  - The records are built from `src/components/studio/fixtures/models.ts` (Task S12), so they follow the DTOs.
- **`shoot.mjs [--only ids] [--themes light,dark] [--scale 2] [--out .cache/preview/after] [--strict]`:**
  - It takes the 26 screens of the before set: a, b, b2, c0, c, c2, d, d2, d3, e0, e, e2, f, f2, g, g2, h, i, i2, j, k, z1, z2, x1, x2 and x3.
  - After each screen it records:
    - commands the mock did not answer;
    - **sideways scroll** (`document.scrollingElement.scrollWidth > innerWidth`);
    - **body share**: the share of visible text characters set at 14 px or more.
  - It prints a table of these. With `--strict`, it exits 1 on an unanswered command, any sideways scroll, or a body share under 0.5 on a Studio screen (all but i, i2 and x1–x3). Design 10.4: "at least half of each screen's text is at body size"; 10.4: "the 900×650 narrow window must not scroll sideways".
- **`pairs.mjs <before> <after> [--out .cache/preview/pairs.html]`** writes one page with each screen's before and after side by side, for both themes. It is for the owner's review (design 10.5).
- **The before set.**
  - At the start of Phase U, the task takes it with `--out .cache/preview/before`. Plan decision: that is 0.1.2's look with Phase S's data, so before and after differ only by Phase U.
  - The 2026-11-24 capture stays in the session scratch as a reference.
  - PNGs are never committed; `.cache/` is ignored.

**Steps**

- [ ] **Step 1: Move the harness and adapt the mock and the checks** as above.

- [ ] **Step 2: Run the gates and see them pass.**
  - Run: `npm run lint && npm run typecheck && npm run build && test ! -e dist/e2e`
  - Expected: PASS.

- [ ] **Step 3: Take the before set.**
  - Run: `npm run preview:serve &` then `npm run preview:shoot -- --out .cache/preview/before`
  - Expected: 52 PNGs, no unanswered command. The table shows the current sideways scroll on `j` and the body shares (critique §5: about 30–40%).

- [ ] **Step 4: Commit.**
```bash
git add e2e/preview e2e/README.md eslint.config.js package.json
git commit -m "Studio: the preview harness for before/after screenshots, in e2e/preview"
```

### Task U2: Shared controls, one focus ring and the type scale

**Files:**
- Create in `src/components/ui/`:
  - `styles.ts`;
  - `SelectField.tsx`, `MenuButton.tsx`, `SwitchField.tsx`, `CheckboxField.tsx`, `SliderField.tsx`;
  - `Tip.tsx`, `ChipGroup.tsx`, `SlideOver.tsx`;
  - `ui.test.tsx`.
- Modify:
  - `src/styles.css`:
    - the global `:focus-visible` rule (134–137) moves into `@layer base`, so utilities can override it (critique §6: it beats Tailwind's `outline-none`);
    - the type-scale utilities;
    - `.studio-page`;
  - `src/App.tsx`: one `Tooltip.Provider` at the root;
  - `src/components/studio/styles.ts`: `CARD`, `ROW`, `CHIP` and `MENU_ITEM` move here, and the duplicate copies in `PromptDock`, `EmptyStates`, `RunCard`, `ModelPicker` and `ParamField` go (critique §5);
  - `src/lib/words.ts` (`UI`).
- Test: `src/components/ui/ui.test.tsx`

**Interfaces:**
- Produces:
```ts
// src/components/ui/styles.ts
export const FOCUS_RING = "outline-none focus-visible:ring-2 focus-visible:ring-accent/40";   // the one focus ring (design 10.4)
// type scale (@utility in styles.css): text-view-title = text-xl font-semibold; text-title = text-lg font-semibold;
// text-section = text-sm font-semibold; text-body = text-sm; text-meta = text-xs text-fg-muted
// .studio-page: background var(--bg-subtle) in light, var(--bg) in dark (design 10.4: clearer page/card separation in light, within the tokens)
export function SelectField<T extends string>(p: { label: string; hideLabel?: boolean; value: T | null; placeholder?: string;
  options: { value: T; label: string; hint?: string }[]; onChange(v: T): void; disabled?: boolean }): JSX.Element;      // Radix Select
export function MenuButton(p: { label: string; trigger: ReactNode; align?: "start" | "end"; children: ReactNode }): JSX.Element;   // Radix DropdownMenu
export function MenuItem(p: { onSelect(): void; disabled?: boolean; children: ReactNode }): JSX.Element;
export function MenuRadio<T extends string>(p: { value: T; options: { value: T; label: string }[]; onChange(v: T): void }): JSX.Element;
export function SwitchField(p: { label: string; description?: string; checked: boolean; onChange(v: boolean): void; disabled?: boolean }): JSX.Element;
export function CheckboxField(p: { label: string; checked: boolean; onChange(v: boolean): void }): JSX.Element;
export function SliderField(p: { label: string; min: number; max: number; step: number; value: number; onChange(v: number): void;
  format?(v: number): string }): JSX.Element;                                  // Radix Slider, with the value as text beside it
export function Tip(p: { content: string; side?: "top" | "right" | "bottom" | "left"; children: ReactElement }): JSX.Element;   // Radix Tooltip
export function ChipGroup<T extends string>(p: { label: string; value: T; options: { value: T; label: string }[]; onChange(v: T): void }): JSX.Element;  // ToggleGroup, single
export function SlideOver(p: { open: boolean; onOpenChange(open: boolean): void; title: string; width: "panel" | "half";
  modal?: boolean; children: ReactNode }): JSX.Element;                       // Radix Dialog from the right; modal: true by default
```
- Words: `UI.close` = "Close".

**Steps**

- [ ] **Step 1: Write the failing tests** (`ui.test.tsx`, Testing Library with `fireEvent`):
  - `SelectField is a combobox named by its label, and choosing an option calls onChange`;
  - `SwitchField has role switch with aria-checked, and toggles on click and Space`;
  - `SliderField moves by its step on arrow keys and shows its value as text`: min −1, max 2, step 0.05, value 1, ArrowRight gives 1.05;
  - `Tip shows its content on keyboard focus`;
  - `ChipGroup is a group of pressed buttons, one at a time`;
  - `MenuButton opens a menu, and its radio items report the choice`;
  - `SlideOver traps focus, closes on Esc and returns focus to its opener`.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `npx vitest run src/components/ui`
  - Expected: FAIL.

- [ ] **Step 3: Implement**, with every control using `FOCUS_RING`.

- [ ] **Step 4: Run them and see them pass, and take the screenshots.**
  - Run: `npm run lint && npm run typecheck && npm test`. Expected: PASS.
  - Then: `npm run preview:shoot -- --only a,b,d --out .cache/preview/after`. Expected: the prompt and search boxes no longer show the doubled 2 px outline.

- [ ] **Step 5: Commit.**
```bash
git add src/components/ui src/styles.css src/App.tsx src/components/studio src/lib/words.ts
git commit -m "UI: styled Radix select, menu, switch, checkbox, slider, tooltip and slide-over; one focus ring; a type scale"
```

### Task U3: The composer and the main layout

**Files:**
- Create in `src/components/studio/`:
  - `Composer.tsx` (`Composer`);
  - `ModelChip.tsx` (`ModelChip`);
  - `ComposerMenus.tsx` (`ShapeMenu`, `CountMenu`);
  - `InputZones.tsx` (`InputZones`, `InputZone`);
  - `Composer.test.tsx`.
- Modify:
  - `StudioView.tsx`: the results take the page; the composer sits at the bottom of the results column; the side panel is closed by default;
  - `TopBar.tsx`;
  - `RefTray.tsx`: `RefThumb` and the role logic serve the composer's strip; `FrameDrop` is replaced by `InputZone`;
  - `SettingsPanel.tsx`: shown only when open; its content is unchanged until Task U8;
  - `src/store/studio.ts` and `src/components/studio/types.ts`;
  - `src-tauri/src/studio/prefs.rs` (`settings_open: bool`, serde default false);
  - `src/lib/words.ts`.
- Delete: `PromptDock.tsx` and `PromptDock.test.tsx`. Its behaviours move into `Composer.test.tsx` deliberately: the refusal banner, the price and time line, Create's enabled states and the prompt check.
- Test: `Composer.test.tsx`; `StudioView.test.tsx` (updated); `prefs.rs` (one test)

**Interfaces:**
- Consumes:
  - draft state: `drafts[tab]` (prompt, values, refs), `setPrompt`, `setValue`, `addRef`, `removeRef`, `setRefRole`;
  - the existing estimate and create actions; `prefs`;
  - `local.installed: InstalledLocal[]` (Task S9), `MediaModel.params` (`ParamKind::Images` with roles `source`, `mask`, `control`, `startFrame`, `endFrame`; Task S11);
  - `INPUT_ROLE` (Task S12).
- Produces:
```ts
// store/studio.ts
settingsOpen: boolean;                                                     // from prefs.settingsOpen at load
settingsSection: "setups" | "advanced" | "loras" | "files" | null;
setSettingsOpen(open: boolean, section?: "setups" | "advanced" | "loras" | "files"): void;   // saves prefs.settingsOpen
// StudioPrefs (types.ts) gains settingsOpen: boolean
export function Composer(p: { tab: StudioTab }): JSX.Element;
export function ModelChip(p: { tab: StudioTab; onOpen(): void }): JSX.Element;   // opens the 0.1.x picker until Task U4
export function InputZones(p: { tab: StudioTab; model: MediaModel }): JSX.Element | null;
```
- **The composer** is a `<section aria-label>` ("Make a picture" / "Make a video") with the existing `.composer` card style:
  - Row 1: reference thumbnails with a Remove button each, *Add picture*, the input zones, then the prompt (`textarea` labelled "Prompt", placeholder "Describe the picture you want…").
  - Row 2 is a `role="toolbar"`: `ModelChip`, `ShapeMenu`, `CountMenu`, *Settings*, then on the right the time or price and Create.
  - Under it, one `text-meta` privacy line. The chip's dot carries the same line in a `Tip`.
- **`InputZone`** is one per `Images` param whose role is `source`, `mask`, `control`, `startFrame` or `endFrame`:
  - a labelled drop target and button, "Add the picture to change" for example;
  - the thumbnail and Remove when set;
  - its hint in a `Tip`;
  - "required" when its min is 1.
- **Create** is disabled only for: no model, an empty prompt, a required input picture missing, an invalid setting, or the prompt check. Never for fit (Task S11). The words say why.
- **The top bar** (design 10.1):
  - the title "Studio" at `text-title`;
  - `KindTabs`;
  - Search, Starred and Grid;
  - **Get models**, a button with the `Download` icon, which calls `showLocalModels(true)`.
  - The `PlaceMark` chip goes (critique §6: it looked clickable).
  - The spend for today shows only while `prefs.dailyLimitUsd !== null`; Task G1 extends this to every spending switch.
  - The ⋯ menu keeps *Update model lists* and loses *Get local models*.
- **The side panel** opens from *Settings*. It stays docked on windows at least 1400 px wide while `settingsOpen`, and is a `SlideOver` (`width: "panel"`) below that.
- Words:
  - `STUDIO_COMPOSER`: `region(kind)`, `prompt` "Prompt", `placeholder(kind)`, `toolbar` "Picture settings", `shape` "Shape", `count` "How many", `settings` "Settings", `addPicture` "Add picture", `remove(name)`, `required` "required", `maskNeedsSource` "Add the picture to change before its mask.";
  - `STUDIO_TOPBAR.getModels` "Get models".

**Steps**

- [ ] **Step 1: Write the failing tests.**
  - `Composer.test.tsx`:
    - `the composer is one region with the prompt, the model chip and Create`;
    - `the model chip names the model and where it runs, and its dot's tooltip is the privacy line`;
    - `shape and count are menus in the toolbar`: "Shape: 1:1" opens a menu of radio items, and choosing "16:9" sets `aspect_ratio`;
    - `Create shows the time for a local model and the price for a cloud one`;
    - `added pictures show as thumbnails before the prompt, each with a Remove button named after it`;
    - `an inpainting model shows Picture to change and Mask zones, and Create stays off until both are added`;
    - `Flex.2's zones are optional, and a mask without a picture to change says so`;
    - `Settings opens the side panel and moves focus into it`;
    - the refusal-banner, price-line and prompt-check cases ported from `PromptDock.test.tsx`.
  - `StudioView.test.tsx`:
    - `the results take the page and the side panel is closed by default`;
    - `the panel stays docked on a 1400 px window when it was left open`;
    - `Get models is a top-bar button and the place chip is gone`;
    - `today's spend shows only while a daily limit is on`.
  - `prefs.rs`: `settings_open_defaults_to_false_and_round_trips`.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `npx vitest run src/components/studio && cargo test -p app --lib studio::prefs`
  - Expected: FAIL.

- [ ] **Step 3: Implement.**

- [ ] **Step 4: Run them and see them pass, and take the screenshots.**
  - Run: `npm run lint && npm run typecheck && npm test && cargo test -p app --lib studio::prefs`. Expected: PASS.
  - Then: `npm run preview:shoot -- --only a,b2,c0,j,k,z1,o --out .cache/preview/after`, with a new screen `o-inpainting-inputs`: the SDXL inpainting model chosen, both zones visible. Expected: one composer at the bottom, and no sideways scroll on `j`.

- [ ] **Step 5: Commit.**
```bash
git add src src-tauri/src/studio/prefs.rs e2e/preview
git commit -m "Studio: one composer with the model chip, shape, count, input pictures, Settings and Create; the results take the page"
```

### Task U4: The model sheet, and *Add a model file*

**Files:**
- Create in `src/components/studio/`:
  - `ModelSheet.tsx` (`ModelSheet`, `SheetRow`, `RecentTiles`, `SheetFooter`);
  - `modelSheet.ts` (`sheetSections`, plus `pickerGroups`, `priceHint` and `localHint` moved unchanged from `ModelPicker.tsx`);
  - `ImportDialog.tsx` (`ImportDialog`).
- Modify:
  - `ModelChip.tsx` (opens the sheet);
  - `StudioView.tsx` (mounts `ImportDialog`);
  - `src/store/studio.ts`: `sheetOpen` and `openSheet` replace `pickerOpen` and `openPicker`, and every caller moves; `importOpen` and `openImport`;
  - `src/lib/words.ts`.
- Delete: `ModelPicker.tsx`. `ModelPicker.test.tsx` becomes `ModelSheet.test.tsx`: its `pickerGroups`, `priceHint` and `localHint` cases are kept as they are, and its UI cases are replaced.
- Test: `ModelSheet.test.tsx`, `ImportDialog.test.tsx`

**Interfaces:**
- Consumes:
  - `local.installed: InstalledLocal[]` with `recipe`, `version`, `sizeBytes`, `fit`, `untested`, `needsSetup` and `takesLoras` (Task S9);
  - `typeName(version, recipe)`, `KIND_WORD`, `LOCAL_WARNING.mayNotFit` (Tasks S6 and S12);
  - the draft's `values.loras` (its length is the LoRA count);
  - `recentRuns`, `endpoints`, `chooseModel(tab, target, id)`, `showLocalModels(true)`, `setSettingsOpen(true, "files")`;
  - for importing: `chooseFile("model")`, `importInspect`, `importModel`, `importLora` and `importProgress` (Task S12), and `ImportInspection.lastMode` and `insideModels` (Task S8).
- Produces:
```ts
export function sheetSections(s: Pick<StudioState, "local" | "endpoints" | "recentRuns" | "drafts">, tab: StudioTab, query: string,
  filter: "all" | "local" | "cloud"): { recent: SheetModel[]; local: SheetModel[]; cloud: { company: string; rows: SheetModel[] }[] };   // recent ≤ 4, never repeated below
export function ModelSheet(p: { tab: StudioTab; anchor: RefObject<HTMLElement> }): JSX.Element;
export function ImportDialog(): JSX.Element | null;                   // driven by importOpen
// store/studio.ts
sheetOpen: boolean; openSheet(open: boolean): void;
importOpen: "model" | "lora" | null; openImport(kind: "model" | "lora" | null): void;
```
- **The sheet** is a Radix Popover anchored to the chip, about 640×560 (`data-layout="popover"`). On windows narrower than 1000 px it is a full-height `SlideOver` (`data-layout="sheet"`).
  - Plan decision: 1000 px is where 0.1.2's panel already narrows.
  - **At the top:** a search field, focused on open, and a `ChipGroup` with All · On this computer · Cloud. The kind follows the tab.
  - **Rows:**
    - local: name; type label `${typeName} · ${KIND_WORD}` ("SDXL · picture"); size; a fit dot whose `Tip` is the warning; the LoRA count;
    - "Needs setting up" and "untested set-up" where true;
    - cloud: name and price or "price unknown"; at most two capability icons, each with an `aria-label` ("Can change a picture", "Transparent background"); model notes in a `Tip` on an info icon.
  - **Unusable endpoints** (no key, no list) are rows with their fix as a link (design 10.2).
  - **The footer** has *Get more models* (primary) and *Add a model file*.
  - **Keys:** arrows move, Enter picks and closes, Esc closes; focus returns to the chip.
- **`ImportDialog`** steps through:
  1. choose the file;
  2. read it: its label in words (`FILE_LABEL`);
  3. *Copy it into Studio's models folder* or *Use it where it is* (a radio group), preselected from `lastMode`, and skipped when `insideModels`;
  4. the plan's extra files and the total to download, if any;
  5. progress (`importProgress`), then done, which selects the model.
  - A LoRA file is added with `importLora`. A pickle file shows "Studio can't read .ckpt files".
- Words:
  - `MODEL_SHEET`: `title` "Choose a model", `search` "Search models", `all`, `local` "On this computer", `cloud` "Cloud", `recent` "Recent", `priceUnknown` "price unknown", `canEdit`, `transparent`, `notes` "About this model", `getMore` "Get more models", `addFile` "Add a model file", `loraCount(n)`, `needsSetup` "Needs setting up", `untested` "untested set-up";
  - `IMPORT_DIALOG`: `title`, `reading` "Reading the file…", `copy`, `inPlace`, `alsoDownloads(size)`, `add` "Add", `done` "Added".

**Steps**

- [ ] **Step 1: Write the failing tests.**
  - `ModelSheet.test.tsx`:
    - `the sheet opens from the chip with its search focused, and Esc closes it with focus back on the chip`;
    - `arrow keys move through rows and Enter picks the model and closes the sheet`;
    - `recent models are at most four tiles and are not repeated below`;
    - `an installed row says its type, size and fit, with the warning in its tooltip`: "SDXL · picture", "6.9 GB", and "May not fit this computer: …";
    - `a model with LoRAs attached shows the count`;
    - `cloud rows show a price or "price unknown" and at most two capability icons with accessible names`;
    - `a model's notes are in a tooltip reachable by keyboard`;
    - `the filter narrows the rows, and the kind follows the tab`;
    - `an endpoint without a key is a row with its fix as a link`;
    - `a model that needs setting up says so, and picking it opens Files`;
    - `on a window narrower than 1000 px the sheet is full height`;
    - `Get more models opens Get models, and Add a model file opens the import dialog`;
    - the ported `pickerGroups`, `priceHint` and `localHint` cases.
  - `ImportDialog.test.tsx`:
    - `a picked file is read and its type shown, with Copy preselected from the last choice`;
    - `a file inside the models folder skips the question`;
    - `a LoRA file is added as a LoRA`;
    - `a .ckpt file says Studio can't read .ckpt files`;
    - `progress shows while the file is read, then the model is selected`.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `npx vitest run src/components/studio/ModelSheet.test.tsx src/components/studio/ImportDialog.test.tsx`
  - Expected: FAIL.

- [ ] **Step 3: Implement.**

- [ ] **Step 4: Run them and see them pass, and take the screenshots.**
  - Run: `npm run lint && npm run typecheck && npm test`. Expected: PASS.
  - Then: `npm run preview:shoot -- --only b,b2,z2,n,q --out .cache/preview/after`, with new screens `n-import-dialog` and `q-model-sheet-narrow` (900×650). Expected: one-line rows, no repeated Recent rows, and no sideways scroll.

- [ ] **Step 5: Commit.**
```bash
git add src/components/studio src/store/studio.ts src/lib/words.ts e2e/preview
git commit -m "Studio: the model sheet (search, recent, on this computer, cloud) and Add a model file"
```

### Task U5: The Get models view

**Files:**
- Create in `src/components/studio/`:
  - `GetModels.tsx` (`GetModelsView`, `GetModelsHeader`, `HardwareChips`);
  - `ModelSearch.tsx` (`ModelSearch`);
  - `ModelCard.tsx` (`CatalogCard`, `ResultCard`, `InstalledCard`, `LoraCard`, `VerdictBadge`, plus `LicenceBadge`, `FitLine`, `TimeLine`, `useModelDownload` and `DownloadProgress` moved from `LocalModelRow.tsx`);
  - `GetModels.test.tsx`.
- Modify:
  - `StudioView.tsx`: `localView` renders `GetModelsView` full width, with neither the composer nor the side panel, and resets the scroller to the top when it opens (critique §2 and §6: it opened at the bottom);
  - `TopBar.tsx` (Get models while open reads as pressed);
  - `src/store/studioLocal.ts`: `searchFilter: SearchFilter` and `fitsOnly: boolean` replace `localTab` and `onlyFits`;
  - `EngineLine.tsx` (shown in the header);
  - `src/lib/words.ts`.
- Delete: `LocalModels.tsx` and `LocalModelRow.tsx`. `LocalModels.test.tsx` becomes `GetModels.test.tsx`: its engine-line and notice cases are kept, and its list cases are replaced. `LocalSearch.tsx` stays only for its `RepoResult`, shown in a `SlideOver` when a result opens, until Task U6 replaces it.
- Test: `GetModels.test.tsx`, `StudioView.test.tsx`

**Interfaces:**
- Consumes:
  - `LocalModels` (`hardware`, `engine`, `recommended: LocalEntry[]`, `installed`, `loras`);
  - `search(text, filter)` and `localSearch.hits: LocalSearchHit[]` (`author`, `downloads`, `likes`, `updatedAt`, `verdict`, `exact`);
  - `repos`, `openRepo`, `deleteLocal`, `repairLocal`, `deleteLora` and `openImport` (Tasks S12 and U4).
- Produces:
```ts
export function GetModelsView(): JSX.Element;
export function VerdictBadge(p: { verdict: Verdict; exact: boolean }): JSX.Element;   // words below; a hint is lighter, with Tip "From the name: opening it reads the files"
export function ResultCard(p: { hit: LocalSearchHit; onOpen(): void }): JSX.Element;  // an <article> named by the repo; its open control is the card's heading button
```
- **The header:** "Get models" at `text-view-title`; this computer as `HardwareChips` (graphics card and memory, RAM, free disk); *Add a model file* and *Add a LoRA file*; Back.
- **The search** (design 10.3):
  - a large field labelled "Search Hugging Face", placeholder "Search Hugging Face: SDXL, Flux, Wan 2.2, or a repo name";
  - a `ChipGroup` with Pictures · Video · LoRAs, starting from the tab;
  - a `SwitchField` *Fits this computer*, which hides results already opened (`repos[repo]`) with no `best` file. Results not yet opened stay.
- **Without a query:**
  - *Recommended for this computer* (`CatalogCard`s);
  - *Popular on Hugging Face*: `search("", filter)`, asked once each time the view opens. Task D1 documents this request;
  - *On this computer*: `InstalledCard`s (Delete, Repair) and `LoraCard`s (Delete);
  - all in a grid of 2–3 columns.
- **Result cards:** name and author; downloads and likes; "updated <relative date>"; `VerdictBadge`.
- Words:
  - `GET_MODELS`: `title`, `back` "Back", `searchLabel`, `searchPlaceholder`, `pictures`, `video`, `loras`, `fitsOnly` "Fits this computer", `recommended` "Recommended for this computer", `popular` "Popular on Hugging Face", `here` "On this computer", `addModel`, `addLora`, `downloads(n)`, `likes(n)`, `updated(date)`, `gpu(name, gb)`, `ram(gb)`, `freeDisk(gb)`;
  - `VERDICT`: `works(type)` "Works in Studio · SDXL", `lora(type)` "LoRA for FLUX.1", `cantRun(reason)` "Can't run: Lumina 2" (a short form per `CannotRunReason`), `unknown` "Open to check", `hint`.

**Steps**

- [ ] **Step 1: Write the failing tests.**
  - `GetModels.test.tsx`:
    - `Get models opens as a full view at its top, without the composer or the side panel`: the scroller's `scrollTop` is 0, and there is no "Prompt" textbox;
    - `Back returns to the results and the composer`;
    - `without a query it shows Recommended, Popular on Hugging Face and On this computer, and asks for the popular list once`: `search` is called with "" and "pictures";
    - `the header shows this computer as chips`;
    - `the LoRAs chip searches LoRAs`;
    - `a result card shows author, downloads, likes, when it was updated and its verdict in words`: "Works in Studio · SDXL", "LoRA for FLUX.1", "Can't run: Lumina 2";
    - `a hint becomes exact once the result was opened`;
    - `Fits this computer hides opened results that have no file for this computer`;
    - `installed models and LoRAs offer Delete, and models offer Repair`;
    - the ported engine-line and notice cases.
  - `StudioView.test.tsx`: `the Get models button reads as pressed while the view is open`.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `npx vitest run src/components/studio/GetModels.test.tsx src/components/studio/StudioView.test.tsx`
  - Expected: FAIL.

- [ ] **Step 3: Implement.**

- [ ] **Step 4: Run them and see them pass, and take the screenshots.**
  - Run: `npm run lint && npm run typecheck && npm test`. Expected: PASS.
  - Then: `npm run preview:shoot -- --only c0,c,c2,d --out .cache/preview/after`. Expected: `c` and `c2` are the same (opened at the top), and search and verdict badges are above the fold.

- [ ] **Step 5: Commit.**
```bash
git add src/components/studio src/store/studioLocal.ts src/lib/words.ts e2e/preview
git commit -m "Studio: Get models as a full view: search with filters, verdict badges, recommended, popular and installed"
```

### Task U6: A result's detail panel

**Files:**
- Create in `src/components/studio/`:
  - `ModelDetail.tsx` (`ModelDetail`, `MainFileList`, `PlanParts`, `OtherFiles`);
  - `ModelDetail.test.tsx`.
- Modify:
  - `GetModels.tsx`: result and catalog cards open `ModelDetail`;
  - `src/store/studioLocal.ts`: `detail` with `openDetail` and `closeDetail`; `offer`, `setOffer` and `downloadLocal` go;
  - `src/lib/words.ts`.
- Delete: `LocalSearch.tsx`, `DownloadLocalModel.tsx` and `localOffers.ts`. `DownloadLocalModel.test.tsx` becomes `ModelDetail.test.tsx`: the licence-wording cases of `verdict()` are kept, and the dialog cases are replaced, because the panel is the confirmation. `LocalNotice.tsx` and its test are unchanged.
- Test: `ModelDetail.test.tsx`

**Interfaces:**
- Consumes:
  - `openRepo` → `LocalRepo`: `summary`, `files: LocalFileChoice[]` (`quant`, `label`, `plan`, `best`), `fileViews`, `loras`, `licence(s)` and `gate`;
  - a catalog `LocalEntry` (`parts`, `licences`, `toFetchBytes`, `fit`);
  - `setupPlan(source, choice)`, `downloadSetup(source, choice)` and `downloadLora`;
  - `chooseFile("model")`, giving `SlotOverride { role, file: { disk: { path } } }`;
  - `api.studioLocalDownloadFile` and `noticeOpen` (Task S12);
  - `LicenceBadge`, with OpenRAIL in amber (Task S6).
- Produces:
```ts
// store/studioLocal.ts
detail: { repo: string } | { entryId: string } | null;
openDetail(d: { repo: string } | { entryId: string }): Promise<void>;   // a repo is read with openRepo
closeDetail(): void;
export function ModelDetail(): JSX.Element | null;                      // a SlideOver, width "half", titled by the repo or model name
```
- **The panel** (design 10.3), top to bottom:
  1. the summary;
  2. *Main file*: a radio group, one per quantisation or precision, each with its size and fit word. *Best for this computer* is marked and preselected; with none, the smallest is;
  3. *Model type*: a `SelectField`, when `plan.recipes` has more than one recipe;
  4. *Extra files Studio adds* (`PlanParts`): each with `SLOT_LABEL`, size, licence and "already on this computer". A missing part offers *Choose a file*;
  5. the licence(s) and "Total to download: X";
  6. one **Download**, off with the reason while a part is missing or a gated repo has no token, with the token's link;
  7. `OtherFiles`: files Studio can't run, each with its reason and a quiet *Download file anyway*; LoRA files, each with *Download LoRA*.
- **Download.**
  - Download calls `downloadSetup` directly. There is no second dialog; this keeps the four-click target.
  - The first download still shows the local-model notice, as now.
- Words: `MODEL_DETAIL`: `mainFile` "Main file", `best` "Best for this computer", `modelType` "Model type", `extras` "Extra files Studio adds", `alreadyHere` "already on this computer", `chooseFile` "Choose a file", `total(size)`, `download` "Download", `missingWhy` "Choose a file for each part first.", `needsToken`, `anyway` "Download file anyway", `downloadLora` "Download LoRA".

**Steps**

- [ ] **Step 1: Write the failing tests** (`ModelDetail.test.tsx`):
  - `opening a result shows its summary, one choice per quantisation, and Best for this computer on the largest that fits`;
  - `the best choice is preselected, and the plan lists each extra file in words with its size and "already on this computer"`;
  - `the total counts only the files to download`;
  - `changing the model type asks for a new plan`: `setupPlan` is called with `{ recipe: "flux1-dev", overrides: [] }`;
  - `a missing part offers Choose a file, and Download says why it is off`;
  - `Download starts the download with the choice, with no other dialog`: `downloadSetup` is called once;
  - `the first Download shows the local-model notice first`;
  - `a file Studio can't run shows its reason and a quiet Download file anyway`;
  - `an OpenRAIL licence reads amber "Commercial use allowed, with use rules" with a link`;
  - `the panel traps focus, and Esc closes it with focus back on the card`;
  - the kept `verdict()` licence-wording cases.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `npx vitest run src/components/studio/ModelDetail.test.tsx`
  - Expected: FAIL.

- [ ] **Step 3: Implement.**

- [ ] **Step 4: Run them and see them pass, and take the screenshots.**
  - Run: `npm run lint && npm run typecheck && npm test`. Expected: PASS.
  - Then: `npm run preview:shoot -- --only d2,d3,e0,e --out .cache/preview/after`. Here `e` is the panel's plan, in place of the 0.1.2 dialog. Expected: one Download per result, and the best file marked.

- [ ] **Step 5: Commit.**
```bash
git add src/components/studio src/store/studioLocal.ts src/lib/words.ts e2e/preview
git commit -m "Studio: a result's detail panel: one choice per quantisation, the best for this computer, extra files, one Download"
```

### Task U7: Downloads: progress, Pause and Cancel, the top-bar indicator and *Use it*

**Files:**
- Create in `src/components/studio/`:
  - `DownloadState.tsx` (`DownloadState`, plus `useModelDownload` moved from `ModelCard.tsx`);
  - `DownloadIndicator.tsx` (`DownloadIndicator`);
  - `DownloadState.test.tsx`;
  - `sizes.test.ts`.
- Modify:
  - `ModelCard.tsx` and `ModelDetail.tsx` (show `DownloadState`);
  - `TopBar.tsx` (the indicator);
  - `src/store/studio.ts`: `pickNewModel`, and the download follower;
  - every file under `src/components/studio/` and `src/store/studio*.ts` that uses `amountGb` (critique §2.7 and §6: two formats on one card);
  - `src/lib/words.ts`.
- Test: `DownloadState.test.tsx`, `GetModels.test.tsx`, `StudioView.test.tsx`, `sizes.test.ts`

**Interfaces:**
- Consumes:
  - `useDownloads` groups (`DownloadGroup.genModelId`, `loraId` from Task S10, `bytesDone`, `bytesTotal`, `status`, `bytesPerSecond`);
  - `ipc.pauseDownload`, `ipc.resumeDownload` and `ipc.cancelDownload` (`src/lib/ipc.ts:267-269`);
  - `refreshLocal`, `loadLoras` and `chooseModel`.
- Produces:
```ts
// store/studio.ts
pickNewModel(genModelId: string): void;   // chooses the installed model for its kind (tab switched to it), closes Get models and the detail panel; not named use…, which the hooks lint rule reserves
// the follower: subscribed to `download-progress` when Studio loads, not by a view; a group with genModelId or loraId that
// reaches "completed" triggers refreshLocal() or loadLoras() (critique §6: 0.1.2 only re-read while Get local models was open)
export function DownloadState(p: { group: DownloadGroup; name: string; onUse?(): void }): JSX.Element;   // bar, "2.4 GB of 6.7 GB · 2 min left", Pause/Resume, Cancel, Use it
export function DownloadIndicator(): JSX.Element | null;   // top bar; hidden with no Studio download; a popover lists them, each a DownloadState
```
- **Sizes.** Studio shows sizes with `formatBytes` (decimal GB) only. Plan decision: it is what Hugging Face and the chat's Models dialog show (critique §5).
- Words: `STUDIO_DOWNLOADS`: `indicator(n)` "2 downloading", `progress(done, total, left)`, `pause(name)`, `resume(name)`, `cancel(name)`, `useIt` "Use it", `ready` "Ready".

**Steps**

- [ ] **Step 1: Write the failing tests.**
  - `DownloadState.test.tsx`:
    - `progress reads in one size format`: "2.4 GB of 6.7 GB";
    - `Pause and Cancel are buttons named for the model, and call the download commands`;
    - `a finished download offers Use it`.
  - `GetModels.test.tsx`:
    - `a finished model's card offers Use it, which selects it and returns to the results`;
    - **`using a new model takes at most 4 clicks`.** From the results, with the notice already accepted, it counts every `fireEvent.click`:
      1. *Get models*;
      2. a result card;
      3. *Download* (then the test emits `download-progress` with status `completed` for its `genModelId`);
      4. *Use it*.

      It asserts `clicks <= 4`, the draft's model is the new one, `localView === false`, and the composer is shown (design 10.3).
    - **`a download finished elsewhere lists the model and offers Use it`** (Review Focus 3). With Get models closed and the results shown, `download-progress` completes a group with a `genModelId`. Then `refreshLocal` has run, the model is a row of the model sheet, and the indicator's popover offers *Use it*.
  - `StudioView.test.tsx`: `the top-bar indicator counts running downloads and lists them`.
  - `sizes.test.ts`: `no Studio source uses amountGb`. It uses `import.meta.glob("./**/*.{ts,tsx}", { query: "?raw", eager: true })`, and `src/store/studio*.ts` the same way.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `npx vitest run src/components/studio`
  - Expected: FAIL.

- [ ] **Step 3: Implement.**

- [ ] **Step 4: Run them and see them pass, and take the screenshots.**
  - Run: `npm run lint && npm run typecheck && npm test`. Expected: PASS.
  - Then: `npm run preview:shoot -- --only e2,p --out .cache/preview/after`, with a new screen `p-download-indicator` (the popover open). Expected: Pause and Cancel on the card, and one size format.

- [ ] **Step 5: Commit.**
```bash
git add src/components/studio src/store src/lib/words.ts e2e/preview
git commit -m "Studio: downloads show progress with Pause and Cancel everywhere, a top-bar indicator, and Use it in four clicks"
```

### Task U8: The Settings panel: My setups, Advanced, LoRAs and Files

**Files:**
- Create in `src/components/studio/`:
  - `LorasSection.tsx` (`LorasSection`, `LoraRow`) and `LorasSection.test.tsx`;
  - `ModelFilesSection.tsx` (`ModelFilesSection`, `SlotRow`, `ChangeFileMenu`, `HubFileDialog`) and `ModelFilesSection.test.tsx`.
- Modify:
  - `SettingsPanel.tsx`: sections `MySetups`, `AdvancedSection`, `LorasSection` and `ModelFilesSection`; it scrolls to `settingsSection`;
  - `AdvancedDrawer.tsx`, `ParamField.tsx` and `SettingField.tsx`:
    - native `select`, range and checkbox become `SelectField`, `SliderField` and `CheckboxField`/`SwitchField` (critique §2.8);
    - Sampler and Scheduler are both `SelectField`s;
    - "Automatic" is the control's placeholder, not a second label (critique §4.f);
  - `Composer.tsx`: a *LoRAs (n)* toolbar button for local models with `takesLoras`, which calls `setSettingsOpen(true, "loras")` (design 10.1);
  - `src/lib/words.ts`.
- Test:
  - `SettingsPanel.test.tsx` (rewritten: its My setups cases are kept with the new select, and its Shape/How many cases move to `Composer.test.tsx` in Task U3);
  - `AdvancedDrawer.test.tsx` and `ParamField.test.tsx` (updated for the Radix roles `combobox`, `slider` and `switch`, with the same behaviours);
  - `LorasSection.test.tsx`, `ModelFilesSection.test.tsx`.

**Interfaces:**
- Consumes:
  - `loras`, `lorasFor(model)`, `attachLora`, `setLoraWeight`, `setLoraHighNoise` and `detachLora`;
  - the param `ParamKind` `loras` (`max`, `highNoise`, `weightMin`, `weightMax`, `weightStep`, `weightDefault`);
  - `InstalledLocal.highNoise`, `takesLoras`, `loraBase`, `needsSetup`, `untested` and `recipe`;
  - `modelFiles(id)` → `ModelFiles` (`slots: PlannedSlot[]`, `recipes`, `untested`, `canRun`), and `setModelFiles(id, choice)` → `SetModelFilesResult`;
  - `chooseFile`, `openRepo` (for a Hugging Face file), `saveSetup`, `applySetup` and `openImport("lora")`.
- Produces:
```ts
export function LorasSection(p: { tab: StudioTab; model: InstalledLocal }): JSX.Element;
export function ModelFilesSection(p: { model: InstalledLocal }): JSX.Element;
```
- **LoRAs** (design 8):
  - *Add* is a `MenuButton` listing `lorasFor(model).fits`, then `others`, each marked "may not fit this model".
  - Each attached LoRA shows its name, a `SliderField` "Strength of <name>" (−1 to 2, step 0.05, default 1), the *High-noise part* `SwitchField` only when `model.highNoise`, and *Remove*.
  - Under them: *Add a LoRA file* (`openImport("lora")`) and *Get LoRAs* (Get models with the LoRAs filter).
- **Files** (design 7.5):
  - The *Model type* `SelectField` (`recipes`), then one `SlotRow` per planned slot: `SLOT_LABEL`, the file name, size and "required".
  - *Change* is a `ChangeFileMenu` with *A file on this computer*, *A file from disk…* and *A file from Hugging Face…*. The last is a small dialog: a repo name, then a file from its `fileViews`.
  - "untested set-up" shows when `untested`.
  - A model that `needsSetup` opens here, and Create stays off until `canRun`.
- **The panel** never holds Shape or How many (design 10.1).
- Words:
  - `STUDIO_PANEL`: `title` "Settings", `setups` "My setups", `advanced` "Advanced", `loras` "LoRAs", `files` "Files";
  - `LORAS` additions: `strengthOf(name)`, `getLoras` "Get LoRAs", `lorasButton(n)` "LoRAs (1)";
  - `FILES`: `change` "Change", `installedFile` "A file on this computer", `diskFile` "A file from disk…", `hubFile` "A file from Hugging Face…", `repoLabel` "Repository", `untested`, `needsSetup` "Choose the model type and its files to use this model.".

**Steps**

- [ ] **Step 1: Write the failing tests.**
  - `SettingsPanel.test.tsx`:
    - `the panel holds My setups, Advanced, LoRAs and Files, and never Shape or How many`;
    - `My setups is a select named "My setups", and applying one fills the draft`.
  - `LorasSection.test.tsx`:
    - `Add lists the LoRAs for this model first, and marks the others "may not fit this model"`;
    - `a LoRA's strength slider goes from -1 to 2 in steps of 0.05, starts at 1, and is named after the LoRA`;
    - `a Wan 2.2 A14B model shows the High-noise part switch, and others do not`;
    - `the composer's LoRAs button shows the count and opens this section`.
  - `ModelFilesSection.test.tsx`:
    - `Files lists each part with its role in words, and Change offers installed, disk and Hugging Face files`;
    - `choosing a disk file calls setModelFiles with a disk override`;
    - `a model type the detection did not pick shows the untested set-up note`;
    - `a model that needs setting up opens Files, and Create is off until it can run`.
  - `AdvancedDrawer.test.tsx` and `ParamField.test.tsx`:
    - `Sampler and Scheduler are selects listing every engine choice by its plain name`: 21 and 17 options;
    - the existing cases, with the new roles.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `npx vitest run src/components/studio`
  - Expected: FAIL.

- [ ] **Step 3: Implement.**

- [ ] **Step 4: Run them and see them pass, and take the screenshots.**
  - Run: `npm run lint && npm run typecheck && npm test`. Expected: PASS.
  - Then: `npm run preview:shoot -- --only f,f2,l,m --out .cache/preview/after`, with new screens `l-loras` (two LoRAs attached) and `m-model-files` (the untested model). Expected: no native controls in the panel.

- [ ] **Step 5: Commit.**
```bash
git add src/components/studio src/lib/words.ts e2e/preview
git commit -m "Studio: the Settings panel with My setups, Advanced, LoRAs in full and the model's files"
```

### Task U9: Results and polish

**Files:**
- Modify in `src/components/studio/`:
  - `RunCard.tsx`, `results.tsx` (`ResultTile`) and `Working.tsx`;
  - `Grid.tsx`: a Select mode, with no checkbox on every tile;
  - `EmptyStates.tsx`;
  - `Viewer.tsx`: its native checkbox becomes `CheckboxField`, and help in `title` becomes `Tip`;
  - `StudioView.tsx`: the grid opens at its top (critique §6, g2), and the page uses `.studio-page`.
- Also modify `src/lib/words.ts`.
- Test:
  - `RunCard.test.tsx`, `Grid.test.tsx`, `Viewer.test.tsx` and `StudioView.test.tsx` (updated);
  - `EmptyStates.test.tsx` (new);
  - the strict harness run.

**Interfaces:**
- **Run cards** (design 10.4):
  - thumbnails up to 256 px at the picture's own shape;
  - the prompt as the card's heading (`h3`, `text-section`);
  - settings and model as one `text-meta` line (`settingsLine.ts`, with the LoRA part from Task S12 and "untested set-up" when true);
  - progress as a thin bar with one status line;
  - a failure as an inline `role="alert"` with the message, the next step and its actions.
- **The grid:**
  - tiles of about 220 px, with actions on hover and focus, each named;
  - *Select* turns on a mode with a `CheckboxField` per tile and a bar of actions on the selection.
- **Empty states:** an icon, one line and a primary action.
  - "No models on this computer yet" offers *Get models*.
  - First use names the model that fits, as in 0.1.2, and its button opens that model's detail panel.
- **Polish:**
  - Light mode: the page is `bg-subtle`, and cards are `surface` with `border` and `shadow-soft`.
  - The accent gradient is on Create and primary buttons only.
- **The narrow window** (900×650): the composer toolbar wraps onto two lines, the side panel is an overlay, and nothing scrolls sideways.
- Words: `STUDIO_RESULTS`: `select` "Select", `selected(n)`, `done` "Done", `noModels` "No models on this computer yet", `getModels` "Get models", `deleteSelected(n)`.

**Steps**

- [ ] **Step 1: Write the failing tests.**
  - `RunCard.test.tsx`:
    - `a finished card leads with its pictures at their own shape, up to 256 px`;
    - `the prompt is the card's heading, and settings and model are one line`;
    - `a failure is an inline alert with its message, next step and actions`.
  - `Grid.test.tsx`:
    - `tiles have no checkbox until Select, and Select mode picks tiles and offers actions on them`;
    - `a tile's actions are buttons named for the picture, reachable by keyboard`.
  - `StudioView.test.tsx`: `the grid opens at its top`.
  - `EmptyStates.test.tsx`: `with no local models it says so and Get models opens Get models`.
  - `Viewer.test.tsx`: `saving the settings as a .json file is a checkbox named for it`.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `npx vitest run src/components/studio`
  - Expected: FAIL.

- [ ] **Step 3: Implement.**

- [ ] **Step 4: Run them and see them pass, and take the after set and the pairs.**
  - Run: `npm run lint && npm run typecheck && npm test`. Expected: PASS.
  - Then:
    - Run: `npm run preview:shoot -- --strict --out .cache/preview/after`
    - Expected: every screen in both themes, no unanswered command, no sideways scroll (including `j` at 900×650), and a body share of at least 0.5 on every Studio screen.
  - Then: `npm run preview:pairs -- .cache/preview/before .cache/preview/after`. Expected: `.cache/preview/pairs.html`, with the pairs design 10.5 names for the owner (main view `a`, model sheet `b`, Get models `c`, a result's detail panel `d2`, grid `g`).

- [ ] **Step 5: Commit.**
```bash
git add src/components/studio src/styles.css src/lib/words.ts e2e/preview
git commit -m "Studio: results lead with the pictures, a grid with a Select mode, empty states, and a window that fits at 900 px"
```

## Phase G: fewer blocks (section 9)

The inventory ids (S3–S11, S49, S54, C2–C5) and their `file:line` are from the restrictions inventory of release 0.1.2 (e7dc878).

### Task G1: Spending guards off by default, as optional switches

**Files:**
- Modify:
  - `src-tauri/src/studio/prefs.rs` (inventory S3 `108-109, 134`; inventory S4 `42-43, 195-197`; inventory S6 `40-41, 136, 189-194`);
  - `src-tauri/src/studio/spend.rs` (inventory S3 `38-54, 87-107`; inventory S5 `101-105`; inventory S7 `30-32, 210-217`);
  - `src-tauri/src/studio/manager.rs`: where `confirm_rules()` is used;
  - `src/components/settings/StudioSettings.tsx` (inventory S4 `28-30, 333-342`; inventory S6 `343-378`) and `src/components/settings/StudioSettings.test.tsx`;
  - `src/components/studio/TopBar.tsx` (the spend for today);
  - `src/components/studio/types.ts` (`StudioPrefs`, `Estimate.confirmAtUsd`);
  - `src/lib/words.ts` (inventory S4 `1818-1821`, inventory S6 `1822-1826`);
  - `e2e/m12_studio.py`: `c_video` turns *Ask before every video* on first; `c_daily_limit` turns *Daily limit* on before setting $1;
  - `scripts/docs_test.py` (`test_the_money_rules_are_the_codes`);
  - `SECURITY.md` and `README.md`: the spending sentences.
- Test: inline in `prefs.rs`, `spend.rs` and `manager.rs`; `StudioSettings.test.tsx`

**Interfaces:**
- Produces:
```rust
pub const PREFS_VERSION: u32 = 1;
pub struct StudioPrefs { #[serde(default)] pub prefs_version: u32, pub confirm_every_video: bool, pub confirm_at_usd: Option<f64>,
    #[serde(default)] pub confirm_unknown_price: bool, pub daily_limit_usd: Option<f64>, /* the rest as now */ }
// Default: prefs_version PREFS_VERSION, confirm_every_video false, confirm_at_usd None, confirm_unknown_price false, daily_limit_usd None
pub fn migrate(prefs: &mut StudioPrefs) -> bool;   // run on load; true when it changed something (the file is then saved)
pub struct StudioPrefsPatch { /* … */ #[serde(default, with = "double_option")] pub confirm_at_usd: Option<Option<f64>>, pub confirm_unknown_price: Option<bool> }
// spend.rs
pub struct ConfirmRules { pub every_video: bool, pub at_usd: Option<f64>, pub unknown_price: bool }
```
- **`migrate`**, once, only when `prefs_version < 1`:
  - `confirm_every_video: true` → `false`;
  - `confirm_at_usd: Some(1.0)` → `None`;
  - `daily_limit_usd: Some(10.0)` → `None`;
  - `confirm_unknown_price` → `false` (it was always on);
  - then `prefs_version = 1`.
  - Any other value stays.
  - An old file stores `confirmAtUsd` as a number; it reads as `Some`.
- **`confirm_reasons`** gives:
  - `Video` only when `every_video`;
  - `AtLeast` only when `at_usd` is `Some(x)` and the estimate is ≥ x;
  - `UnknownPrice` only when `unknown_price`.
- `check_limit` is unchanged: `Ok` when there is no limit, and the $5 reserve (`UNKNOWN_PRICE_RESERVE_USD`) only with a limit.
- **The constants stay.** `CONFIRM_AT_USD` 1.0 and `DEFAULT_DAILY_LIMIT_USD` 10.0 are the amounts filled in when a switch is turned on. Plan decision: they are familiar starting amounts, and `docs_test` keeps pinning them.
- **Settings → Studio → Spending** has four `SwitchField`s (Task U2), each labelled and all off. Turning one on shows its amount field, with the same ranges as now: $1–$1,000 and $0.25–$100.
- **The top bar's spend for today** shows while any of the four switches is on. Task U3 showed it for the daily limit only (design 10.1).

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[test] fn old_default_spending_prefs_move_to_off_once_and_changed_ones_stay() {
    let dir = tempdir(); write(&dir, r#"{"confirmEveryVideo": true, "confirmAtUsd": 1.0, "dailyLimitUsd": 10.0, "gpuPolicy": "unload"}"#);
    let p = load(&dir);
    assert_eq!((p.confirm_every_video, p.confirm_at_usd, p.daily_limit_usd, p.confirm_unknown_price, p.prefs_version), (false, None, None, false, 1));
    assert!(read(&dir).contains(r#""prefsVersion":1"#));
    save(&dir, StudioPrefs { confirm_every_video: true, ..p }); assert!(load(&dir).confirm_every_video);       // never migrated again
    write(&dir, r#"{"confirmEveryVideo": false, "confirmAtUsd": 2.5, "dailyLimitUsd": 25.0}"#);
    let q = load(&dir);
    assert_eq!((q.confirm_every_video, q.confirm_at_usd, q.daily_limit_usd), (false, Some(2.5), Some(25.0)));
}
#[test] fn a_new_install_starts_with_every_spending_switch_off() {
    let p = StudioPrefs::default();
    assert_eq!((p.confirm_every_video, p.confirm_at_usd, p.confirm_unknown_price, p.daily_limit_usd), (false, None, false, None));
}
#[test] fn nothing_asks_when_every_switch_is_off() {                                     // spend.rs
    let off = ConfirmRules { every_video: false, at_usd: None, unknown_price: false };
    assert!(confirm_reasons(&video_estimate(Some(3.0)), &off).is_empty());
    assert!(confirm_reasons(&picture_estimate(None), &off).is_empty());
}
#[test] fn each_switch_adds_its_reason() {
    assert_eq!(confirm_reasons(&video_estimate(Some(0.2)), &ConfirmRules { every_video: true, at_usd: None, unknown_price: false }), [ConfirmReason::Video]);
    assert_eq!(confirm_reasons(&picture_estimate(Some(1.5)), &ConfirmRules { every_video: false, at_usd: Some(1.0), unknown_price: false }), [ConfirmReason::AtLeast]);
    assert_eq!(confirm_reasons(&picture_estimate(None), &ConfirmRules { every_video: false, at_usd: None, unknown_price: true }), [ConfirmReason::UnknownPrice]);
}
#[tokio::test] async fn a_cloud_video_starts_without_confirmation_when_the_switch_is_off() { /* manager.start with confirmed: false → Ok */ }
```
In `StudioSettings.test.tsx`: `the four spending switches are off by default and turning one on shows its amount`. Turning *Daily limit* on shows "$10", and *Ask before runs of $X or more* shows "$1".

In `docs_test.py`, `test_the_money_rules_are_the_codes` keeps the three constants and asserts that `SECURITY.md` and `README.md` each contain "off by default" and "$10".

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p app --lib studio::prefs studio::spend studio::manager && npx vitest run src/components/settings/StudioSettings.test.tsx`
  - Expected: FAIL.

- [ ] **Step 3: Implement**, and rewrite the spending sentences:
  - **SECURITY.md:** "Spending guards are off by default: a daily limit (suggested $10), asking before every video, before runs of $1 or more, and when the price is unknown are switches in Settings → Studio. Prices are still shown before every cloud run."
  - **README.md:** one sentence with the same facts.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p app --lib && npm test && python3 -I scripts/docs_test.py`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add src-tauri/src/studio src/components src/lib/words.ts e2e/m12_studio.py scripts/docs_test.py SECURITY.md README.md
git commit -m "Studio: spending guards are optional switches, off by default; old default settings move to off once"
```

### Task G2: Local run blocks become warnings, in the webview

The backend side, removing the refusals and judging video on the processor, is Task S11. Phase U builds the new composer, cards and detail panel without any fit-based disabling, and the warning wording is defined in Task S12. This task adds the warning lines where the person decides, offers video on processor-only computers, and removes every old refusal (inventory S10, S11, S12).

**Files:**
- Modify:
  - `src/components/studio/Composer.tsx` and `Composer.test.tsx` (Task U3; 0.1.2's block was `PromptDock.tsx`, inventory S10 `99-117, 188-191`);
  - `src/components/studio/ModelCard.tsx` and `ModelDetail.tsx` (Tasks U5 and U6; 0.1.2's were `LocalModelRow.tsx`, inventory S10 `200-235`, and `LocalModels.tsx`, inventory S10 `53-63, 203-216`);
  - `src/components/studio/TopBar.tsx`: the Video tab on processor-only computers (inventory S11);
  - `src/lib/words.ts`: remove inventory S10 `1157-1164, 1408-1414, 1880` and inventory S11 `1115-1117, 1230-1231`;
  - `src-tauri/src/studio/errors.rs`: remove inventory S11/S12 `1226-1229`, the "video needs a graphics card" reasons.
- Test: `Composer.test.tsx`, `GetModels.test.tsx`, `ModelDetail.test.tsx`, `StudioView.test.tsx`, and a new `src/lib/words.test.ts`

**Interfaces:**
- Consumes:
  - `LocalFit.word`, `LocalFit.reason` and `LocalFit.videoOnProcessor`, and `LocalModels.videoOnProcessor` (Task S11);
  - `LOCAL_WARNING` (Task S12).
- Produces:
  - A warning line, `role="status"`, beside Create, under a card's fit and in the detail panel's plan. It is never a disabled state.
  - The *Only what fits* filter, now Get models' *Fits this computer* switch, stays (inventory S21, not the owner's pick).

**Steps**

- [ ] **Step 1: Write the failing tests.**
  - `Composer.test.tsx`:
    - `a model that may not fit keeps Create enabled and shows the warning`: fit `{ word: "notHere", reason: "needs 6 GB of graphics memory" }` gives an enabled Create button and the text "May not fit this computer: needs 6 GB of graphics memory. It may fail or be very slow.".
    - `video on the processor shows its warning`.
  - `GetModels.test.tsx`: `a card that may not fit shows the warning and still offers Download and Download again`.
  - `ModelDetail.test.tsx`: `a choice that may not fit shows the warning and Download stays on`.
  - `StudioView.test.tsx`: `the Video tab is offered on a computer that runs models on the processor`.
  - `words.test.ts`: `no wording says a model is not for this computer`. Every string reachable from `words.ts`'s exports contains neither "Not for this computer" nor "needs a graphics card or an Apple-silicon Mac".

- [ ] **Step 2: Run them and see them fail.**
  - Run: `npx vitest run src/components/studio src/lib/words.test.ts`
  - Expected: FAIL.

- [ ] **Step 3: Implement.**

- [ ] **Step 4: Run them and see them pass.**
  - Run: `npm run lint && npm run typecheck && npm test && cargo test -p app --lib studio::errors`
  - Expected: PASS.
  - Then: `npm run preview:shoot -- --only a,c,d2,k --out .cache/preview/after`. Expected: the warning lines show, with no sideways scroll.

- [ ] **Step 5: Commit.**
```bash
git add src/components/studio src/lib src-tauri/src/studio/errors.rs e2e/preview
git commit -m "Studio: 'May not fit this computer' is a warning; Create, Download and Download again stay; video on the processor is offered"
```

### Task G3: One 1 MiB prompt bound, Create that finishes the price, and updates that stop the job

**Files:**
- Modify:
  - **The prompt bound (inventory S9):**
    - `crates/runtime/src/sd/args.rs` (`22-24, 336-337`);
    - `crates/agent/src/media/spec.rs` (`44-45, 434-435`);
    - `src-tauri/src/studio/manager/lane.rs` (`405-407`);
    - `src-tauri/src/studio/local.rs` (`714-716`);
    - `src-tauri/src/studio/errors.rs` (`365-366, 1086-1098, 1255-1257`);
    - `src/store/studioRun.ts` (`276-302`);
    - `src/components/studio/Composer.tsx` (Task U3; 0.1.2's cap was at `PromptDock.tsx:107-110, 150-162`).
  - **Waiting for the price (inventory S49):** `src/store/studio.ts` (`216-236, 409-416, 470-480`) and `src/components/studio/Composer.tsx` (0.1.2: `PromptDock.tsx:114-115, 168`).
  - **Updates while a local job runs (inventory S54):** `src-tauri/src/updates.rs` (`660-669, 675-677`), `src/components/UpdateBox.tsx` and their tests.
  - `src/lib/words.ts`.
  - `ARCHITECTURE.md` §7.9: the 4,000-character sentence and "Installing an update is refused…".
  - `scripts/docs_test.py` (`test_7_9_states_the_local_rules_the_code_keeps`).
- Test: inline in `args.rs`, `spec.rs`, `lane.rs` and `updates.rs`; `src/store/studio.test.ts`; `src/components/UpdateBox.test.tsx`

**Interfaces:**
- Produces:
```rust
// crates/runtime/src/sd/args.rs (replaces PROMPT_MAX_CHARS)
pub const PROMPT_MAX_BYTES: usize = 1_048_576;      // 1 MiB, a safety bound against pathological input; never shown as a limit
// crates/agent/src/media/spec.rs: the cloud prompt and every Text field use runtime::sd::args::PROMPT_MAX_BYTES (bytes of UTF-8)
```
- **The prompt bound:**
  - The negative prompt's `ParamKind::Text { max_chars }` becomes 1,048,576.
  - The webview's "{n} left" counter shows only when fewer than 1,000 remain. Plan decision: it stays out of sight for any real prompt.
  - The one refusal copy, for local and cloud alike, is "This prompt is too long." / "Shorten it."
- **Waiting for the price.**
  - Pressing Create while extra settings wait for a model's price awaits that request, then sends.
  - When it fails, it sends without the extra settings and shows "Sent without the extra settings: their price couldn't be read.".
  - Create is no longer disabled while it waits.
- **Updates while a local job runs.**
  - `refuse_while_studio_works` (`updates.rs:663`) becomes `pub(crate) async fn stop_studio_for_update(studio: Option<&StudioManager>)`. It awaits `studio.stop_local()` (`manager/lane.rs:355`) when `local_running()`, and returns, so the install goes ahead.
  - `UpdateBox` shows "Installing stops the picture being made" (a `role="note"` line above the install button) while a local job runs.
  - The `busy_for_update` error and its copy (`errors.rs:1122-1130`) are removed.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[test] fn a_prompt_up_to_1_mib_is_accepted_and_one_byte_more_refused() {          // args.rs
    let mut j = job(); j.prompt = "a".repeat(5_000); assert!(build_args(&j).is_ok());
    j.prompt = "a".repeat(PROMPT_MAX_BYTES); assert!(build_args(&j).is_ok());
    j.prompt.push('a'); assert_eq!(build_args(&j), Err(ArgsError::PromptTooLong));
}
#[test] fn a_cloud_prompt_of_40000_characters_is_accepted() { /* spec.rs validate */ }
#[tokio::test] async fn a_local_prompt_of_5000_characters_runs() { /* lane.rs start_local → Ok */ }
#[tokio::test] async fn installing_while_a_local_job_runs_stops_it_and_installs() {   // updates.rs
    let t = update_harness(local_job_running()).await;
    assert!(t.install().await.is_ok()); assert_eq!(t.stop_local_calls(), 1);
}
```
- `studio.test.ts`:
  - `Create while a price is pending waits for it, then sends with the extras`.
  - `Create sends without extras when the price request fails, and says so`.
- `UpdateBox.test.tsx`: `the warning line shows while a local job runs`.
- `docs_test.py` asserts "1 MiB" in §7.9 and `pub const PROMPT_MAX_BYTES: usize = 1_048_576;` in `args.rs`, in place of the 4,000 checks.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p runtime --lib sd::args && cargo test -p agent --lib media::spec && cargo test -p app --lib updates:: studio::manager && npm test`
  - Expected: FAIL.

- [ ] **Step 3: Implement**, and edit the two §7.9 sentences.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test --workspace && npm test && python3 -I scripts/docs_test.py`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add crates/runtime crates/agent src-tauri src/store src/components src/lib/words.ts ARCHITECTURE.md scripts/docs_test.py
git commit -m "Studio: one hidden 1 MiB prompt bound, Create finishes a pending price, and an update stops a local job"
```

### Task G4: Chat tool modes switch without "are you sure"

**Files:**
- Modify:
  - `src-tauri/src/approvals.rs` (inventory C5 `80-84, 304-341, 476-509`): remove `full_access_pending` with its setter, getter and taker, and the commands `get_full_access_default_pending` and `answer_full_access_default`;
  - `src-tauri/commands.list` (84–85), `src-tauri/capabilities/main.json` (111–112), `src-tauri/src/lib.rs` (203–204), and the two autogenerated permission files;
  - `src/components/AllowAllButton.tsx` (inventory C2 `10-13, 46-72, 155-196, 314-331, 412-420`; inventory C3 `74-88, 203-236`; inventory C5 `238-304`): `FullAccessStartCheck` and the confirm dialogs go;
  - `src/components/ApprovalCard.tsx` (inventory C2 `154-163, 181`) and `src/components/PermissionsSettings.tsx` (inventory C4 `154-160`);
  - the api wrappers of the two commands;
  - `src/lib/words.ts`: the dialogs' copy;
  - `SECURITY.md`: C5's protection is removed by the owner's choice.
- Test: inline in `approvals.rs`; `AllowAllButton.test.tsx`, `ApprovalCard.test.tsx`, `PermissionsSettings.test.tsx`

**Interfaces:**
- `load_default_into(store, modes)` applies the saved start mode, Full access included, with no pending state.
- Esc no longer touches the setting.
- Chats start in Ask first unless another start mode is saved (C1 unchanged).

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[tokio::test] async fn a_saved_full_access_default_applies_at_launch_without_asking() {
    let (store, modes) = with_saved_default(ApprovalMode::FullAccess).await;
    load_default_into(&store, &modes).await;
    assert_eq!(modes.mode_for_new_chat(), ApprovalMode::FullAccess);
}
#[tokio::test] async fn with_nothing_saved_chats_start_in_ask_first() { /* unchanged C1 */ }
```
- `AllowAllButton.test.tsx`: `switching to Allow all or Full access applies at once`. There is no `role="dialog"` after the click, and the mode command is called once.
- `ApprovalCard.test.tsx`: `Allow all in this chat applies at once`.
- `PermissionsSettings.test.tsx`: `changing the start mode saves without asking`.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p app --lib approvals:: && npx vitest run src/components/AllowAllButton.test.tsx src/components/ApprovalCard.test.tsx src/components/PermissionsSettings.test.tsx`
  - Expected: FAIL.

- [ ] **Step 3: Implement**, and add to SECURITY.md's approvals section:

  > "Since 0.2.0, switching a chat to Allow all or Full access, or saving either as the start mode, takes effect without a confirmation, and a saved Full-access start mode is applied at launch without being asked again. Earlier versions asked again at each launch, in case another program had changed the setting; the owner chose to drop that check."

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p app --lib && npm test && python3 -I scripts/docs_test.py`
  - Expected: PASS. `commands_list_matches_capabilities` passes with the two commands gone.

- [ ] **Step 5: Commit.**
```bash
git add src-tauri src/components src/lib SECURITY.md
git commit -m "Chat: switching to Allow all or Full access, and the saved start mode, no longer ask again"
```

## Phase F: fixes along the way (section 11)

### Task F1: The Reverse engineering job on Linux: the strace checker

**Files:**
- Create:
  - `scripts/check_strace.py`: the checker moved verbatim out of `.github/workflows/reverse.yml:79-245`, then changed only as below;
  - `scripts/check_strace_test.py`.
- Modify:
  - `.github/workflows/reverse.yml`, the Linux job:
    - `strace` runs with `-yy` on both runs (247 and 254);
    - the hosts line (246) replaces any line naming the host instead of appending;
    - the controls (247–251);
  - `.github/workflows/ci.yml`: add `python3 -I scripts/check_strace_test.py` next to `pin_reverse_test.py`;
  - `scripts/docs_test.py`: `test_the_linux_checker_fails_on_name_lookups` reads `htons(53)` from `scripts/check_strace.py`, and asserts that `reverse.yml` runs `scripts/check_strace.py`;
  - `ARCHITECTURE.md`: the checker bullet that `docs_test` reads names the UDP port-0 rule.
- Test: `scripts/check_strace_test.py`

**Interfaces:**
- Unchanged command line: `python3 -I scripts/check_strace.py <log> <rea-dir> <workdir> <allowed-program>...`; exit 1 with findings.
- **The cause** (spec correction 12):
  - `connect(…UDP…, {AF_INET, htons(0), 10.1.0.32})` is glibc `getaddrinfo` sorting two addresses for the host name (RFC 6724): it connects an unbound UDP socket to each, which sends nothing.
  - The host name has two addresses because line 246 appends `127.0.1.1 <host>` beside the runner's own line.
- **New rules:**
  1. With `-yy`, a socket fd prints as `N<UDP:[…]>`, `N<UDPv6:[…]>`, `N<TCP:[…]>` or `N<UNIX:[…]>`. A `connect` of a **UDP** socket to **port 0** is allowed, IPv4 or IPv6.
  2. Everything else stays a finding:
     - a TCP connect to a non-loopback address;
     - a UDP connect to any other port;
     - `htons(53)` (a name lookup);
     - `sendto`/`sendmsg`/`sendmmsg` to a non-loopback address.
  3. Each finding names its program: the checker maps pids to the last `execve` path, inherited through `clone`/`fork`/`vfork` results, and prints `<program> (pid N): <call>`.
- **Workflow:**
  - `sudo sed -i "/[[:space:]]$(hostname)\([[:space:]]\|\$\)/d" /etc/hosts && echo "127.0.1.1 $(hostname)" | sudo tee -a /etc/hosts`, then `test "$(getent ahosts "$(hostname)" | awk '{print $1}' | sort -u)" = "127.0.1.1"`.
  - Three controls, run under the same `strace -f -qq -yy`:
    - the TCP `192.0.2.1:9` connect, which must be caught;
    - a UDP `connect(("192.0.2.1", 0))`, which must pass;
    - a UDP `sendto(b"x", ("192.0.2.1", 9))`, which must be caught.

**Steps**

- [ ] **Step 1: Write the failing tests** (`unittest`, each feeding a few strace lines through the checker's function):
  - `test_tcp_connect_to_a_non_loopback_address_fails`;
  - `test_udp_connect_to_port_0_passes` (IPv4 and IPv6);
  - `test_udp_connect_to_another_port_fails`;
  - `test_a_name_lookup_fails` (`htons(53)`);
  - `test_sendto_a_non_loopback_address_fails`;
  - `test_loopback_and_unix_sockets_pass`;
  - `test_a_finding_names_its_program`: `100 execve("/usr/lib/jvm/bin/java", …)` then `100 connect(5<TCP:[1]>, … 192.0.2.1 …)` gives a finding that starts with `/usr/lib/jvm/bin/java (pid 100)`;
  - one test per rule the moved checker already enforces (writes outside the work folder, programs outside the pack, and so on), named after the rule.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `python3 -I scripts/check_strace_test.py`
  - Expected: FAIL, because `scripts/check_strace.py` is missing.

- [ ] **Step 3: Implement.**
  - Move the heredoc into the script. Its first commit is a pure move, and the old-rule tests pass on it.
  - Then add the rules above and change the workflow.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `python3 -I scripts/check_strace_test.py && python3 -I scripts/docs_test.py`
  - Expected: PASS.
  - Then run the workflow by hand on the branch: `gh workflow run reverse.yml --ref <branch>`. Expected: the Linux job is green, and its log shows the three controls behaving as above.

- [ ] **Step 5: Commit.**
```bash
git add scripts/check_strace.py scripts/check_strace_test.py .github/workflows scripts/docs_test.py ARCHITECTURE.md
git commit -m "Reverse engineering CI: the strace checker is a tested script; a UDP route check to port 0 is not a connection"
```

### Task F2: The Reverse engineering job on macOS: prepare rea's process helper before Kuna's probe

**Files:**
- Create: `crates/reverse/src/warmup.rs`
- Modify:
  - `crates/reverse/src/lib.rs` (module);
  - `crates/reverse/src/process.rs`: prepare before starting rea on macOS;
  - `crates/reverse/src/env.rs`: `REA_HELPER_MODULE` is an allowed variable, set only for the prepare run;
  - `ARCHITECTURE.md` §7.7: one sentence.
- Test: inline in `warmup.rs` and `process.rs`

**Interfaces:**
- Consumes: `RunConfig` and `run_config` (`env.rs:170`), `Flavor`, `Installed::rea_entry` (`install.rs:334`), the spawn helper that `process.rs` uses.
- Produces:
```rust
pub const HELPER_PREPARE_TIMEOUT: Duration = Duration::from_secs(180);
pub const PREPARE_SCRIPT: &str = "const m = await import(process.env.REA_HELPER_MODULE); await m.prepareOwnedCommand();";
pub fn helper_module(rea_entry: &Path) -> Option<PathBuf>;   // <rea_entry>/../../dist/process/OwnedCommand.js when it exists
pub fn prepare_config(config: &RunConfig, rea_entry: &Path, flavor: Flavor) -> Option<RunConfig>;
    // macOS only: program = config.program (node), args ["--input-type=module", "--eval", PREPARE_SCRIPT],
    // env = config.env + ("REA_HELPER_MODULE", file:// URL of helper_module), cwd = config.cwd; None elsewhere or without the module
#[derive(Debug, thiserror::Error)] pub enum WarmupError { #[error("timed out")] TimedOut, #[error("exited {code:?}")] Failed { code: Option<i32>, stderr_tail: Vec<String> } }
pub async fn prepare_process_helper(config: &RunConfig, rea_entry: &Path, flavor: Flavor) -> Result<(), WarmupError>;
```
- **The cause** (spec correction 12):
  - rea 6.9.0's Kuna probe (`dist/kuna/KunaInstallation.js:270`) runs `kuna --version` under a 10 s deadline (`:19`).
  - On a fresh `HOME`, its first owned command compiles the Darwin process helper (`dist/process/DarwinHelperCache.js`, cached under `$HOME/Library/Caches/rea/process-helper/darwin`). That compile has its own 120 s bound (`REA_SWIFTC_COMPILE_TIMEOUT_MS`), but here it runs inside the 10 s deadline.
  - rea's own `prepareOwnedCommand` (`dist/process/OwnedCommand.js:60-73`) exists to do this first: "Keep independently bounded macOS helper compilation outside a short command deadline". Ghidra's probe calls it, and Kuna's does not.
- **The fix.**
  - Before each rea start on macOS, `process.rs` runs `prepare_process_helper` with rea's own environment, so `HOME` and the cache are the pack's.
  - When the helper is cached, it returns quickly.
  - A failure or timeout is logged, and the start goes on. rea then reports Kuna exactly as before.
  - This also fixes a slow Mac, where users would see Kuna unavailable.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[test] fn the_prepare_run_imports_reas_helper_with_reas_environment() {
    let (config, entry) = macos_config_with_pack();     // a temp pack with rea-6.9.0/scripts/rea.mjs and dist/process/OwnedCommand.js
    let p = prepare_config(&config, &entry, Flavor::Macos).unwrap();
    assert_eq!(p.args, ["--input-type=module", "--eval", PREPARE_SCRIPT]);
    assert_eq!(env(&p, "HOME"), env(&config, "HOME"));
    assert!(env(&p, "REA_HELPER_MODULE").starts_with("file://") && env(&p, "REA_HELPER_MODULE").ends_with("/dist/process/OwnedCommand.js"));
}
#[test] fn no_prepare_off_macos_or_without_the_module() {
    let (config, entry) = macos_config_with_pack();
    assert!(prepare_config(&config, &entry, Flavor::Linux).is_none());
    assert!(prepare_config(&config, Path::new("/nope/rea.mjs"), Flavor::Macos).is_none());
}
#[tokio::test] async fn the_helper_is_prepared_before_the_first_start_on_macos() {     // process.rs, with the fake host
    let t = fake_host(Flavor::Macos); t.start().await.unwrap();
    assert_eq!(t.spawned_programs(), ["prepare", "rea"]);
}
#[tokio::test] async fn a_failed_prepare_does_not_stop_rea() { let t = fake_host_failing_prepare(Flavor::Macos); assert!(t.start().await.is_ok()); }
```

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p reverse --lib warmup:: process::`
  - Expected: FAIL to compile.

- [ ] **Step 3: Implement.** Spawn through the same helper as rea (`scripts/check-spawn.sh` stays clean), with `HELPER_PREPARE_TIMEOUT`.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p reverse && bash scripts/check-spawn.sh`
  - Expected: PASS.
  - Then: `gh workflow run reverse.yml --ref <branch>`. Expected: the macOS job is green, and `rea_with_kuna` passes.

- [ ] **Step 5: Commit.**
```bash
git add crates/reverse ARCHITECTURE.md
git commit -m "Reverse engineering: prepare rea's macOS process helper before Kuna's 10-second version probe"
```

### Task F3: The llama-server contract test's lost stderr tail

**Files:**
- Modify:
  - `crates/runtime/src/llama/server.rs` (`READER_DRAIN` at 55, `drain_readers` at 709–719, `read_stderr` at 546);
  - `crates/runtime/Cargo.toml`: `[[test]] name = "llama_tail"`, `harness = false`.
- Create: `crates/runtime/tests/llama_tail.rs`. It doubles as a fake `llama-server`: when its own file stem is `llama-server`, it acts as one.
- Test: inline in `server.rs`; `llama_tail.rs`

**Interfaces:**
- Produces (`server.rs`, private):
```rust
const READER_IDLE: Duration = Duration::from_secs(2);         // no new line from any reader for this long ends the wait
const READER_DRAIN_CAP: Duration = Duration::from_secs(10);   // the most the exit waits for its output
async fn drain_to_end(&mut self);   // replaces drain_readers: waits for both readers together until each reaches EOF, or READER_IDLE passes
                                    // with no new line, or READER_DRAIN_CAP passes; then aborts what is left
```
- The readers bump a shared `Arc<AtomicU64>` (milliseconds since start) on every line.
- `finish_after_exit` calls `drain_to_end` **before** it returns the tail and sends `Exited`. The order stays the same; only the drain changes (spec correction 13).

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[tokio::test(start_paused = true)] async fn the_drain_waits_while_lines_keep_coming() {
    // a stderr reader that writes its three last lines 1.5 s apart (4.5 s in all): the old 2 s cut lost them
    let mut s = server_with_readers(stdout_done(), stderr_lines_every(Duration::from_millis(1500), &["a", "b", "last"]));
    s.drain_to_end().await;
    assert_eq!(s.stderr_tail().last().map(String::as_str), Some("last"));
}
#[tokio::test(start_paused = true)] async fn the_drain_reads_both_readers_together() {
    // stdout never ends (a pipe some other process holds); stderr ends at once: drain returns after READER_IDLE, stderr complete
    let mut s = server_with_readers(stdout_never_ends(), stderr_lines_every(Duration::ZERO, &["x", "last"]));
    let t0 = Instant::now(); s.drain_to_end().await;
    assert!(t0.elapsed() <= READER_IDLE + Duration::from_millis(200));
    assert_eq!(s.stderr_tail().last().map(String::as_str), Some("last"));
}
```
`llama_tail.rs` (`harness = false`, a `main`):
- It copies itself to `<tmp>/llama-server` (`.exe` on Windows).
- Then 50 times, it calls `runtime::llama::server::start` with a `Backend` whose `server` is that copy, while a CPU-bound thread runs to load the machine.
- Each time, it asserts `StartError::Exited` with the tail's last line `fake llama-server: the last line`.
- As the fake: it writes 400 numbered lines to stderr in bursts of 50 with 5 ms pauses, then the last line, then exits 1.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p runtime --lib llama::server && cargo test -p runtime --test llama_tail`
  - Expected: FAIL to compile (`drain_to_end`); `llama_tail` reports at least one empty tail with the old drain on a loaded machine.

- [ ] **Step 3: Implement `drain_to_end`** and remove `READER_DRAIN`.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p runtime --lib llama:: && cargo test -p runtime --test llama_tail && cargo test -p runtime --test llama_server`
  - Expected: PASS. In CI, with `LLAMA_SERVER_BIN`, `a_broken_image_file_is_classified_and_the_model_loads_without_it` passes.

- [ ] **Step 5: Commit.**
```bash
git add crates/runtime
git commit -m "Runtime: read llama-server's last stderr lines to the end, both pipes together, before reporting its exit"
```

### Task F4: Stale docs from 0.1.x

**Files:**
- Modify:
  - `ARCHITECTURE.md`;
  - `SECURITY.md` (557–559, 648–650);
  - `README.md` (first sentence, 49, 58);
  - `CHANGELOG.md` (24, 49);
  - `docs/dev/0.1.0/follow-ups.md`: items 1–8 and 11 are marked "Done in 0.2.0"; items 9 and 10 are done in Task D3;
  - `scripts/docs_test.py`.
- Test: `scripts/docs_test.py`

**Interfaces:**
- What changes, item by item, from `follow-ups.md`:
  1. **ARCHITECTURE §4** gains an `sd-cli` row: one process per job, through the spawn helper. **§13.1** gains a Studio engine row, and its "Model files" row names `sd-cli` beside `llama-server`.
  2. **§13.6's Studio row** names `resume_interrupted` at launch: each cloud video still waiting or downloading is checked and fetched, with the endpoint's key.
  3. **The accuracy statements** match the code. "The engine is given only its model files, its prompt file and the run's folder" adds "and the pictures added to the run". The other statements the 0.1.0 review listed are checked against `local.rs::job_for` and `runtime::sd::args` and corrected.
  4. **The sidecar.** SECURITY.md, README.md and CHANGELOG.md say each run's `.json` holds its settings **and its prompt**, next to "the prompt is never written into a local record". That sentence is about the AI label and the engine's metadata, which the docs say.
  5. **The "signed" claims.** SECURITY.md (648–650) and ARCHITECTURE.md (around 352) say `sd-cli` is signed and notarized with the app *once the Apple Developer ID exists*, as README.md (73) does.
  6. **CHANGELOG.md line 49:** the internal sentence about the build machine is removed.
  7. **README.md:** the first sentence names Studio beside chat, tools, skills and MCP, and "Versions 0.0.x are early test builds" goes.
  8. **ARCHITECTURE §7.1, the VC++ bullet:** llama.cpp needs the first three DLLs; the Studio engine also needs `msvcp140_codecvt_ids.dll` and `vcomp140.dll`.
  11. **`docs_test`** checks local-before-cloud in the 0.1.0 entry's `### Added` section, not its opening paragraph.
- **§7.9** no longer describes 0.1.0's licence gate (removed in 0.1.1) or "Not supported yet" (removed in 0.1.2). It says what 0.1.2 does. Task D1 adds 0.2.0.

**Steps**

- [ ] **Step 1: Write the failing tests** in `docs_test.py`:
  - `test_section_4_lists_sd_cli`;
  - `test_13_1_has_a_studio_engine_row`;
  - `test_the_sidecar_is_said_to_hold_the_prompt`: SECURITY.md, README.md and CHANGELOG.md each contain "and its prompt" in the sidecar sentence;
  - `test_the_readme_first_sentence_names_studio`;
  - `test_7_9_has_no_0_1_0_licence_gate`: no "Not supported yet" in §7.9.
  - `test_the_changelog_tells_users_what_0_1_0_does` reads the `### Added` section.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `python3 -I scripts/docs_test.py`
  - Expected: FAIL on the new tests.

- [ ] **Step 3: Edit the documents** as listed.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `python3 -I scripts/docs_test.py && python3 -I scripts/release_notes_test.py`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add ARCHITECTURE.md SECURITY.md README.md CHANGELOG.md docs/dev/0.1.0/follow-ups.md scripts/docs_test.py
git commit -m "Docs: the 0.1.x follow-ups: process and trust tables, the sidecar, signing, README's first sentence"
```

## Phase D: docs, version and the end-to-end test

### Task D1: ARCHITECTURE, SECURITY and README for 0.2.0

**Files:**
- Modify: `ARCHITECTURE.md` (new §7.10, plus §7.9, §13.1, §13.6, §16, §17), `SECURITY.md`, `README.md`, `scripts/docs_test.py`
- Test: `scripts/docs_test.py`

**Interfaces:**
- **§7.10 "Any model the engine can run (0.2.0)":**
  - the vendored recipes (`catalog/sd-recipes.json`, `scripts/vendor_sd_recipes.py`, the commit test);
  - header reads: Range on the revision-pinned resolve URL, the guarded fetcher, **100 MiB**, **64 MiB** and **24 files**, the 512-entry cache;
  - the detection port: two passes, the not-ported per-family renames, the guards for Lumina 2, LTX-Video 0.9 and HunyuanVideo v1;
  - the matching table: 94 recipes in scope, and the reasons the other 18 are not; mask and control pictures as inputs;
  - the companion table in the signed catalog;
  - the set-up order;
  - import, copied or in place, never deleted outside the models folder;
  - *Set it up yourself* and the untested note;
  - LoRAs: the `@loras` folder, relative names, tags by prompt file, typed tags removed;
  - arguments from recipes, and the memory flags;
  - fit as a warning.
- **§13.6, the "Studio on this computer" row:** file headers are read with Range requests when a result opens, and the Popular list is read each time Get models opens (Task U5).
- **§13.1:** headers from Hugging Face and imported files are untrusted input, parsed with caps and never executed. Pickle files are never parsed.
- **§16/§17:** the detection fixtures and the capture script, which is not run in CI.
- **SECURITY.md:**
  - header reads and their caps;
  - imported files;
  - the LoRA folder;
  - the 1 MiB prompt bound;
  - spending defaults (Task G1) and approvals (Task G4), checked for consistency.
- **README.md:** Studio's paragraph says any Hugging Face picture or video model the Studio engine can run works, with LoRAs and *Add a model file*.

**Steps**

- [ ] **Step 1: Write the failing tests** in `docs_test.py`:
  - `test_7_10_names_the_caps_the_code_keeps`: `crates/models/src/sd/index.rs` matches `pub const SAFETENSORS_HEADER_CAP: u64 = 100 \* 1024 \* 1024;` and `pub const GGUF_HEADER_CAP: u64 = 64 \* 1024 \* 1024;`; `crates/models/src/sd/remote.rs` matches `pub const MAX_FILES_PER_REPO: usize = 24;`; §7.10 contains "100 MiB", "64 MiB" and "24 files".
  - `test_7_10_names_the_recipes_and_their_commit`: §7.10 contains `` `catalog/sd-recipes.json` `` and `` `532d2c1` ``, read from `sd.lock.json`'s tag.
  - `test_13_6_says_when_studio_reads_file_headers`: the row contains "Range" and "when a result opens".
  - `test_security_names_the_header_caps_and_the_prompt_bound`: "100 MiB" and "1 MiB".
  - `test_the_readme_says_any_model_the_engine_runs`: README's Studio paragraph contains "LoRA" and "Hugging Face".

- [ ] **Step 2: Run them and see them fail.**
  - Run: `python3 -I scripts/docs_test.py`
  - Expected: FAIL.

- [ ] **Step 3: Write the sections.**

- [ ] **Step 4: Run them and see them pass.**
  - Run: `python3 -I scripts/docs_test.py`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add ARCHITECTURE.md SECURITY.md README.md scripts/docs_test.py
git commit -m "Docs: 0.2.0: any model the Studio engine can run, LoRAs, header reads and fewer blocks"
```

### Task D2: The 0.2.0 changelog entry and the version bump

**Files:**
- Modify:
  - `CHANGELOG.md`;
  - `Cargo.toml` (`version = "0.1.2"` at 20 → `"0.2.0"`) and `Cargo.lock` (by `cargo build`);
  - `package.json` (4);
  - `package-lock.json` (3 and 9);
  - `src-tauri/tauri.conf.json` (4);
  - `src-tauri/src/commands.rs` (228: `assert_eq!(v, "0.2.0")`);
  - `scripts/docs_test.py`.
- Test: `the_version_is_the_same_everywhere` (`commands.rs:225-238`); `scripts/release_notes_test.py`; `docs_test.py`

**Interfaces:**
- **`## 0.2.0 (YYYY-MM-DD)`**, dated the day the task is done. The owner re-dates it at tagging, as with 0.1.0.
- It follows 0.1.0's house style:
  - an opening paragraph quoting the owner's words from the top of the design ("I was mostly talking about how nothing from huggingface for video and image models is supported", and "Work on fixing the app and removing ur set restrictions due to it being very not user friendly");
  - then `### Added`, `### Changed`, `### Fixed` and `### Known limits`, each a bold lead-in and a few plain sentences.
- **Added:**
  - any Hugging Face picture or video model the Studio engine can run, with "Works in Studio" before downloading and one Download for the model and its extra files;
  - *Add a model file*;
  - LoRAs;
  - *Set it up yourself*;
  - the model type and the full sampler and scheduler lists;
  - inpainting and Fill models with a mask picture, and FLUX.1 Canny, Depth and Flex.2 with a control picture;
  - OpenRAIL licences shown as "Commercial use allowed, with use rules".
- **Changed:**
  - spending guards are off by default, as switches;
  - "May not fit this computer" is a warning;
  - video on the processor is offered;
  - no prompt length limits;
  - Create finishes a pending price;
  - installing an update stops a local job;
  - chat mode switches stop asking, and a saved start mode is not asked again;
  - plus Phase U's redesign, described in user terms:
    - one composer with the model chip;
    - the model sheet;
    - Get models as a full view with search, verdicts and a detail panel;
    - Pause, Cancel and *Use it* on downloads;
    - bigger pictures.
- **Fixed:** the scheduled Reverse engineering test (Linux and macOS), and Kuna on a slow Mac.
- **Known limits:**
  - models the engine cannot run (Lumina 2, LTX-Video 0.9, HunyuanVideo v1, HiDream-I1, SVD), with "Can't run in Studio";
  - ControlNet, IP-Adapter, PhotoMaker and other adapter files, upscalers and audio-driven video are not offered;
  - masks are added as black-and-white pictures; painting a mask in the app comes later;
  - pickle files are not read;
  - split (sharded) files can't run.
- No internal notes: no task ids, file names or CI details.

**Steps**

- [ ] **Step 1: Write the failing tests.**
  - `commands.rs`: change the expected version to "0.2.0".
  - `docs_test.py`: `test_the_changelog_tells_users_what_0_2_0_does` asserts that the `0.2.0 (` entry contains "Hugging Face", "LoRA", "Works in Studio", "off by default", "### Added", "### Changed" and "### Known limits", and none of "Task ", ".rs" or "CI".

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p app --lib the_version_is_the_same_everywhere; python3 -I scripts/docs_test.py`
  - Expected: FAIL.

- [ ] **Step 3: Bump the five version places, run `cargo build`, and write the entry.**

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test -p app --lib the_version_is_the_same_everywhere && python3 -I scripts/docs_test.py && python3 -I scripts/release_notes_test.py && python3 -I scripts/release_notes.py app 0.2.0`
  - Expected: PASS, and the release notes print the 0.2.0 entry.

- [ ] **Step 5: Commit.**
```bash
git add CHANGELOG.md Cargo.toml Cargo.lock package.json package-lock.json src-tauri/tauri.conf.json src-tauri/src/commands.rs scripts/docs_test.py
git commit -m "Release 0.2.0: changelog and version"
```

### Task D3: The end-to-end Studio script with a generic model, and follow-ups 9 and 10

**Files:**
- Modify:
  - `e2e/fake_sd/sd-cli`: `FLAGS` becomes the full list of `runtime::sd::args::FLAGS`, and lines split only on `\n`, keeping line ends (follow-up 9);
  - `e2e/m8_offline.py`: `install_generic`; `SETTINGS_PAGES` gains "Search"; `install_klein` fixes (follow-up 9);
  - `e2e/webdriver.py`: `settings_tab`, `close_settings` and `studio_tab` move here from m8 and m12;
  - `e2e/m12_studio.py`: imports them;
  - `src-tauri/src/studio/wire_tests.rs`: the mock records and checks `Authorization`, with a comment on `WireHarness`'s field drop order (follow-up 10);
  - `crates/runtime/src/sd/args.rs`: a test that the e2e fake knows every flag;
  - `docs/dev/0.1.0/follow-ups.md`: items 9 and 10 done.
- Test: `the_e2e_fake_engine_knows_every_flag` (`args.rs`); `wire_tests.rs`; the e2e script, run by hand as now ("not in CI", `docs_test`'s m8 sentence)

**Interfaces:**
- **`install_generic(con, models) -> str`** inserts:
  - a `gen_models` row: id `local:e2e0000000000000`, recipe `sdxl-checkpoint`, family `sdxl`, kind `image`, source `import`;
  - one `model` part: a tiny `.safetensors` whose header holds `model.diffusion_model.input_blocks.0.0.weight`, `conditioner.embedders.1.model.token_embedding.weight` and `cond_stage_model.transformer.text_model.embeddings.token_embedding.weight` (shape `[49408, 768]`);
  - a `gen_loras` row `local/e2e style ü` with its file under `<models>/@loras/local/`.
- **The new scenario `c_generic_model_with_a_lora`:**
  1. Pick the model in the model switcher, attach the LoRA at strength 0.8, and press Create.
  2. Assert that the fake engine's recorded arguments hold `-m <part file>`, `--lora-model-dir <models>/@loras`, `--steps 20` and `--cfg-scale 5`, and no `--diffusion-model`.
  3. Assert that the recorded prompt file ends with ` <lora:local/e2e style ü:0.8>`.

**Steps**

- [ ] **Step 1: Write the failing tests.**
```rust
#[test] fn the_e2e_fake_engine_knows_every_flag() {
    let fake = include_str!("../../../../e2e/fake_sd/sd-cli");
    for f in FLAGS { assert!(fake.contains(&format!("\"{f}\"")), "{f}"); }
}
```
In `wire_tests.rs`, `the_key_reaches_the_endpoint_in_the_authorization_header`: the mock records `Authorization: Bearer <key>` for the create request.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p runtime --lib sd::args::tests::the_e2e_fake_engine_knows_every_flag && cargo test -p app --lib studio::wire_tests`
  - Expected: FAIL.

- [ ] **Step 3: Implement**, and run the e2e scripts by hand as their headers describe.
  - Run: `python3 -I e2e/m8_offline.py` (with the built app and its driver, as before)
  - Expected: all scenarios pass, including `c_generic_model_with_a_lora`.

- [ ] **Step 4: Run them and see them pass.**
  - Run: `cargo test --workspace && python3 -I scripts/docs_test.py`
  - Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add e2e crates/runtime/src/sd/args.rs src-tauri/src/studio/wire_tests.rs docs/dev/0.1.0/follow-ups.md
git commit -m "e2e: a generic SDXL model with a LoRA through the fake engine; shared webdriver helpers; the key reaches its endpoint"
```

## Spec coverage

| Design section | Tasks |
|---|---|
| 1–4 (why, goals, non-goals, anti-goals) | All; the anti-goals are pinned by S7 (nothing unlisted downloads; no "works" without a runnable plan), S1, S2 and S11 (upgrade), and the Global Constraints |
| 5 Constraints | R1 (commit test), R2 and R3 (caps), S7 (pinned downloads), S10 (LoRA names), Global Constraints |
| 6 Approaches | A: Phase R and S; B: S9 |
| 7.1 Recipes | R1, R7 |
| 7.2 Reading a file | R2, R4, R5, R6 |
| 7.3 Before downloading | R3, S5, S6, U5 and U6 (labels on cards and in the detail panel) |
| 7.4 Setting up | R8, S4, S6, S7, S1 (storage) |
| 7.5 Set it up yourself | S9 (backend), U8 (Files) |
| 7.6 Running | S2, S11 |
| 7.7 Fit and time | S3, S11 |
| 7.8 Import | S8 (backend), U4 and U5 (*Add a model file* in the model sheet and Get models) |
| 8 LoRAs | R6 (detection), S1 (store), S10, S11, S12, U8 (the LoRAs section) |
| 9.1 | G1 |
| 9.2 | S11 (backend), S12 (wording), G2, G3 |
| 9.3 | G4 |
| 10.0–10.1 (layout, composer, top bar) | U2, U3 |
| 10.2 Switching models | U4 |
| 10.3 Getting models (view, detail panel, downloads, the 4-click target) | U5, U6, U7 |
| 10.4 Results and polish (controls, focus, type scale, surfaces, empty states, narrow window) | U2, U8, U9 |
| 10.5 How the look is checked | U1 (harness), every U task's screenshot step, U9 (strict run and pairs) |
| 11a OpenRAIL licences | S6 |
| 11a Mask and control pictures | R7, S2, S3, S11, S12, U3 |
| 11 | F1, F2, F3, F4 |
| 12 Left to the builder | R5 module layout and "Plan decision:" notes throughout; R7 matching table; R8 defaults; S3 memory factors |
