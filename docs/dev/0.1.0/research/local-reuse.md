# Local image and video generation in Studio: how it reuses harnessUI's local runtime

I only read files; nothing was edited. Repo root: /home/user/harnessUI. Upstream facts were checked on 2026-11-17 from stable-diffusion.cpp's release page, README and server `api.md`. The GitHub API is blocked in this session, so I read the release asset list as a web page.

## 0. Summary

- **Engine.** Use **stable-diffusion.cpp** (leejet, MIT license). It is the ggml sibling of llama.cpp. Its latest build is `master-1106-532d2c1` (2026-11-16). It runs SD1.x/SDXL, SD3.5, FLUX.1/FLUX.2, Chroma, Qwen-Image and Edit, and Z-Image. For video it runs Wan2.1/2.2, LTX-2.x and HunyuanVideo 1.5. Backends: CPU, Vulkan, Metal, CUDA, HIP. It reads GGUF and safetensors.
- **How to ship it:**
  - **Windows:** a one-click **"Studio engine" pack**, about 29 MB (Vulkan+CPU). An optional CUDA pack is about 930 MB.
  - **macOS:** build it from source in CI and bundle it, the same way as `llama-server`: universal, static, embedded Metal, an `externalBin`. It is notarized with the app. The upstream Mac zip cannot be used.
- **How it runs.** One **`sd-cli` process per job**, not a long-lived server, for 0.1.0. The upstream `sd-server` has a good async job API, but `api.md` documents **no API key or auth**, which breaks the model llama-server follows (random key, `--cors-origins none`).
- **Catalog.** Picture and video models go in a **new top-level `generators` key** in `catalog/models.json`. They cannot go into `models`. Old apps (0.0.x) download the latest release's catalog, and their validator rejects any file that does not end in `.gguf`.
- **Installed list.** Generation models get their **own store table**, not the `models` table. Registering a model there requires a GGUF header (`hub.rs:836`), and every row in that table feeds the chat picker.
- **Fit.** Reuse `models::fit::Room`, `SPARE_BYTES` and `RAM_RESERVE_BYTES`, and the existing VRAM probe from `llama-server --list-devices`. Peak memory is the largest of the text-encoder, diffusion and VAE stages. Activations grow with latent tokens, which depend on resolution and frames. Prefer peak figures measured per catalog entry.
- **GPU sharing.** One graphics-card job at a time. If the job does not fit next to the loaded chat model, use the existing D3 path: `before_change` then `unload_by_user`. That already shows "Unloaded to free memory · Load". A chat load waits or asks while a local job holds the card.

## 1. What exists today and can be reused

**Pinning and fetching: `scripts/fetch_llama.py` and `llama.lock.json`**
- `download()` checks size and SHA-256 and allows only the `GITHUB` and `LUNARG` URL prefixes.
- `windows()` unpacks the Vulkan zip, filtered by the **global** `keep_files`, into `src-tauri/resources/llama/main/`. It adds `vulkan/vulkan-1.dll` and the VC++ DLLs.
- `macos()` builds both CMake slices from the pinned commit. `macos_plan` requires `BUILD_SHARED_LIBS=OFF`, `GGML_NATIVE=OFF`, deployment target 13.3 and one arch per slice. It then runs `lipo` and `check_macos_binary`.
- `check_macos_app` (line ~386) checks **every** program in `Contents/MacOS`: both archs, system libraries only, at least 3 programs, and the llama targets present. A new sidecar is therefore covered automatically.
- `verify_packs` (line 426) sends a HEAD request for each pack URL.

**Backend packs**
- `crates/runtime/src/packs.rs`:
  - `ALLOWED_HOSTS` (line 22) lists GitHub's release hosts.
  - `download_verified(f, dest, hosts, cancel, on_bytes)` (line 195, `pub`) and `unpack_zip(archive, out, keep, keep_all)` (line 391, `pub`) are generic.
  - `install()` (line 467) is tied to llama: it takes `LlamaLock.keep_files` and the VC++ list. It does resume with `Range`, writes a `.complete` marker and renames `.tmp-<dir>` into place.
- `crates/runtime/src/llama/lock.rs`: `PackSpec` and `PackFile` are reusable types. The lock is compiled in with `include_str!`.
- `src-tauri/src/packs_cmd.rs` holds the commands `list_packs`, `download_pack(id, accept_license)`, `cancel_pack_download` and `remove_pack`. It emits a `pack-progress` event, throttled to 300 ms.
- `src-tauri/src/gpu.rs::pack_availability` decides which cards can use a pack.
- `src/components/BackendDownloads.tsx` is the UI: a license AlertDialog, progress, Cancel, Remove and Resume.
- **Do not put the engine in `llama.lock.json` `packs`.** `list_packs` would show it under the llama Backend downloads, `install` would filter its files with llama's `keep_files`, and `discover_with` (`backend.rs` ~line 244) expects `llama-server`.

**The runtime crate**
- `spawn.rs` is the only allowed way to start a process; `scripts/check-spawn.sh` and clippy enforce it. It provides a Job Object or process group, kill-tree, a scrubbed `EnvSpec::minimal()` and no console window.
- `llama/fit_params.rs::measure` (line 360) is the right template for a **one-shot** child: `SpawnSpec` with the backend folder as working directory, stdin set to null, stdout and stderr piped, a timeout and a `CancellationToken`.
- `llama/server.rs::start` and `server/supervisor.rs` are long-lived server machinery (health, `/props`, key, ports, restarts, step-down). A one-shot engine does not need it.
- `llama/backend.rs::base_env` (line 159) adds `GGML_NO_BACKTRACE=1` and the Vulkan fallback on `PATH`. It only looks for `<dir>/vulkan/vulkan-1.dll`. An engine folder under local data therefore needs `path_with_vulkan_fallback` (line 125) pointed at the installed `resources/llama/main/vulkan`.
- `llama/lines.rs::next_line` (line 29) splits only on `\n`. sd.cpp's progress bar redraws with `\r`, so this reader would see one very long line, cut at 16 KiB.
- `llama/devices.rs` probes devices. sd.cpp has its own `--list-devices` with lower-case names (`vulkan0`).

**Models crate**
- `hardware.rs` reads system RAM.
- `fit.rs` has `Target`, `Room::{graphics_card, apple, processor}`, `Room::verdict` and `SPARE_BYTES` (1 GB), all public. `RAM_RESERVE_BYTES` is 2 GB.
- `src-tauri/src/hub.rs::detect` (line ~125) builds the `Target` from the llama probe: Metal working set, Vulkan, CUDA, ROCm, otherwise CPU.
- `gguf.rs` reads any GGUF header and tensor table. Diffusion GGUFs carry `general.architecture` values such as `flux` or `wan`. `arch.rs` with `catalog/llama-architectures.txt` already marks those as unsupported by llama.
- `files.rs::quant_label` works on diffusion GGUF names, but `classify` drops every file that is not `.gguf`.
- `download.rs::download_file_checked` is generic over `FileSpec {repo, commit, path, size, sha256}`, so safetensors files work. A file already at its final name with the right size and hash is checked, not downloaded again, which makes shared text encoders free.
- `hf.rs::search` hard-codes `filter=gguf` plus `conversational` (line 763).

**Catalog**
- `catalog.rs::validate`, line 294, requires every file to be `.gguf` with a SHA-256. It requires `schema_version == 1`.
- `Catalog` has no `deny_unknown_fields`, so old parsers ignore a new top-level key.
- `refresh_catalog` (`hub.rs` ~line 1605) downloads `models.json` and `models.json.minisig` from `releases/latest/download`. It checks them against the updater key and requires a higher sequence number.

**Downloads and store**
- `src-tauri/src/downloads.rs`: `GroupMeta` (`model_files`, `projector`, serde-default extras). `register_model` fails with "finished download has no model file" (line 1297) and calls `hub::register_installed`, which needs a GGUF header.
- `crates/store/src/schema.rs`: the `models` table (path UNIQUE, `gguf_summary`). `MIGRATIONS` is append-only (line 414). The cloud Studio design already adds V3 there.

**Runtime manager**
- `src-tauri/src/runtime_manager.rs`:
  - `loaded_on_device()` (line 227, private) gives the backend, device and bytes of the loaded model.
  - `freed_memory()`.
  - `before_change(busy, when_idle)` (line 316) is the D3 wait-or-stop step.
  - `unload_by_user()` (line 365) sets `unloaded_path`, which shows "Unloaded to free memory · Load".
  - `models_cmd.rs::unload_model` (line 185) shows how they combine.

**Models dialog**
- `src/components/ModelsDialog.tsx` has four tabs: Installed, Browse, Compare, Downloads.
- `Installed.tsx` greys out models whose architecture is unsupported (`architectureSupported === false`).
- `Browse.tsx` searches GGUF repositories.

**CI and notices**
- `.github/workflows/ci.yml` line 260 fails a Windows installer over 150 MB. macOS runs `check-macos-app`. The cache key hashes `llama.lock.json` and `fetch_llama.py`.
- `scripts/notices.py` hard-codes the "Bundled native components" list and the "Optional GPU backends … not part of the installer" sentence. Its `--check` runs in CI.
- `reverse.lock.json` and `crates/reverse` show the more general pinned-download pattern: per-platform builds, other hosts, a disk-space check (`diskspace.rs`) and `verified_by`.

## 2. Proposal

### (1) Inside the installer or a "Studio engine" pack: sizes

stable-diffusion.cpp assets for `master-1106-532d2c1` (2026-11-16):

| Asset | Size | Use |
|---|---|---|
| `sd-master-532d2c1-bin-win-vulkan-x64.zip` | **28.9 MB** | **Windows Studio engine pack (default)**: Vulkan+CPU, every GPU vendor |
| `sd-master-532d2c1-bin-win-cpu-x64.zip` | 17.5 MB | Not needed; the Vulkan build also has CPU |
| `sd-master-532d2c1-bin-win-cuda12-x64.zip` + `cudart-sd-bin-win-cu12-x64.zip` | 393 MB + 537 MB = **about 930 MB** | Optional "Studio engine: NVIDIA CUDA" pack (0.1.x). It cannot share llama's CUDA 13.4 runtime (424 MB). |
| `sd-master-532d2c1-bin-win-rocm-10.1.0-x64.zip` | 270 MB | Later |
| `sd-master-532d2c1-bin-Darwin-macOS-26.7.2-arm64.zip` | 36.6 MB | **Not usable**: arm64 only, built on macOS 26.7.2, so it fails the 13.3 minimum, the universal rule and likely the system-libraries-only rule |
| `sd-master-532d2c1-bin-Linux-Ubuntu-26.04-x86_64.zip` | 27.8 MB | Development and CI contract tests (`dev` entry) |

Recommendation:

**Windows: a pack, not bundled.**
- Bundling would add an estimated 25–30 MB to the NSIS installer for every user. Today's installer size is not recorded in the repo; CI prints it. Most users never use local generation.
- Local models are 4–20 GB downloads anyway, so offer one button: "Download model and engine (+29 MB)".
- This keeps the 150 MB cap untouched and matches how the CUDA, ROCm and reverse-engineering downloads work.

**macOS: bundled.**
- Add `sd-cli` to `macos-aarch64` and `macos-x86_64` `build_from_source` entries in a new lock file. Build both slices with static ggml, embedded Metal and target 13.3, join them with `lipo`, and add `binaries/sd-cli` to `externalBin` in `src-tauri/tauri.macos.conf.json`.
- Tauri then signs and notarizes it. `check_macos_app` already checks every program in `Contents/MacOS`, so it catches a Homebrew `libwebp`/`libwebm` or `@rpath` link. Add `sd-cli` to the required-targets list there.
- Estimated cost: about 30–40 MB more in the universal app, roughly 15–20 MB in the .dmg. This is an estimate; measure it in CI. The Mac has no size cap.
- Alternative: a notarized pack built by `release.yml` and hosted on harnessUI's own GitHub release. This works with the existing GitHub host list, but adds release plumbing.

**Lock file: a separate `sd.lock.json`, compiled in.**
- Contents: the upstream `tag` and `commit`, the Windows `packs` (id `studio-vulkan`, later `studio-cuda`), their own `keep_files` (`sd-cli.exe`, `stable-diffusion*.dll`, `ggml*.dll`, licenses), `macos-*` `build_from_source` with target `sd-cli`, and a `dev.linux-x86_64` entry.
- Install to `<local data>/engines/sd-<build>/`. **Never install into `llama/main`.** The sd.cpp zip's `ggml*.dll` files are a different ggml version from llama's, and llama.cpp loads backend DLLs from its own folder. Verify the zip's contents at pin time.
- Generalize `runtime::packs::install` into `install_files(dir, files, keep, keep_all, vc_runtime)` so both lock files use it. Copy the VC++ DLLs from the installed llama folder, as packs already do.

### (2) How the engine process runs, reports progress and is cancelled

**0.1.0: one `sd-cli` process per job.** It goes in a new `crates/runtime/src/sd/` module, modelled on `fit_params::measure`.
- **Spawn.** Use `runtime::spawn` with the engine folder as working directory. The environment is `EnvSpec::minimal()` plus `GGML_NO_BACKTRACE=1`, plus `PATH` with llama's `main/vulkan` folder last.
- **Arguments.** stdin is null; stdout and stderr are piped. Arguments come only from typed fields:
  - Each catalog part role maps to one fixed flag: `--diffusion-model`, `--llm`, `--t5xxl`, `--clip_l`, `--vae`, `--clip_vision`, `--high-noise-diffusion-model`, or `-m` for a single-file model.
  - Then numbers: steps, cfg, width, height, `--video-frames`, `--fps`.
  - Then fixed words: sampler and scheduler from a whitelist, `--backend <probed device>`, `--vae-tiling`, `--offload-to-cpu` or `--params-backend` for fallback, `--seed <app-generated>` and `-o <gallery>/.partial/<run>/…`.
  - Verify every flag name against the pinned `sd-cli -h`; the current README defers the full list to `-h`.
- **Progress.** A typed adapter (`sd/progress.rs`) needs a line reader that splits on `\r` as well as `\n`, so `lines.rs` needs a variant. It turns `|====>  | k/N - x s/it` into a step count, and log lines into stages: loading, encoding text, sampling (one pass, or two for Wan2.2 A14B's high and low noise), VAE decode, saving.
  - Map these onto one 0–1 value: load 0–10 %, text encoder 10–15 %, sampling 15–90 %, decode 90–99 %.
  - Emit it through the Studio design's `studio-progress` (`JobState::Running{progress}`).
  - Cover it with contract tests on fixtures captured from the pinned build. Treat it as "no interface guarantee", like ARCHITECTURE 7.1.
- **Cancel.** The `CancellationToken` triggers `ManagedChild::start_kill()`. The kill is immediate and takes the whole tree. Delete `.partial/<run>` and mark the run `cancelled`; no charge wording.
  - Esc and the global stop hotkey still never cancel Studio jobs, as the design says.
- **Failure.** A `sd/failure.rs` classifier sorts out of memory, Vulkan device lost, missing DLL, and an unreadable or wrong model file.
  - Like the llama supervisor's "As much as fits", retry **once** with `--offload-to-cpu --vae-tiling` after a graphics-card out-of-memory failure.
  - No restart policy: one job, one process.
- **Resume after a restart.** Local jobs cannot survive an app exit; the Job Object or reaper kills them. `resume_interrupted` marks them `interrupted` with "Run again", which costs nothing.
- **Prompt in argv.** Unlike llama, the prompt text reaches the command line. It is visible to processes of the same user, and Windows caps a command line at 32,767 characters.
  - Cap the prompt length.
  - Check whether the pinned build reads a prompt from a file or stdin; I found none documented.
  - Use the `--prompt=<text>` form, if the build accepts it, so a prompt starting with `-` is never read as a flag.
- **Later (0.1.x): long-lived `sd-server`, behind the same trait.** It has `POST /sdcpp/v1/img_gen` and `vid_gen` returning 202 with an id, `GET /sdcpp/v1/jobs/{id}` with `progress{stage,completed,total,unit}` and previews, `POST /jobs/{id}/cancel` (cooperative), and `models/load`/`unload`/`status`.
  - It keeps the model loaded between jobs, saving roughly 5–60 s of reload per image.
  - Adopt it only when it can require a key (an upstream flag, or our own build on Windows too) and refuse foreign `Origin`/`Host`. Its built-in Web UI would also have to stay off.
- **Studio integration.**
  - Implement the design's `agent::media::MediaProvider` as `LocalSdProvider` in `src-tauri/src/studio/local.rs`. `start` returns `Started::Job{local id}`; `check` reads the in-memory progress.
  - Add `Price::Free` to `crates/agent/src/media/price.rs`.
  - `StudioManager` runs at most **one local job at a time**, separate from the per-endpoint cloud queue. Confirmation and the $10 daily limit are skipped for $0 runs. The backend's `studio_needs_confirmation` rule must treat local as free.
  - Use `-b <count>` (batch count, to verify) so "How many" loads the model once.
- **Video output.** Ask for WebM, which needs a WebM-enabled build: vendor libwebm on macOS, and verify the Windows zip. The fallback is animated WebP. **Not AVI**: it is outside the design's allowed list and does not play in WebView2 or WebKit. No MP4 and no ffmpeg, keeping the `--video-ffmpeg-dir no-ffmpeg` principle.

### (3) How picture and video models appear in the model manager

**Catalog.** Add `"generators": { id: GenEntry }` and `"generatorTiers": { "8": [...], "16": [...] }` to `catalog/models.json`. Keep `schemaVersion: 1` and bump `sequence`. The minisign signature covers the whole file, so the existing refresh carries these entries with no other change. `GenEntry`:
- `name`, `kind` (`image` | `image-edit` | `video`), `family` from a compiled-in whitelist (`sd1`, `sdxl`, `flux`, `z-image`, `wan2.1`, `wan2.2-ti2v`, …), and `inputs` (`startFrame`, `endFrame`, `reference`).
- `parts: [{role, repo, revision(40-hex), path, size, sha256}]`. Each part has its **own repo**, because VAEs and text encoders usually come from other repos.
- Allowed extensions are **`.gguf` and `.safetensors` only**. Never `.ckpt`, `.pt` or `.pth`, which are pickle files that can run code.
- `defaults` (steps, cfg, sampler, size, frames, fps), `params` (the same `ParamSpec` kinds as the cloud adapters: choice, range, on/off, so `ParamField.tsx` renders them as-is) and `limits` (frame rule 4n+1 for Wan, 8n+1 for LTX).
- `memory: [{width, height, frames, peakBytes, device, build}]`, measured.
- `license` and `gated`, `engine.minBuild` (entries that need a newer engine are hidden), and `tested`.
- Parse it in `crates/models/src/catalog.rs` with its own validation. An unknown role or family hides the entry rather than failing the whole catalog. Add a fixture and tests.

**Store.**
- A new table `gen_models(id, catalog_id, name, kind, family, parts JSON{role→path,sha256,size}, size_bytes, added_at, source)`.
- Put it in the **same V3 step** as the cloud Studio tables, or V4 if V3 has already landed; migrations are append-only.
- The chat picker and `register_installed` stay untouched.

**Downloads.**
- Add `GroupMeta.generator: Option<{catalog_id, parts:[{role,file}]}>`, serde-default.
- `register_model` branches to `register_gen_model` before the "no model file" error.
- Files land at `<models>/<owner>/<repo>/<path>` (`download::destination`), so a text encoder shared by two entries downloads once.
- Delete must count uses across `gen_models` and `models`. Installed already has `keptInUse` wording.
- Show the license before download, as the pack dialog does. FLUX.1-dev, SDXL-Turbo, SD3.5 and some others are non-commercial or gated. Gated repos use the existing Hugging Face token and accept-terms errors.

**Where the models appear (UI).**
- Studio owns them:
  - Studio's ModelPicker gets a **"This computer"** group with a "Free · private · runs here" mark in place of the Cloud mark.
  - A **"Get local models"** sheet lists the catalog generators by Pictures or Video, with fit dot, size, license, Download, and engine status.
  - Installed generation models get Delete.
- Models → Downloads shows these groups too (the same DownloadManager).
- Models → Installed gets one line: "Picture and video models: N (X GB), manage in Studio". Disk use stays honest, and "Use" can never load a diffusion model into llama.
- Importing a diffusion GGUF in Installed (architecture `flux`, `wan`, …) should say "This is a picture/video model. Add it in Studio".
- Free-form Hugging Face search for diffusion models is **0.1.x**. Repos are split across files and need matching encoders and VAE; a later "Add from Hugging Face" could match a diffusion GGUF's family to the catalog's shared encoder and VAE parts.
- Model badges: reuse `gguf::read_file` to confirm the diffusion file's `general.architecture` matches the declared family before a run. Reuse `quant_label` for the Q4/Q8 badge.

### (4) Fit verdicts for generation models

Implement this in a new `crates/models/src/gen_fit.rs`. It reuses `fit::Room`, `SPARE_BYTES`, `RAM_RESERVE_BYTES` and the `Target` from `hub::detect`.
- **Weights.** D is the diffusion model (plus D2, the second Wan A14B expert, used one after the other). T is the sum of the text encoders. V is the VAE.
- **Tokens.**
  - Images: tokens = ⌈W/16⌉·⌈H/16⌉ (8× VAE, 2×2 patch), with family-specific factors.
  - Video: multiply by latent frames: (F−1)/4+1 for Wan, (F−1)/8+1 for LTX. Wan2.2-TI2V's 16× VAE gives (W/32)(H/32).
- **Activations.**
  - Diffusion: act_dit ≈ a_f + b_f·tokens. This is linear with flash attention, which sd.cpp uses on `auto`.
  - VAE decode: act_vae ≈ c_f·W·H·frames, or bounded by the tile size with `--vae-tiling`.
  - **Prefer the measured catalog points** and interpolate in tokens. Fall back to conservative per-family coefficients, like `UNMEASURED_ALLOWANCE_BYTES` does for chat models.
- **Verdicts:**
  - **Green**: D+T+V + max(act_dit, act_vae) + SPARE ≤ `room.whole`. Everything stays on the card.
  - **Yellow, one part at a time**: max(T, D+act_dit, V+act_vae) + SPARE ≤ `whole`, and the rest fits the computer's memory less the reserve. The engine gets `--params-backend te=cpu…` or `--offload-to-cpu`; it is slower.
  - **Yellow, slow**: D+act_dit ≤ `room.partial`. Part of the diffusion model runs on the CPU (sd.cpp `--auto-fit on` or `--max-vram`).
  - **Red** otherwise.
- **Suggestion**, like the yellow `suggested_ctx`: the largest preset that is green, from 512², 768² and 1024², and for video 17, 33, 49, 81 or 121 frames. For example: "Fits at 832×480, 33 frames".
- **Speed mark**, separate from memory. CPU-only gives "pictures take minutes" and "video not recommended". An Intel Mac counts as CPU (Auto already prefers the CPU there).
- **With a chat model loaded**, compute twice: with the probe's free memory as it is, and with `RuntimeManager::freed_memory()` / `loaded_on_device()` added back. The second gives the note "Fits after the chat model is unloaded".
- `scripts/studio_catalog_test.py` (new, like `catalog_test.py`) runs the pinned `sd-cli` on each entry at two or three points and records `memory[]` and `tested`.

### (5) A local generation while a chat model is loaded

- **New file `src-tauri/src/gpu_lease.rs`.** One graphics-card tenant at a time: either the chat server or a Studio local job.
- **Starting a job:**
  - If the job's green or yellow figure fits **alongside** the chat model's `loaded_on_device()` bytes, run both. Warn that chat replies may slow down.
  - Otherwise follow Settings → Studio's choice: "Unload the chat model" (default), "Ask" or "Use the processor for local pictures".
  - Unloading is `runtime.before_change(&model_busy, when_idle=true)`. It waits for a running reply, says "Starts when the reply ends · Cancel", and stops the sharing proxy's users the D3 way. Then `runtime.unload_by_user()`, which sets "Unloaded to free memory · Load". Then the job runs.
- **During a local job.** A chat-model load, from a chat send or Load, waits with "Studio is using the graphics card. Chat loads when it finishes · Stop Studio job", or asks. This stops llama-server from running out of memory mid-chat and its supervisor looping on restarts.
- **After the queue empties.** Leave the chat model unloaded, with the existing "Load" chip. Automatic reload is an option ("reload it after Studio" checkbox) but costs time.
- **Unified memory (Apple).** Unloading frees the same pool, and the same rules apply with `Room::apple`. `keep_in_memory` (mlock) always forces an unload.
- **CPU-only machines.** The contest is over RAM and cores. Run with physical cores minus one, and warn that chat will be slow.
- A `BusyKind` value is not right here: it counts users of the llama model, not graphics-card tenants. Add `pub fn device_bytes()`, wrapping the private `loaded_on_device`, to `RuntimeManager`.

### (6) Files that change

**New**
- `/home/user/harnessUI/sd.lock.json`.
- `/home/user/harnessUI/crates/runtime/src/sd/{mod.rs, lock.rs, engine.rs (discover installed, pack or dev), devices.rs (sd-cli --list-devices, matched to llama's devices by description, because index and order may differ), args.rs, progress.rs, failure.rs, run.rs}`, exported from `crates/runtime/src/lib.rs`.
- `/home/user/harnessUI/crates/models/src/gen_fit.rs`.
- `/home/user/harnessUI/crates/store/src/gen_models.rs`.
- `/home/user/harnessUI/src-tauri/src/studio/local.rs` (`LocalSdProvider`) and `/home/user/harnessUI/src-tauri/src/studio/engine_cmd.rs`:
  - `studio_engine_status`, `studio_engine_download(id, accept_license)`, `studio_engine_cancel`, `studio_engine_remove`, with event `studio-engine-progress`.
  - `studio_local_catalog`, `studio_local_download(entry_id)`, `studio_local_delete`, `studio_local_fit(entry, w, h, frames)`.
- `/home/user/harnessUI/src-tauri/src/gpu_lease.rs`.
- `/home/user/harnessUI/src/components/studio/LocalModels.tsx`.
- `/home/user/harnessUI/scripts/studio_catalog_test.py`.
- `/home/user/harnessUI/third_party/licenses/stable-diffusion.cpp-LICENSE`, plus whatever it vendors: ggml MIT, and libwebp/libwebm BSD if WebM is on.
- A test fixture model script, like `scripts/fetch_test_model.py`, for CI contract tests (`crates/runtime/tests/sd_cli.rs`).

**Changed (Rust)**
- `crates/runtime/src/packs.rs`: generic install.
- `crates/runtime/src/llama/lines.rs`: split on `\r`.
- `crates/runtime/src/llama/backend.rs`: a Vulkan fallback that takes an explicit folder.
- `crates/models/src/catalog.rs`: `generators`, `generatorTiers`, validation.
- `crates/models/src/arch.rs`: `is_diffusion(arch)` for the import message.
- `crates/store/src/schema.rs`: V3 or V4.
- `crates/agent/src/media/price.rs`: `Free`.
- `src-tauri/src/studio/manager.rs`: the local lane, no confirmation for $0 runs, local rows become `interrupted` on resume.
- `src-tauri/src/downloads.rs`: `GroupMeta.generator`, `register_gen_model`, delete counting.
- `src-tauri/src/hub.rs`: catalog list with generators, the import message, `detect` reused.
- `src-tauri/src/runtime_manager.rs`: device bytes, and holding a chat load while a lease is held.
- `src-tauri/src/lib.rs`: `manage`, `generate_handler!`.
- `src-tauri/commands.list` and `src-tauri/capabilities/main.json`: `allow-<cmd>` for each new command.
- `src-tauri/tauri.macos.conf.json`: `externalBin` adds `binaries/sd-cli`.

**Changed (frontend)**
- `src/components/studio/ModelPicker.tsx`: the "This computer" group.
- `src/components/studio/RunCard.tsx`: local stages and the unload note.
- The new Settings → Studio page: engine, CUDA engine pack, graphics-card policy.
- `src/components/models/Installed.tsx`: the Studio line.
- `src/components/models/Downloads.tsx`: generator group labels.
- `src/lib/{ipc.ts, types.ts, words.ts}`.

**Build, CI and documentation**
- `scripts/fetch_llama.py`, or a new `scripts/fetch_sd.py` sharing its helpers:
  - `macos` builds `sd-cli` too; `check_macos_app` requires it.
  - `windows` does **not** bundle it.
  - `verify-packs` covers `sd.lock.json`.
  - A `linux` dev fetch.
- `scripts/fetch_llama_test.py`.
- `scripts/notices.py`: add stable-diffusion.cpp under bundled components on macOS, and "the Studio engine is not part of the Windows installer" next to the GPU packs sentence. Regenerate `THIRD-PARTY-NOTICES.md`.
- `.github/workflows/ci.yml` and `release.yml`: build the macOS `sd-cli` and add `sd.lock.json` to the cache keys. Linux `sd-cli` contract tests. The installer size check is unchanged.
- `catalog/models.json`: generators; bump sequence and sign again.
- `e2e/m8_offline.py`: local generation with every request refused after download.
- `ARCHITECTURE.md`: 7.1 (sd pin), a new 7.9 for local generation, 11, 12, 13 (no sd-server, the argv prompt, refused model formats), 16 and 17.
- `docs/dev/0.1.0/studio-design.md`: add a "Local" addendum keeping the owner's words verbatim. `README.md`, `CHANGELOG.md`, `SECURITY.md`.

**Rough effort.** About 30–40 person-days on top of the cloud plan:

| Part | Person-days |
|---|---|
| Pin and CI build | 4–5 |
| runtime::sd | 6–8 |
| Catalog and fit | 5–6 |
| Store and downloads | 3–4 |
| Provider, manager and lease | 5–6 |
| UI | 6–8 |
| Tests and docs | 4–5 |

Suggested 0.1.0 scope:
- Vulkan, Metal and CPU engines only.
- Two or three picture entries, Apache-2.0 first: for example Z-Image Turbo GGUF Q4, FLUX.1-schnell Q4, an SDXL-class model.
- One small video entry: Wan2.1 T2V 1.3B or Wan2.2 TI2V 5B in GGUF.

Defer to 0.1.x: the CUDA engine pack, sd-server mode, Hugging Face free-form import, and Wan A14B.

## 3. The owner's machines

These figures are approximate, from memory, and must be measured with `studio_catalog_test.py`.

**MacBook Air M5**
- With 16 GB of memory, the Metal working set is about 10.5–12 GB.
- **Pictures work well.** For example, Z-Image Turbo at about 6.5–7 GB of weights (Q4 diffusion + Qwen3-4B text encoder + 0.3 GB VAE) plus 1–2 GB of activations at 1024² is green with no chat model loaded. It is yellow or needs an unload next to a 9B chat model.
- **Video is possible but slow.** The Air has no fan and throttles. Wan 1.3B or 5B at 480p, 33–49 frames, likely takes many minutes, so the default should be small and marked "Slow".
- It runs Metal from the bundled universal `sd-cli`.

**Windows PC, graphics card unknown**
- The Vulkan engine covers AMD, NVIDIA and Intel.
- An 8 GB card gives green pictures at Q4 and small video clips.
- With only an iGPU or no card, it runs on the CPU: pictures take minutes, and video should be marked red or "not recommended".
- The existing Vulkan fallback runtime must be on `sd-cli`'s `PATH`, or a PC whose driver lacks the Vulkan runtime silently runs on the CPU.

## 4. Risks

1. **Upstream churn.** Releases are `master-NNNN-hash` several times a week, with no semver. The latest commit is a refactor ("adopt opaque options"). CLI flags, the progress format and the device naming can change.
   - Mitigation: pin, contract tests, keep every assumption in `runtime::sd`, and use `verify-packs` HEAD checks.
   - Upstream could delete old assets; mirror them to harnessUI's release if that happens.
2. **Security.**
   - `sd-server` has no documented auth and a Web UI, so it is not used in 0.1.0.
   - `sd-cli` puts the prompt in argv: cap its length and check for a prompt-file or `--prompt=` form.
   - Refuse `.ckpt`, `.pt` and `.pth`; every catalog part is pinned by SHA-256.
   - The catalog's roles map to fixed flags, so a signed catalog still cannot inject arguments.
   - The working directory is the engine folder (DLL planting) and the environment is scrubbed.
3. **DLL collisions.** If sd's `ggml*.dll` share a folder with llama's, either engine can break. Keep separate folders.
4. **Graphics-card contention.** Running both with no lease leads to llama-server running out of memory and restart loops. Vulkan drivers may free memory late after a process exits, so re-probe or allow a margin before reloading chat.
5. **Fit accuracy.** Diffusion memory depends on the family, flash attention, VAE tiling and auto-fit. Prefer measured points, and keep red and yellow conservative.
6. **Catalog compatibility.** Generators must stay out of `models` and `tiers`, or 0.0.x apps reject the refreshed catalog (line 294) or suggest diffusion models for chat. A refreshed catalog can list entries that need a newer engine: gate them on `engine.minBuild`.
7. **Licenses and gating.** Many weights are non-commercial or gated. Default to Apache-2.0 entries and show the license before download.
8. **Video playback.** WebM needs a WebM-enabled build, verified for the Windows zip and vendored statically on macOS, or `check_macos_app` fails. The fallback is animated WebP. AVI is not supported.
9. **Load cost per job** with one-shot `sd-cli` is about 5–60 s. Use batch count for "How many"; server mode later.
10. **Device mapping.** sd's ggml may order and name devices differently from llama's. Probe separately and match by description.
11. **Disk.** Models are 5–20 GB each, in the visible models folder. Shared encoders need correct use counting on delete.
12. **CI fixture.** A small diffusion model is needed for Linux contract tests. Small turbo models often have non-commercial terms, so check the license for CI use.
13. **macOS sd-cli flags are unverified.** The static Metal build options (SD_METAL / GGML_METAL_EMBED_LIBRARY, shared libraries off, target 13.3) and the Intel slice flags need confirming at pin time.

## 5. Decisions for the owner

1. **Windows engine:** pack (recommended, about 29 MB on first local download) or bundled (about +25–30 MB installer, works right after install).
2. **When a local job needs the graphics card:** unload the chat model automatically (recommended), or always ask.
3. **0.1.0 local scope:** pictures plus one small video model (recommended), or pictures only, with video in 0.1.x.

## Sources

- [stable-diffusion.cpp repository and README](https://github.com/leejet/stable-diffusion.cpp)
- [Release master-1106-532d2c1 assets](https://github.com/leejet/stable-diffusion.cpp/releases/expanded_assets/master-1106-532d2c1)
- [Server README](https://github.com/leejet/stable-diffusion.cpp/blob/master/examples/server/README.md) and [server API (api.md)](https://github.com/leejet/stable-diffusion.cpp/blob/master/examples/server/api.md)
- [Backend guide](https://github.com/leejet/stable-diffusion.cpp/blob/master/docs/guides/backend.md) and [CLI README](https://github.com/leejet/stable-diffusion.cpp/blob/master/examples/cli/README.md)
- [CLI flags as summarized by DeepWiki](https://deepwiki.com/leejet/stable-diffusion.cpp/5.1-command-line-interface) (third-party; verify against `sd-cli -h`)