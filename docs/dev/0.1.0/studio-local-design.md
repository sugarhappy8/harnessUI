<!-- Owner decisions recorded 2026-11-17 -->
> **Owner's decisions on this addendum (2026-11-17), which override anything below that disagrees:**
>
> 1. **Local is the main thing; cloud is the backup.** Local pictures and local video ship in **0.1.0**, together with the cloud Studio. Studio leads with "On this computer": local models are listed first, are the default whenever this computer can run one well, and Studio's empty state and first-run suggestion lead with local models. Cloud is presented as the backup for computers without a good graphics card; Studio suggests it (OpenRouter or OpenAI) when no local model fits or a local run would be very slow. The owner's words: "It should be models u can pick the like hugging face downloads and it should ship with cloud and emphasis on local being the main thing while cloud is backup for those who don't have good gpu's".
> 2. **People pick local models the way they pick chat models: by searching Hugging Face**, not only from a fixed list. *Get local models* has the recommended list (below) **and** a Hugging Face search for picture and video models the engine can run. For model families harnessUI recognises (from the GGUF metadata and file names), the search fills in the extra files those models need (text encoders, VAE) from a known-good table and shows the total size, licence and fit before download. Families it does not recognise show "Not supported yet" instead of a Download button. Gated repositories use the Hugging Face token the app already supports. This replaces the "only from the signed catalog in 0.1.0" assumption below.
> 3. **The Windows engine downloads with the first local model** (Decision 2 = A). The Mac bundles it.
> 4. **The recommended list is free-for-any-use only** (Decision 3 = A): FLUX.2 [klein] 4B, Z-Image Turbo and Wan 2.2 TI2V-5B (fast version), plus Wan 2.1 1.3B if the benchmark calls for it. Models found through the search show their licence in plain words, with the same green/amber/red badge, and the licence must be accepted before download.

<!-- Addendum drafted 2026-11-17 -->
> **Owner's request (verbatim):** "I also mean like downloading local models that can generate video."
>
> **Status:** proposal, waiting on the owner's three decisions in L8. Nothing in the approved cloud design changes for cloud runs. Where this addendum changes a shared part (the V3 table, the `MediaProvider` trait, `StartRequest`), it says so, and those changes should land before plan Task A8 ("V3 exactly as in the design §5") is built.

# Addendum: Local models in Studio

This proposal is dated 2026-11-17 and is read only: no files were edited. It draws on three research reports from 2026-11-17 (engines and models, licences, how harnessUI's local runtime can be reused). I also re-read stable-diffusion.cpp's server and CLI READMEs today. As in the main design, plain English comes first in each section and the parts for engineers are marked **Technical detail**.

Speed and memory figures carry these labels:
- **[M]**: measured by a named third party.
- **[E]**: an estimate drawn from those measurements.
- **[U]**: not confirmed yet.

Every figure for the owner's own machines is [E] until the one-day benchmark in L7 has been run.

---

## L0. Summary

- **What.** Studio's model picker gets an **"On this computer"** group above the cloud companies. Local models are downloaded once from a short list that harnessUI checks and signs, and then run on your own graphics card, or on the Apple chip in a Mac. They make pictures and short videos. Nothing leaves the computer, nothing costs money, and a run can be stopped at once.
- **Finding and downloading models.** Studio gets its own **Get local models** view, organised by Pictures and Video. Each model shows:
  - its size;
  - its licence, in plain words;
  - whether it fits this computer;
  - an honest time estimate for this computer, for example "a 4-second 480p clip takes about 6 min here".

  The licence is shown again, and must be accepted, before anything downloads. The Models dialog shows these downloads in its Downloads tab and one summary line in Installed. It never offers to load a picture or video model as a chat model.
- **Engine.** **stable-diffusion.cpp** (MIT licence) is the sister project of llama.cpp. It reads the same GGUF files, needs no Python, and runs FLUX.2, Z-Image, Wan 2.1/2.2 video, LTX-2.x and more on Vulkan, Metal, CUDA and CPU.
  - **Windows:** a 29 MB "Studio engine" (Vulkan and CPU) downloads together with your first local model, pinned by SHA-256 like the existing ROCm and CUDA packs. The installer does not change.
  - **Mac:** the engine is built from source and bundled, like `llama-server`.
- **Starter models in 0.1.0** (all Apache-2.0, none gated, if Decision 3 = A):
  - Pictures: **FLUX.2 [klein] 4B** (5.3 GB, runs on everything) and **Z-Image Turbo** (6.7 GB).
  - Video: **Wan 2.2 TI2V-5B** in a 4-step fast version (about 9–13 GB), at 480p by default.
  - On a computer with no usable graphics card, video is hidden.
- **What stays the same.** Local runs use the same cards, gallery, viewer, Again, Reuse and Animate.
- **What is different for local runs:**
  - The Create button shows a time ("~4 min") instead of a price.
  - There is no cost confirmation and no daily limit, and local runs never count toward "spent today".
  - The Cloud mark is replaced by "On this computer".
  - Stop really stops the work and costs nothing.
  - The chat model is unloaded automatically when the graphics card is needed, with a "Load it again" link.
- **Safety and law.**
  - A one-time notice before the first local download.
  - No uncensored models in the list.
  - A small built-in check on prompts.
  - Every local file is labelled as AI-made in its metadata, without the prompt. Here harnessUI is the only party that can add such a label, unlike cloud results.
- **Size of the work.** About **38–49 person-days** on top of the cloud plan's 37–43. Of that, about 6–9 days are video-specific. For two engineers that is about 4–5 more calendar weeks.
- **Before any time estimate is promised in the UI:** run a one-day benchmark on the owner's MacBook Air M5 and Windows PC (L7). Two key facts are unconfirmed: whether Metal runs well on the M5, and whether the fast 4-step Wan version loads in this engine.

---

## L1. What you see

### The model picker: "On this computer" next to the cloud companies

- The picker (Wireframe L1) lists **On this computer** first, then **Cloud · OpenRouter** and **Cloud · OpenAI**, exactly as in the main design.
- Local models carry the tag "free · private · nothing is sent" instead of the Cloud mark. Each row shows:
  - a fit word: "Fits", "Slow here" or "Fits after the chat model is unloaded";
  - a time per picture, or per default clip, on this computer;
  - the same feature chips as cloud models (Edit, Start frame).
- Only **installed** local models are listed. A last row, **+ Get local models (2 more fit this computer)**, opens the download view.
- **No local models yet:** "No local models yet. FLUX.2 klein 4B fits this computer (5.3 GB). [Get local models]".
- **Right panel.** When a local model is chosen, the panel shows "On this computer · Radeon RX 7600" (the probed device) where cloud models show "via OpenRouter · Cloud".
- **The line above Create** reads "On this computer: your prompt and pictures stay here", followed by "· the chat model will be unloaded to make room" when that applies.
- **Defaults.** The default model is still the one you used last. The first time, on a computer where a local picture model is installed and green, Studio suggests it. Studio never switches between local and cloud by itself.

### Wireframe L1: the picker

```
+ Choose a model ----------------------------------------------------------+
| Search...                       [Pictures] [Video]   Edit . Start frame  |
|                                                                          |
| ON THIS COMPUTER                       free . private . nothing is sent  |
| > FLUX.2 klein 4B         Fits        about 25 s a picture     Edit      |
|   Z-Image Turbo           Fits        about 1 min a picture              |
|   + Get local models   (2 more fit this computer)                        |
|                                                                          |
| CLOUD . OpenRouter                                          [Cloud]      |
|   GPT Image 2.6 Flare                 $0.04 a picture   Edit . Refs 16   |
|   Nano Banana 2.1                     price shown when picked            |
|   ...                                                                    |
| CLOUD . OpenAI                                              [Cloud]      |
|   GPT Image 2.6 Sunburst              price per picture     Edit         |
+--------------------------------------------------------------------------+
```

### Get local models (Studio's own view)

- **Where it lives.** It replaces the card stream, with a [Back] button. It opens from:
  - the picker;
  - Studio's empty state;
  - the Settings → Studio "Local models" block;
  - the "Manage in Studio" link in Models → Installed.
- **The header** says what harnessUI detected, so the estimates can be checked: "This computer: Radeon RX 7600, 8 GB graphics memory · 32 GB memory · 412 GB free".
- **Filters.** Tabs **Pictures | Video**. **Only what fits** is on by default. **Show non-commercial models** is off, and has nothing behind it in 0.1.0 unless Decision 3 = C.
- **Each model shows:**
  - the name and one line about what it is good at;
  - the publisher;
  - the total download size (all parts);
  - a licence badge in plain words (green "Free for any use, including commercial"; amber "Commercial use only under $X revenue"; red "Non-commercial");
  - the fit word;
  - the time estimate for this computer;
  - [Download].
- **"Recommended for this computer"** marks one picture model and one video model per hardware tier.
- **Models that do not fit** are folded into **Not for this computer (n)**, each with a plain reason: "needs 16 GB of graphics memory", "needs a Mac with 32 GB or more", or "video needs a graphics card or an Apple-silicon Mac". They are visible, so nobody wonders why a well-known model is missing, but they cannot be downloaded from here.
- **Licence and region rules:**
  - Models whose licence excludes whole regions (HunyuanVideo, MiniMax-H3) and models that advertise themselves as uncensored (Chroma, "Uncensored" re-uploads) are not in the list at all.
  - Free-form Hugging Face search for picture and video models is not in 0.1.0 (L6).
- **The footer** shows the Studio engine's state, "comes with your first model (29 MB, MIT)" or "installed · build master-1106 · [Remove]", and the content notice link.

### Wireframe L2: Get local models, on the MacBook Air (Video tab)

```
+ Studio > Get local models ---------------------------------------------- [Back] +
| This computer: Apple M5, 16 GB shared memory . 180 GB free                       |
| [ Pictures | Video ]                 [x] Only what fits   [ ] Show non-commercial |
|                                                                                  |
| Wan 2.2 5B, fast                                    Recommended for this computer|
| Short clips from words, or from a start picture.             Wan (Alibaba)       |
| about 9 GB . Apache 2.0: free for any use, including commercial                  |
| Slow here. A 2-second 480p clip takes about 2-5 min. 5 s at 720p can take        |
| 10-30 min, and the Mac gets warm. (first guess)                     [Download]   |
| -------------------------------------------------------------------------------- |
| > Not for this computer (1)                                                      |
|     LTX-2.3 with sound . 25.6 GB . needs a Mac with 32 GB or more                |
+----------------------------------------------------------------------------------+
| Studio engine: built in.   Local models run only on this computer; no company    |
| checks prompts or results.  [What that means]                                    |
+----------------------------------------------------------------------------------+
```

On a Windows PC with an 8 GB card, the Pictures tab lists FLUX.2 klein 4B ("Fits · about 10–25 s a picture here") and Z-Image Turbo ("Fits · about 30–70 s a picture here"), and the footer reads "Studio engine: comes with your first model (29 MB)".

### Download and licence, before anything is fetched

- [Download] opens a confirmation listing every part: the model itself, the part that reads the prompt, and the picture decoder. Each part shows its size and licence, then the plain-words verdict for what you make.
- **Gated repos.** If a part comes from a gated repo, the dialog says which form the publisher will show and what it collects ("the publisher receives your Hugging Face username and email"). The repo opens in the browser. The existing 401 and 403 messages ("add a Hugging Face token", "accept its terms") apply, and the download retries itself after acceptance. None of the recommended 0.1.0 models is gated.
- **Amber licences** add a line, for example "Commercial use only if your organisation earns less than $5M a year. The licence asks you to review pictures before you share them."
- **Shared parts.** A part already on disk from another model is listed as "already on this computer" and is checked, not downloaded again.
- **Disk space.** If the download would leave less than 10 GB free, the dialog says so before starting.
- **Progress.** Downloads use the existing download manager (resume, SHA-256 check) and show in Models → Downloads as well as on the model's row.

### Wireframe L3: the download confirmation

```
+ Download Z-Image Turbo? ------------------------------------------------+
| 3 files, 6.7 GB in all, from Hugging Face (huggingface.co)              |
|   Z-Image Turbo (makes the picture)        3.9 GB    Apache 2.0         |
|   Qwen3 4B (reads your prompt)             2.5 GB    Apache 2.0         |
|   Picture decoder                          0.3 GB    Apache 2.0         |
| Pictures you make: free for any use, including commercial.              |
| Also downloads the Studio engine (29 MB, MIT licence).                  |
| [Read the licences]                                                     |
| Downloading means you accept these licences.                            |
|                                      [Cancel]   [Accept and download]   |
+-------------------------------------------------------------------------+
```

### An honest speed and fit estimate

- **Two separate verdicts.** The fit word answers "will it run?". The time answers "how long will it take here?".
  - A model can fit and still be slow: "Slow here".
  - On a computer with no usable graphics card, pictures are marked "takes several minutes" and video is unavailable.
  - An Intel Mac counts as processor only.
- **First guess, then measured.** Until this computer has made something with a model, its time is labelled **(first guess)** and shown as a range from the model's catalog data for this kind of hardware. After the first run, Studio uses what it measured and the label goes: "about 6 min for a 4-second 480p clip on this computer". This is the local version of the cloud design's "usually about 2 min".
- **During a run** the card switches to a live estimate built from the steps done so far: "step 3 of 4 · about 0:30 left".
- **The estimate follows the settings.** Each Length and Resolution chip shows its own time ("2 s · ~3 min", "5 s · ~9 min"), so the cost of a bigger setting is visible before you press Create, just as the cloud chips show price.
- **Long runs are not blocked.** When the estimate passes 15 minutes, an amber line under Create says "This will take about 25 min and keep the graphics card busy. On a MacBook it will get warm." There is no dialog.

### Wireframe L4: a local run card, and the Create line

```
| "fox in deep snow, golden hour, 35mm"                                                |
| FLUX.2 klein 4B . on this computer . 1024x1024 . x2 . seed 81723 . free              |
| [######...........]  Picture 1 of 2 . step 3 of 4 . 0:21 so far . about 0:30 left     |
| The chat model was unloaded to make room.  [Load it again]                  [Stop]    |

| "slow push-in on a lighthouse, waves"   Wan 2.2 5B, fast . on this computer . 2 s     |
| Waiting for the graphics card: a chat reply is being written. Starts when it ends.    |
|                                                                           [Cancel]   |

| "same, at night"   Wan 2.2 5B, fast . on this computer                                |
| Stopped. Your computer is no longer working on it. Nothing was charged.  [Run again]  |

+----------------------------------------------------------------------------------------+
| [+ Add picture]  Describe the video you want...                                         |
| On this computer: your prompt and pictures stay here . the chat model will be unloaded  |
|                                                                 about 4 min  [ Create ] |
+----------------------------------------------------------------------------------------+
```

### First local download: a one-time notice (also in Help and the view's footer)

> **Local models run only on your computer.** No company checks your prompts or pictures. Don't create sexual images of children, or sexual or intimate images of real people without their consent, and don't impersonate people. These are crimes in many countries. Each model's licence and use policy also applies. [I understand]

**Technical detail**
- **Detection.** The header and verdicts reuse `hub::detect` (the `Target` built from `llama-server --list-devices`: Metal working set, Vulkan, CUDA, ROCm, otherwise CPU) and `hardware.rs` for system memory. Free disk comes from the existing `diskspace` check.
- **New and changed webview files:**
  - New `src/components/studio/LocalModels.tsx` (the view), `LocalModelRow.tsx`, `DownloadLocalModel.tsx` (the licence dialog) and `LocalMark.tsx`. `CloudMark.tsx` stays as it is.
  - `ModelPicker.tsx` gets the group.
  - `PromptDock.tsx` shows the time where cloud runs show the price.
  - `RunCard.tsx` gets local stages, the unload note and Stop.
  - `FitBadge.tsx` is reused for the fit word.
- **Notice acceptance.** The notice is a typed pref (`studioPrefs.localNoticeAccepted`), recorded with a date. Downloads are refused by the backend until it is set.
- **All copy** goes in `src/lib/words.ts`.

---

## L2. The engine: what runs the models, and how it is delivered

### Which engine

**stable-diffusion.cpp** (leejet, MIT) runs on both Windows and macOS.
- **Why this one.** It needs no Python, so it can ship inside a Tauri app the way llama.cpp does. It covers the most models: picture models, plus Wan 2.1/2.2, LTX-2.3/2.5 and HunyuanVideo 1.5 for video. It also has the low-memory tricks a laptop needs: running one part at a time, tiling the decoder, and moving parts to the processor.
- **The other options:**
  - ComfyUI is Python, at least 4.85 GB per install, and GPL.
  - mflux and FastVideo are Python.
  - Draw Things has no Windows version and is GPL.
  - Amuse is closed.
  - turbo-mlx (Swift/MLX, MIT) is the template to keep in mind for a faster Mac engine in 0.2, if the benchmark shows sd.cpp's Metal on the M5 Air is more than about 2× slower than the MLX figures.
- **Upstream churn.** Builds are rolling (`master-1106-532d2c1`, 16 Nov 2026), and the README warns that flags change often. harnessUI pins one build and changes it only together with passing contract tests, the same rule as llama.cpp (ARCHITECTURE 7.1).

### How it is delivered

| | Windows | macOS |
|---|---|---|
| **What** | Upstream `sd-master-532d2c1-bin-win-vulkan-x64.zip`: Vulkan and CPU, every GPU vendor | `sd-cli` built from the pinned commit in CI: universal (arm64 + x86_64), static, Metal library embedded, macOS 13.3 minimum |
| **How** | **One-click pack, downloaded with the first local model** (Decision 2), SHA-256 and size pinned, GitHub hosts only, resume, `.complete` marker | **Bundled** as an `externalBin` next to `llama-server`; signed and notarized with the app |
| **Size** | about 29 MB download. The installer stays about 50 MB (cap 150 MB) | about 30–40 MB more in the app, about 15–20 MB more in the .dmg [E]. The Mac has no size cap |
| **Folder** | `<local data>/engines/sd-<build>/`, **never** `resources/llama/main`: sd.cpp ships its own `ggml*.dll` with the same names as llama.cpp's, built from a different ggml | `Contents/MacOS/sd-cli` |
| **Later packs** | "Studio engine: NVIDIA CUDA", about 930 MB (393 MB build + 537 MB CUDA 12.8 runtime; it cannot share llama's CUDA 13.4 runtime), in 0.1.x. ROCm (270 MB) later | none |

- **Why build from source on the Mac.** The two research reports disagree on whether upstream's Mac zip is universal. It does not matter: it ships a 95 MB shared `libstable-diffusion.dylib`, and its minimum macOS version is unverified. The app's rules (`check_macos_app`: both architectures, system libraries only, 13.3 minimum) and notarization call for the same treatment `llama-server` gets.
- **The M5 Metal bug.** With Metal's new tensor path on, Z-Image produces blank white pictures on M5 chips (upstream issue #1990, open since Sep 2026). The engine is started with `GGML_METAL_TENSOR_DISABLE=1` for the models that need it, until the bug is fixed.

### How a run works: one engine process per job

- **In 0.1.0, each local run starts its own `sd-cli` process.** It loads the model, makes the pictures or the clip, writes the files and exits.
- **Why not upstream's server (`sd-server`)?** It has a better job API (progress, previews, cancel, keeping the model loaded), but:
  - Its README lists only `--listen-ip` and `--listen-port` as network options. There is no key and no auth (checked 2026-11-17).
  - It serves a built-in web UI and OpenAI- and A1111-style APIs on the same port.
  - Any program on the computer, or a web page using DNS rebinding, could drive it. llama-server is protected with a random key and `--cors-origins none`; sd-server cannot be.
  - It moves to 0.1.x behind the same provider once it can require a key (upstream, or our own build) and its web UI can be turned off.
- **The cost of one process per job.** The model reloads for every run, about 5–60 s. "How many" uses the engine's batch count, so 4 pictures load the model once. Again and Reuse pay the load each time.
- **One local job at a time** for the whole computer. Cloud runs keep their own queue and run alongside, because they do not use the graphics card.

### Progress, Stop, failures

- **Progress.** The card shows the stages: loading the model (0–10%), reading the prompt (10–15%), drawing (15–90%, step k of N, picture i of n), decoding (90–99%), saving. These come from the engine's own output.
  - Wan 2.2 14B models draw in two passes; the card says "pass 1 of 2".
  - Live preview pictures are 0.1.x unless the pinned build's CLI can write them [U].
- **Stop works differently from cloud.**
  - Stop kills the engine and everything it started at once.
  - The graphics card's memory is freed when the process exits, and half-made files are deleted.
  - The card says "Stopped. Your computer is no longer working on it. Nothing was charged."
  - Esc and the global stop hotkey still never stop Studio jobs, as in the main design.
- **Out of memory.** If the graphics card runs out of memory, Studio retries **once** on its own with the low-memory options (decoder tiling, parts moved to the processor). It says so: "Ran out of graphics memory, trying again with memory saving (slower)". A second failure is shown with a suggestion: "Try 480p or a shorter clip".
- **Other failures** are named in plain words:
  - "The graphics driver stopped responding";
  - "A model file is damaged, download it again";
  - "The Studio engine is missing, download it again".
- **Quitting.** Local jobs cannot survive quitting the app.
  - Quitting while one runs asks first: "A video is being made on this computer. Quitting stops it. [Keep working] [Quit and stop]".
  - After a crash, the card says "The app closed while this was being made. [Run again]" (free).
  - Keeping the computer awake while a job runs is 0.1.x.

### Sharing the graphics card with a loaded chat model

- **Fits alongside:** if the job fits next to the loaded chat model, both run. The card says "Chat replies may be slower while this runs".
- **Does not fit:** Studio follows the choice in Settings → Studio:
  - **Unload the chat model** (default);
  - **Ask**;
  - **Use the processor for local pictures**.

  Unloading waits for a reply that is being written ("Starts when the reply ends · Cancel"), then unloads the chat model with the existing "Unloaded to free memory · Load" chip.
- **During a local job,** loading a chat model (sending a message, or Load) waits: "Studio is using the graphics card. The chat model loads when it finishes · [Stop Studio job]". This prevents llama-server running out of memory mid-chat and its supervisor looping on restarts.
- **After the queue empties,** the chat model stays unloaded, with the "Load" chip. A "Reload it after Studio" checkbox exists, off by default.
- **Macs** share one memory pool, so the same rules apply to the whole memory. "Keep in memory" (mlock) on a chat model always forces an unload.
- **Processor only:** use physical cores minus one, and warn that chat will be slow.

**Technical detail**
- **The lock file.** New `sd.lock.json`, compiled into `runtime::sd::lock`, holds:
  - the upstream `tag` and `commit`;
  - Windows `packs` (`studio-vulkan`, later `studio-cuda`), each with url, size and sha256;
  - its own `keep_files` (`sd-cli.exe`, `stable-diffusion*.dll`, `ggml*.dll`, licences);
  - `macos-aarch64` and `macos-x86_64` `build_from_source`, with target `sd-cli` and flags like llama's: `BUILD_SHARED_LIBS=OFF`, `GGML_NATIVE=OFF`, Metal embedded, target 13.3, one architecture per slice. The exact sd.cpp Metal flags and the libwebp/libwebm static linking are [U] and get confirmed at pin time;
  - a `dev.linux-x86_64` entry for contract tests.

  It is **not** added to `llama.lock.json` `packs`: `list_packs` would show it under the llama backends, `install` would filter it with llama's `keep_files`, and `discover_with` expects `llama-server`.
- **Fetching.** `scripts/fetch_sd.py`, sharing helpers with `fetch_llama.py`. `download()` keeps its host allow-list. The macOS step builds and runs `lipo`. `check_macos_app` adds `sd-cli` to the required programs. `verify-packs` covers `sd.lock.json`. CI and release cache keys add the new lock and script.
- **Installing the pack.** Generalize `runtime::packs::install` (crates/runtime/src/packs.rs:467) into `install_files(dir, files, keep, keep_all, vc_runtime)`, used by both locks. VC++ DLLs are copied from the installed llama folder, as packs already do. The Studio engine is downloaded through `download_verified` (line 195) with `ALLOWED_HOSTS` (line 22).
- **Commands.** `src-tauri/src/studio/engine_cmd.rs`: `studio_engine_status`, `studio_engine_download(accept_license)`, `studio_engine_cancel`, `studio_engine_remove`, and the event `studio-engine-progress`, modelled on `packs_cmd.rs`.
- **Spawning.** New `crates/runtime/src/sd/`:
  - `engine.rs`: find the bundled, pack or dev engine.
  - `devices.rs`: `sd-cli --list-devices`, matched to llama's devices by description, because index, order and case (`vulkan0`) can differ.
  - `args.rs`, `progress.rs`, `failure.rs`, `run.rs`.

  It is modelled on `llama/fit_params.rs::measure`:
  - `runtime::spawn` (the only allowed spawn; `scripts/check-spawn.sh` enforces it);
  - the engine folder as working directory;
  - `EnvSpec::minimal()` plus `GGML_NO_BACKTRACE=1`;
  - `PATH` ending with the installed `resources/llama/main/vulkan`, via `path_with_vulkan_fallback` taking an explicit folder. Otherwise a PC without a Vulkan runtime silently runs on the processor;
  - stdin null, stdout and stderr piped;
  - a `CancellationToken` → `ManagedChild::start_kill()` (kill the tree);
  - a Job Object or process group, so the reaper kills it if the app dies.
- **Arguments come only from typed fields.**
  - Each catalog part role maps to one fixed flag: `--diffusion-model`, `--llm`, `--t5xxl`, `--clip_l`, `--vae`, `--clip_vision`, `--high-noise-diffusion-model`, or `-m`.
  - Then numbers (steps, cfg, width, height, `--video-frames`, `--fps`, `--seed <app-chosen>`).
  - Then whitelisted words (sampler, scheduler, `--backend <probed device>`, `--vae-tiling`, `--offload-to-cpu`).
  - Then `-o <gallery>/.partial/<run>/…`.

  A signed catalog therefore cannot inject flags. Every flag name is checked against the pinned `sd-cli -h` in a contract test.
- **The prompt reaches argv,** unlike llama, so:
  - its length is capped (4,000 characters; Windows caps a command line at 32,767);
  - `--prompt=<text>` is used if the build accepts it, so a prompt starting with `-` is never read as a flag;
  - a prompt file is used instead if the pinned build offers one [U].
- **Metadata.** sd-cli embeds its "generation record", including the prompt, in output files. Studio passes `--disable-image-metadata` and writes its own label (L4). Whether that flag also covers WebM is [U].
- **Progress parsing.** `progress.rs` needs a line reader that splits on `\r` as well as `\n`: `llama/lines.rs::next_line` (line 29) splits only on `\n` and would see one 16 KiB line. It parses `|===> | k/N - x s/it` and the stage log lines into `LocalProgress { stage, pass, step, steps, picture, pictures, elapsed_ms, sec_per_step }`. Fixtures are captured from the pinned build and treated as "no interface guarantee".
- **Failures.** `failure.rs` classifies out of memory, Vulkan device lost, missing DLL, wrong or unreadable model file, and a crash, with one automatic retry after out of memory. There is no restart policy and no timeout. A stall detector says "No progress for 10 minutes · [Stop] [Keep waiting]".
- **Video output.** Ask for WebM (VP8). Animated WebP is the fallback. **Never AVI**: it is not in the gallery's allowed types and plays in neither WebView2 nor WebKit. No MP4 and no ffmpeg in 0.1.0. WebM playback in WKWebView is [U] and on the benchmark list.
- **The graphics-card lease.** New `src-tauri/src/gpu_lease.rs` allows one tenant at a time, either the chat server or a local Studio job: `acquire_for_studio(need) -> Alongside | AfterUnload | WaitForReply | UseProcessor | Refused`.
  - Unloading is `runtime.before_change(&model_busy, when_idle = true)` (runtime_manager.rs:316), then `runtime.unload_by_user()` (line 365).
  - Add `pub fn device_bytes()` around the private `loaded_on_device` (line 227).
  - `RuntimeManager`'s load path checks the lease and waits or asks.
  - Vulkan drivers can free memory late, so re-probe, or keep a 1 GB margin, before reloading chat.
  - `BusyKind` is not the right tool: it counts users of the llama model, not graphics-card tenants.

---

## L3. Starter catalog

Sizes are full downloads (all parts) from the Hugging Face API. Times are for one picture at 1024×1024, or a 2-second 480p clip, from L7's table [E].

### Pictures

| Model | Licence (outputs) | Download | Needs | In 0.1.0? | Shown as |
|---|---|---|---|---|---|
| **FLUX.2 [klein] 4B**: 4 steps, quick, can also change a picture you add | Apache-2.0, ungated (green) | **5.3 GB** at Q4; 8.9 GB at Q8 | 4–6 GB graphics memory + 16 GB RAM; also runs on processor only (about 5–6 min at 512² [M on a laptop CPU]) | **Yes, the default for every computer** | Recommended on every tier |
| **Z-Image Turbo**: 8 steps, photo-real | Apache-2.0, ungated (green) | **6.7 GB** at Q4; 11.2 GB at Q8 | 8 GB+ graphics memory; slow on the Air (and needs the M5 workaround) | **Yes** | Recommended for 8 GB+ cards; "Slow here" on the Air; hidden on processor only |
| **FLUX.3 image klein**: 5.6B, newest, best text in pictures and editing | FLUX Kommunity v1.0 (amber): commercial use only under $5M revenue; you must filter or review outputs before sharing | 6.4 GB at Q4; 10.8 GB at Q8 | 8 GB+ graphics memory | Decision 3 (B) | Amber badge and the licence line |
| **Qwen-Image 2512**: 20B, the "best quality" option | Apache-2.0 (green). Not Qwen-Image **2.1**, which is research-only | about 17 GB at Q4 [E, measure] | 16 GB+ graphics memory | 0.1.x | "Not for this computer" on the Air and on 8–12 GB cards |

### Video

| Model | Licence | Download | Needs | In 0.1.0? | Shown as |
|---|---|---|---|---|---|
| **Wan 2.2 TI2V-5B, fast (4 steps)**: words or a start picture to video, 24 fps, 480p by default, 720p offered | Apache-2.0 (green). The 4-step add-on's licence is [U] | **about 9–13 GB** (8.5 GB at Q4 to 12.9 GB at Q8; level Q5 or higher, because heavily squeezed versions blur) | 8 GB graphics memory + 16 GB RAM at 480p; 12–16 GB + 32 GB for 720p. Mac with 16 GB+ (24 GB recommended) | **Yes, the one video model** (Decision 1) | Recommended for 8 GB+ cards and Apple-silicon Macs; hidden on processor only |
| **Wan 2.1 T2V 1.3B**: words to video only, 480p, 16 fps | Apache-2.0 (green) | 6.8–9.1 GB (fp16, never squeezed) | 6 GB+ graphics memory; the small option | Only if the benchmark says Wan 2.2 5B is too slow on the Air; otherwise 0.1.1 | "Quick and simple" |
| **LTX-2.3 distilled**: video **with sound**, start and end frames | LTX-2 Community (amber: free under $10M revenue). Item 20's "directly competes" clause is unclear for a free video app. Needs Gemma 3 12B (Gemma Terms) | 25.6 GB at Q4 | 16 GB+ graphics memory + 32 GB RAM | 0.1.x, after written confirmation from Lightricks | Hidden on Macs under 32 GB; "Not for this computer" on 8–12 GB cards |

Later and advanced: Wan 2.2 I2V A14B (about 26 GB, best picture-to-video and first/last frame, 24 GB cards).

**Never in the catalog:**
- HunyuanVideo 1.5 (its licence excludes the EU, UK and South Korea, outputs included).
- MiniMax-H3 (excludes those and the USA, and binds the app to build safeguards).
- Qwen-Image 2.1 (research or evaluation only).
- FLUX.1/FLUX.2 [dev] and FLUX.2 klein 9B (non-commercial; only behind "Show non-commercial", if ever).
- Chroma and "uncensored" re-uploads.
- CogVideoX and Mochi (the engine does not support them).

### What the owner's machines will see (before the benchmark, so [E])

| | MacBook Air M5 (memory size unknown: 16, 24 or 32 GB) | Windows PC (graphics card unknown) |
|---|---|---|
| FLUX.2 klein 4B | Fits; about 1–2 min a picture | 12 GB NVIDIA: 3–10 s; 8 GB card: 10–25 s; no usable card: 5–6 min at 512² |
| Z-Image Turbo | Slow here; 2–5 min | 12 GB: 8–25 s; 8 GB: 30–70 s; no card: hidden |
| Wan 2.2 5B fast, 2 s 480p | Slow here; 2–5 min (4 s: about 4–10 min); 5 s 720p: 10–30 min, warm, may throttle (no fan) | 12 GB: 20–80 s; 8 GB: 1–3 min; no card: video unavailable |
| LTX-2.3 | Not for this computer (hidden under 32 GB) | Only with 16 GB+ card and 32 GB RAM |

- NVIDIA cards on the default Vulkan engine sit at the slow end of each range. The CUDA pack (0.1.x) would be faster.
- The base Wan 5B, without the 4-step version, is about 10× slower than the table. That is why the 4-step version matters (L7).
- The best measured Mac figure (FastVideo's MLX engine on a 24 GB M5 Air: 5 s 720p in 200 s [M]) comes from a different engine. It is a ceiling, not a promise.

**Technical detail: the catalog entry**

The entries live in `catalog/models.json` under a new top-level **`generators`** key, plus **`generatorTiers`** (recommended ids per memory tier and kind). Rules:
- `schemaVersion` stays 1, `sequence` is bumped, and the file is signed again.
- Old apps (0.0.x) refresh the latest catalog and ignore unknown keys: `Catalog` has no `deny_unknown_fields`.
- The entries **must not** go into `models` or `tiers`. `catalog.rs::validate` (line 294) rejects any file that is not `.gguf`, and every `models` row feeds the chat picker.
- A compatibility test parses a catalog with `generators` through the existing `v005` parser.

Illustrative entry (placeholders, not real hashes):

```jsonc
"generators": {
  "flux2-klein-4b-q4": {
    "name": "FLUX.2 klein 4B", "kind": "image",            // image | image-edit | video
    "family": "flux2",                                     // compiled-in whitelist; unknown → entry hidden
    "inputs": ["reference"],                               // reference | startFrame | endFrame
    "parts": [
      { "role": "diffusion", "repo": "leejet/FLUX.2-klein-4B-GGUF", "revision": "<40-hex>",
        "path": "<Q4_0 file>.gguf", "size": 0, "sha256": "…", "license": "apache-2.0" },
      { "role": "llm", "repo": "unsloth/Qwen3-4B-GGUF", "revision": "<40-hex>", "path": "…Q4_K_M.gguf", "…": "…" },
      { "role": "vae", "repo": "Comfy-Org/flux2-klein-4B", "revision": "<40-hex>",
        "path": "split_files/vae/flux2-vae.safetensors", "size": 336211292, "sha256": "868fe7b3…" }
    ],
    "licenses": { "apache-2.0": { "name": "Apache License 2.0", "url": "…/blob/<rev>/LICENSE", "sha256": "…",
        "commercialOutputs": "yes", "revenueLimitUsd": null, "excludedRegions": [], "obligations": [],
        "plainWords": "Free for any use, including commercial." } },
    "gate": { "mode": "none", "collects": [] }, "contentNotes": "none",
    "defaults": { "steps": 4, "cfg": 1.0, "sampler": "euler", "width": 1024, "height": 1024 },
    "params": [ /* the cloud design's ParamSpec, rendered by ParamField.tsx as is */ ],
    "limits": { "multipleOf": 16, "frameRule": null },     // video: "4n+1" (Wan) or "8n+1" (LTX)
    "memory": [ { "width": 1024, "height": 1024, "frames": 1, "peakBytes": 0, "device": "vulkan", "build": 1106 } ],
    "speed":  [ { "deviceClass": "apple-m5", "width": 1024, "height": 1024, "frames": 1,
                  "loadSeconds": 0, "secondsPerStep": 0, "decodeSeconds": 0 } ],
    "quirks": ["metal-tensor-off"],                        // compiled-in whitelist → env or flags; never free text
    "engine": { "minBuild": 1106 }, "tested": "2026-11-2x"
  }
}
```

- **Allowed file types:** **`.gguf` and `.safetensors` only**. `.ckpt`, `.pt` and `.pth` are pickle files that can run code, and are refused.
- **Revisions.** Every part has its own repo and a 40-hex revision, because encoders and decoders usually live elsewhere.
- **Licences.** Each repo's licence is taken from the upstream `base_model` chain, never from a mirror's label. Mirrors' labels are often wrong: AtomicChat's Qwen-Image-2.1 GGUF claims Apache. The **effective licence is the strictest of all parts**.
- **Decoders.** Use the byte-matching Apache, ungated copies (Comfy-Org `flux2-klein-4B`, `z_image_turbo`), not the gated non-commercial FLUX.1-dev and FLUX.2-dev files that the sd.cpp docs link.
- **Parsing** is in `crates/models/src/catalog.rs` with its own validation. An unknown role, family, quirk or `minBuild` above the pinned build hides that entry; it never fails the catalog.
- **Installed models** go in a new `gen_models` table (L5). They never appear in the `models` table: `register_installed` needs a GGUF header (`hub.rs:836`), and every row there feeds the chat picker.

---

## L4. How local runs fit the existing Studio design

- **Same cards, gallery and remix.**
  - Local runs appear in the same card stream, grid and viewer.
  - Again, Reuse all, Reuse without seed, My setups, Use as reference and Send to chat (pictures) all work.
  - **Animate** works from any picture with Wan 2.2 5B, which takes a start frame. End frame is hidden, because it is not documented for this model.
  - **Edit with words** works with FLUX.2 klein 4B, which takes reference pictures.
  - Settings carry between cloud and local models through the same `canonical.ts` rules (shape, count, resolution, length, seed, references, start frame), with the same "what changed" note.
- **Seeds.** Studio always picks and sends the seed, so local results can be repeated. The help adds: "The same seed on another graphics card, or on the processor, can give a slightly different picture."
- **Advanced, for local models,** shows only what the model's catalog entry declares:
  - steps;
  - guidance (hidden for 4-step models, which use 1.0);
  - sampler and scheduler (from a whitelist);
  - exact size (multiples of 16);
  - frames and fps (video);
  - negative prompt (only for models that use guidance above 1);
  - **Memory saving** (Automatic | Off | On);
  - **Run on** (Graphics card | Processor).

  **Extra fields (JSON) is not offered for local models,** because those would become command-line flags. "See the exact request" becomes **"See the exact command"**: the engine's arguments, with file paths and the prompt shown.
- **Time instead of price.**
  - The Create button shows "~4 min", and the card shows "free" where cloud cards show cost.
  - The viewer shows "Cost: free · took 3 min 48 s · on Radeon RX 7600 (Vulkan) · engine build 1106".
  - Local runs never ask for cost confirmation and never touch the daily limit.
  - "Spent today" counts cloud runs only.
  - The backend enforces this too: a local run is never refused with `studio_needs_confirmation` or `studio_daily_limit`.
- **Privacy.**
  - With a local model selected, the Cloud mark is absent everywhere for that run: the picker row, the right panel, the line above Create, the card and the viewer. "On this computer" takes its place.
  - The only network traffic local Studio ever makes is downloading model files from `huggingface.co` and the engine from GitHub, each started by a click.
  - Making a picture or video sends nothing. `e2e/m8_offline.py` makes a local picture with every request refused after download.
  - The audit row is written exactly as for cloud runs (server `studio`, tool `picture` or `video`, the prompt's length but never the prompt), with "on this computer", the catalog model id and revision, and the engine build.
- **Content.**
  - The one-time notice (L1) appears before the first local download.
  - There are no uncensored models, and no repos tagged "not for all audiences".
  - A small, always-on local check refuses prompts that pair sexual words with words for children: "Studio won't make this." It is never retried and there is no way round it. It is described honestly in Help as a narrow check, not a filter.
  - For models whose licence asks users to filter or review outputs (FLUX.3 Kommunity), the gallery's preview-before-export is the review step, and the licence line says so.
- **Labels in the files.**
  - Every local result carries a machine-readable AI label: IPTC `DigitalSourceType = trainedAlgorithmicMedia`, plus "Made with harnessUI 0.1.0, <model> <revision>", a unique id and the date. It goes in XMP for PNG and WebP and in tags for WebM. **The prompt is never included.**
  - This differs from cloud results, which are "kept exactly as the company sent them". Here there is no company, and harnessUI is the only party that can label.
  - EU AI Act Art. 50(2) applies from 2 Aug 2026, California's law was widened by SB 1000, and the LTX and FLUX licences ask for disclosure.
  - A signed C2PA manifest (c2pa-rs, MIT or Apache) and an invisible watermark (Meta Video Seal, MIT) come in 0.1.x, after legal review.
  - The viewer notes "Labelled as AI-made in the file".

**Technical detail**
- **Changes to the plan's types** (all small, best made before Phase A lands):
  - `Company::Local` (serialised `"local"`).
  - `Price::Free` in `crates/agent/src/media/price.rs`.
  - A default `async fn cancel(&self, remote_id) -> Result<(), MediaError> { Ok(()) }` on `MediaProvider`. Cloud keeps the no-op; local kills the process.
  - New `FailureKind`s: `OutOfMemory`, `EngineMissing`, `EngineFailed`, `ModelFileBad`, `DeviceLost`. Local failures always have `charged: Some(false)`.
  - `StartRequest.provider_id` becomes `target: Target { Endpoint(i64) | ThisComputer }`.
  - `ProviderSource` gains `local()`.
  - `RunDto` gains `runs_on` and `eta`.
- **Spending rules.** `spend::confirm_reasons` and `check_limit` return nothing for `Price::Free`. Tests pin that a local run with `confirmed = false` starts, and that it adds $0 to "spent today".
- **V3 additions** (amend design §5 before Task A8; V4 if V3 has already shipped in a build):
  - On `creation_runs`: `runs_on TEXT NOT NULL DEFAULT 'cloud' CHECK (runs_on IN ('cloud','local'))`, `gen_model_id TEXT`, `engine_build INTEGER`, `device TEXT` and `elapsed_ms INTEGER`.
  - Add `'running'` to the `status` CHECK.
  - A local run has `provider_id` NULL, `provider_name` "This computer" and `provider_host` ''. `runs_on` keeps "endpoint deleted" and "local" apart.
- **Labelling** happens in `src-tauri/src/studio/label.rs` (new) during `gallery.finalize`. It writes an XMP iTXt chunk for PNG, an XMP chunk for WebP, and a Matroska Tags element for WebM, with no new dependency. It is tested by reading the files back.
- **The prompt check** is a pure function in `src-tauri/src/studio/prompt_check.rs`, with a table test of examples it must and must not refuse.

---

## L5. Technical architecture, mapped to files

```
 webview                                   Rust (src-tauri)                               crates
 -------                                   ----------------                               ------
 ModelPicker("On this computer") ──ipc──►  studio/commands.rs ──► StudioManager
 LocalModels / DownloadLocalModel          studio/engine_cmd.rs     ├─ cloud lanes ─────► agent::media (OpenRouter, OpenAI)
 RunCard(local stages, Stop)               studio/local_cmd.rs      └─ local lane (1) ──► studio/local.rs: LocalMediaProvider
 PromptDock(~time), LocalMark                  │                         │   gpu_lease.rs ◄─► RuntimeManager (unload, hold chat loads)
 Settings → Studio (engine, card policy)       │                         └─► runtime::sd (spawn sd-cli, args, progress, failure)
                                               ├─► downloads.rs (GroupMeta.generator, register_gen_model)
                                               ├─► models::catalog (generators) + models::gen_fit + gen_eta
                                               └─► store (creation_runs V3 + gen_models + license_acceptances)
```

**`LocalMediaProvider` behind the plan's `MediaProvider` trait** (new `src-tauri/src/studio/local.rs`; it lives in src-tauri because it needs the lease and the runtime):

```rust
pub struct LocalMediaProvider { engine: Arc<runtime::sd::Engine>, catalog: Arc<GenCatalog>,
                                store: store::Store, runs: Mutex<HashMap<String, LocalRun>> }
#[async_trait] impl MediaProvider for LocalMediaProvider {
    fn company(&self) -> Company { Company::Local }
    fn host(&self) -> String { String::new() }            // no host; the UI shows the local mark
    async fn fetch_catalog(&self) -> Result<RawCatalog, MediaError>;  // installed gen_models + signed catalog; never the network
    async fn fetch_image_price(&self, _: &str) -> Result<Value, MediaError> { Ok(Value::Null) }
    fn preview(&self, req: &MediaRequest) -> RequestPreview;          // "See the exact command" (argv)
    async fn start(&self, req: &MediaRequest, scratch: &Path) -> Result<Started, MediaError>; // spawn → Job{ remote_id: "local:<run>" }
    async fn check(&self, remote_id: &str) -> Result<JobState, MediaError>;   // in-memory progress from progress.rs
    async fn download(&self, remote_id: &str, job: &JobDone, i: usize, dest: &Path) -> Result<Fetched, MediaError>; // move → Saved
    async fn cancel(&self, remote_id: &str) -> Result<(), MediaError>;        // kill the tree, delete .partial/<run>
}
```

**The manager's local lane** (`src-tauri/src/studio/manager.rs`):
- One job at a time, separate from the cloud slots (2 pictures and 2 videos per endpoint).
- It takes the lease before `start` ("waiting for the graphics card" is `queued` with a reason).
- It checks every 250 ms with no network and **no `PollPlan` and no 30-minute watch limit**. Events are still at most 4 per second.
- At start-up, `resume_interrupted` turns local `running` and `queued` rows into `interrupted` ("Run again", free).

**Fit** (new `crates/models/src/gen_fit.rs`, reusing `fit::Room`, `SPARE_BYTES` (1 GB), `RAM_RESERVE_BYTES` (2 GB) and the `Target` from `hub::detect`):
- **Weights.** D = diffusion model (D2 = Wan A14B's second expert, used one after the other), T = text encoders, V = decoder.
- **Tokens.**
  - Pictures: ⌈W/16⌉·⌈H/16⌉, with family factors.
  - Video: multiply by latent frames, (F−1)/4+1 for Wan or (F−1)/8+1 for LTX. Wan 2.2 TI2V uses (W/32)(H/32).
- **Activations.** act_dit ≈ a_f + b_f·tokens (flash attention), and act_vae ≈ c_f·W·H·frames, bounded by the tile size when tiling. **Measured `memory[]` points win**, interpolated in tokens. Coefficients per family are a conservative fallback.
- **Verdicts:**
  - **Green:** D+T+V + max(act_dit, act_vae) + SPARE ≤ `room.whole`.
  - **Yellow, one part at a time:** max(T, D+act_dit, V+act_vae) + SPARE ≤ whole, and the rest fits RAM less the reserve. This adds `--offload-to-cpu` or `--params-backend te=cpu`.
  - **Yellow, slow:** D+act_dit ≤ `room.partial`.
  - **Red** otherwise.
- **Suggestion:** the largest green preset (512²/768²/1024²; 17/33/49/81/121 frames), shown as "Fits at 832×480, 49 frames".
- **With a chat model loaded,** compute twice, with and without `RuntimeManager::freed_memory()` / `device_bytes()` added back, which gives "Fits after the chat model is unloaded".

**Time estimate** (new `crates/models/src/gen_eta.rs`):
- ETA = load + text + steps × s_per_step(tokens) + decode(frames, resolution) + save.
- **The first guess** comes from the entry's `speed[]` for the nearest device class (backend + memory size), scaled by tokens^1.3 for unmeasured presets [E]. It is shown as a range.
- **After a run,** this computer's measured `elapsed_ms`, steps and size from `creation_runs` (local, done) replace the guess for that model and device. No new table is needed.
- **During a run,** the live `sec_per_step` from `progress.rs`.
- **Rounding:** to 5 s under a minute, 30 s under 10 minutes, otherwise whole minutes. Fanless Macs add a 1.3× pad to measured video times until the pad itself has been measured.

**Downloads and store:**
- `src-tauri/src/downloads.rs`:
  - `GroupMeta.generator: Option<{catalog_id, parts: [{role, file}]}>`, serde-default.
  - `register_model` branches to `register_gen_model` before the "finished download has no model file" error (line 1297).
  - Files land at `<models>/<owner>/<repo>/<path>`, so a shared encoder downloads once.
  - Delete counts uses across `gen_models` and `models`.
- `download_file_checked` already accepts any `FileSpec`, safetensors included.
- **Licence enforcement.** New command `studio_local_download(entry_id, accepted_license_sha256s)` refuses unless every part's pinned licence hash is accepted and the content notice is set. Acceptances (licence id, sha256, catalog id, time) go in a `license_acceptances` table. A new pinned revision that changes a licence file asks again.
- **Store.** In V3 (or V4): `gen_models(id, catalog_id, name, kind, family, parts JSON, size_bytes, added_at, source)` and `license_acceptances(...)`, with accessors in new `crates/store/src/gen_models.rs`.
- **Models dialog.**
  - `src/components/models/Installed.tsx`: one line, "Picture and video models: 3 (21.4 GB) · Manage in Studio".
  - Importing a GGUF whose `general.architecture` is `flux`, `wan` and so on says "This is a picture or video model. Add it in Studio". This uses `arch.rs::is_diffusion` with `catalog/llama-architectures.txt`.
  - `Downloads.tsx` labels generator groups.
  - Before each run, `gguf::read_file` confirms that the diffusion file's architecture matches the declared family.

**Commands** (each in `src-tauri/commands.list`, `allow-<cmd>` in `capabilities/main.json` and `generate_handler!`, each checking `only_main_window`):
- `studio_engine_status`, `studio_engine_download`, `studio_engine_cancel`, `studio_engine_remove`.
- `studio_local_catalog`, `studio_local_fit(entry, w, h, frames)`, `studio_local_download`, `studio_local_delete`.
- `studio_set_gpu_policy` is part of `studio_set_prefs`.

**Tests:**
- **A fake engine.** `crates/runtime/tests/sd_cli.rs` re-runs its own test binary with a hidden flag (`--runtime-test-fake-sd`), the pattern already used by `tests/reaper_process.rs`. In that mode it acts as `sd-cli`:
  - it replays progress fixtures captured from the pinned build (`tests/fixtures/sd_progress_*.txt`, with `\r` redraws), like the existing `managed_stdout.txt` and `fit_params_*.jsonl`;
  - it writes a tiny PNG or WebM to `-o`;
  - it records its argv to a JSON file;
  - on request it simulates out-of-memory text and exit code, a crash, a hang, a missing DLL message and a damaged model.
- **What the fake engine tests check:**
  - roles map to fixed flags and nothing else;
  - a prompt starting with `-` is never read as a flag;
  - the prompt cap;
  - progress mapping, including `\r` splitting and two-pass Wan;
  - Stop kills the tree within 1 s and removes `.partial`;
  - one retry after out of memory, then a named failure;
  - the stall message;
  - `interrupted` after a restart.
- **Real contract tests** on Linux CI run the pinned `dev.linux-x86_64` `sd-cli` with a tiny test model fetched by a `fetch_test_model.py`-style script (its licence checked for CI use). They check flag names against `-h`, the progress format and the output file types.
- **Unit tests:**
  - catalog `generators` parsing, hidden on unknown family, role, quirk or `minBuild`;
  - the 0.0.5-parser compatibility test;
  - `.ckpt` refused;
  - strictest-licence calculation;
  - `gen_fit` table tests (green, yellow, red, "after unload", suggestions);
  - `gen_eta` first guess vs measured;
  - lease outcomes with a fake RuntimeManager;
  - spend rules skipping `Price::Free`;
  - label read-back;
  - prompt check;
  - V2 → V3 upgrade with the new columns.
- **Frontend (vitest):**
  - the picker group and empty state;
  - Get models fit filtering and "Not for this computer";
  - the licence dialog blocks until accepted;
  - Create shows time, not price, with no confirm dialog;
  - no Cloud mark on local runs;
  - Stop wording.
- **e2e:** m8_offline makes a local picture with all requests refused, using the fake engine on CI and the real one by hand.

**Docs:**
- ARCHITECTURE.md: 7.1 (sd pin), a new 7.9 "Local generation", 12 (gen_models), 13 (no local server, the argv prompt, refused pickle formats), 13.6 (network: huggingface.co and GitHub on click, nothing during a run), 16 (Windows installer unchanged; Mac adds `sd-cli`) and 17.
- SECURITY.md, README.md, CHANGELOG.md.
- `scripts/notices.py` and THIRD-PARTY-NOTICES: stable-diffusion.cpp, ggml, libwebp and libwebm (BSD-3), Oniguruma (BSD-2), utf8proc and the Unicode data licence, cpp-httplib, nlohmann json and others, plus "the Studio engine is not part of the Windows installer".
- docs/SPEC.md line 121 note.
- The website's terms page gets a section "Pictures and videos you make".

---

## L6. Scope, effort and risks

### In 0.1.0 (if Decision 1 = A)
- The Studio engine: Windows Vulkan+CPU pack downloaded with the first model; Mac Metal+CPU bundled.
- Get local models, the licence dialog, the content notice, fit and time estimates, the Models dialog lines.
- Local pictures: FLUX.2 klein 4B and Z-Image Turbo, including Edit with words on Klein.
- Local video: Wan 2.2 5B fast, 480p by default, 720p offered, start frame (Animate).
- One local job at a time, Stop, the out-of-memory retry, the graphics-card lease with automatic unload, quit confirmation.
- Local runs in the same cards and gallery, with time instead of price, no confirmation or limit, no Cloud mark, AI labels in the files, the prompt check.
- Tests with the fake engine, Linux contract tests, the offline e2e test, docs and notices.

### Later (0.1.x / 0.2)
1. The CUDA engine pack (about 930 MB), then ROCm.
2. `sd-server` mode with a key (keeps models loaded), and live previews.
3. LTX-2.3 with sound (after Lightricks confirms), Wan 2.2 A14B, Qwen-Image 2512, FLUX.3 klein if not in 0.1.0, Wan 2.1 1.3B if not in 0.1.0.
4. Adding picture and video models from Hugging Face search, matched to the catalog's shared encoders and decoders, with repos tagged "not for all audiences" hidden.
5. C2PA signing and an invisible watermark, plus a "Check a file" verifier.
6. Keep awake while making; a battery warning.
7. MP4 export through the system's own encoders (AVFoundation, Media Foundation).
8. LoRAs, upscaling (SeedVR2), "Make full version" from a 480p draft.
9. A Swift/MLX Mac engine (0.2), if the benchmark shows sd.cpp's Metal is far behind.

### Rough effort

| Part | Person-days |
|---|---|
| One-day benchmark on the owner's machines plus a Windows GPU; choosing the pin and quant levels | 2 |
| Engine pin and CI: `sd.lock.json`, `fetch_sd.py`, Mac universal static build and `check_macos_app`, Windows pack, notices, verify-packs | 4–5 |
| `runtime::sd`: spawn, args, `\r` progress, failures, devices, Stop, the out-of-memory retry | 6–8 |
| Catalog `generators` and licence schema, validation, compatibility test; `gen_fit`; `gen_eta` | 6–8 |
| Store and downloads: `gen_models`, V3 columns, `GroupMeta.generator`, shared parts, delete counting, licence acceptance | 3–4 |
| `LocalMediaProvider`, the manager's local lane, `gpu_lease` and the RuntimeManager hooks | 5–6 |
| Webview: picker group, Get local models, licence and notice dialogs, local run card, time chips, Settings → Studio block, Models dialog lines | 6–8 |
| AI labels in files, prompt check, content notice | 2–3 |
| Tests (fake engine, contract, e2e), docs, notices, terms page | 4–5 |
| **Total** | **about 38–49 (8–10 person-weeks; about 4–5 more calendar weeks for two engineers)** |

About 6–9 of these days are video-specific: frames and fps, the WebM build and playback check, temporal tiling, video fit and time, measuring the Wan entries, picture-to-video. Local pictures alone come to about 32–40.

### Risks and how the design handles them
- **Default video speed rests on two unconfirmed facts:**
  - whether a 4-step Wan 2.2 5B version (base GGUF plus a Turbo add-on, or FastWan) loads in sd.cpp, and under what licence;
  - whether Metal on the M5 Air is usable.

  Without the 4-step version, video is about 10× slower. *Handling:* the benchmark (L7) decides before any UI wording is fixed. If it fails on the Air, video ships for graphics cards and the Air shows "Slow here" or "Not for this computer".
- **Upstream churn** (rolling builds; the latest is an API refactor). *Handling:* pin the build, keep every assumption inside `runtime::sd`, run contract tests on fixtures, HEAD-check pack URLs, and mirror assets to harnessUI's release if upstream deletes them.
- **Fanless Air throttling** makes long clips slower than short-clip measurements suggest. *Handling:* 480p and 2 s by default, a pad on measured video times, the amber note over 15 minutes.
- **Graphics-card contention** with a chat model, and Vulkan freeing memory late. *Handling:* the lease, automatic unload with a visible note, held chat loads, a re-probe or margin before reloading.
- **DLL collisions** between sd.cpp's and llama.cpp's `ggml*.dll`. *Handling:* separate folders, plus a test that installs both.
- **Fit accuracy.** *Handling:* measured peak points per entry, conservative fallbacks, and one automatic retry with memory saving.
- **Security:**
  - The prompt is in argv: it is capped and passed in `--prompt=` form.
  - No local server without a key.
  - Pickle formats are refused, and every part is pinned by SHA-256.
  - Roles map to fixed flags; quirks come from a compiled-in whitelist.
  - The working directory is the engine folder and the environment is scrubbed.
- **Licences.** *Handling:* 0.1.0 starters are all Apache, ungated, with Apache decoder copies. Licences are taken from upstream, the strictest part applies, and acceptance is recorded per licence hash. Amber and red models are behind an explicit decision.
- **Legal labelling** (EU AI Act Art. 50(2), California SB 942 as amended by AB 853 and SB 1000). *Handling:* IPTC/XMP labels in 0.1.0; C2PA and watermark after legal review. **Get legal review before release** on whether harnessUI counts as a "provider", and whether a curated download catalog counts as a "GenAI hosting platform" (duties from 1 Jan 2027).
- **Video playback.** WebM (VP8) in WKWebView is unverified. *Handling:* the benchmark checks it; animated WebP is the fallback.
- **Disk.** Each model is 5–13 GB, in the visible models folder. *Handling:* sizes shown before download, the free-space check, correct counting of shared parts on delete, and disk use in Settings → Studio.
- **Expectations.** Local video is shorter, smaller and simpler than Veo or Kling in the cloud. *Handling:* the Get models view says so in one line: "Local video is slower and simpler than cloud video, but free and private."
- **The owner's Windows GPU is unknown.** With only an integrated GPU or none, local video is unavailable and pictures take minutes. *Handling:* the header shows what was detected, and the benchmark records the card.

---

## L7. The one-day benchmark (before any time is promised in the UI)

Run the pinned build on the owner's MacBook Air M5 and Windows PC, plus one 8 GB and one 12 GB card if available, and record:
1. **Mac:** whether Metal runs from the universal build. Measure FLUX.2 klein 4B, Z-Image Turbo (tensor path on and off) and Wan 2.2 5B fast at 2 s and 4 s 480p and 5 s 720p, then a second 5 s 720p run straight after the first, to see throttling.
2. **The 4-step Wan version:** whether the 4-step versions load in sd.cpp, and their licences. Fallback: base GGUF plus Turbo add-on, or Wan 2.1 1.3B as the 0.1.0 video model.
3. **Windows:** Vulkan speed on the owner's card, and CUDA for comparison if it is NVIDIA.
4. **Memory:** peak memory per preset, for the catalog's `memory[]`, and seconds per step, for `speed[]`.
5. **Playback:** WebM (VP8) in WKWebView and WebView2.
6. **Sharing:** sd-cli running next to a loaded 4B and 9B chat model.
7. **Metadata:** whether `--disable-image-metadata` also strips the prompt from WebM.

---

## L8. Open decisions for the owner (3)

**Decision 1: When does local video ship?**
- **A. In 0.1.0, with one model, Wan 2.2 5B fast (recommended).** Local pictures and video arrive together, with video gated by the benchmark: if a 2-second 480p clip takes more than about 10 minutes on the Air, 0.1.0 offers local video on graphics cards only. 0.1.0 grows by about 38–49 person-days (about 4–5 calendar weeks for two engineers).
- B. Local pictures in 0.1.0, local video in 0.1.1. This saves about 6–9 person-days (about a week) in 0.1.0, but the thing you asked for waits.
- C. 0.1.0 ships the cloud Studio exactly as planned. Local pictures and video come together in 0.1.1, about 4–5 weeks later.
- *Trade-off:* the engine, downloads, fit, lease and UI are shared, so splitting pictures from video saves little and leaves your request half done. A is the most direct answer to "local models that can generate video". C is right if 0.1.0's date matters more than having local models in it.

**Decision 2: How does the Windows engine get onto the computer?**
- **A. Downloaded with your first local model (recommended).** One button: "Download model and engine (+29 MB)". It is pinned by SHA-256 like the ROCm and CUDA packs. The installer stays about 50 MB, and people who never use local Studio never download it.
- B. Bundled in the installer. The installer grows to about 75–80 MB [E], still under the 150 MB cap. Local Studio then works right after install, with no extra step, but every user downloads the engine on every update.
- (The Mac bundles it either way: it must be built, signed and notarized with the app, and the Mac has no size cap.)
- *Trade-off:* local models are 5–13 GB downloads anyway, so an extra 29 MB in the same step is invisible. Bundling buys "no download step" at the cost of every update for everyone.

**Decision 3: Which starter models?**
- **A. Only models that are free for any use, Apache-2.0 and ungated (recommended for 0.1.0):** FLUX.2 klein 4B, Z-Image Turbo and Wan 2.2 5B fast (plus Wan 2.1 1.3B if the benchmark calls for it). More can be added later through a signed catalog update, with no app release.
- B. A, plus **FLUX.3 image klein** (Kommunity licence: commercial use only under $5M revenue; outputs must be filtered or reviewed before sharing). It is the newest model, with the best text in pictures, shown with an amber badge and that line in the download dialog.
- C. B, plus non-commercial "best quality" models (Qwen-Image 2.1, FLUX.2 klein 9B) behind "Show non-commercial models".
- *Trade-off:* A means no licence surprises and no Hugging Face login, and every picture can be used for anything. B adds a clearly better model for text and editing, at the cost of a licence most people will not read closely. C adds the best open quality, with terms that rule out even some hobby use (Qwen 2.1 is "research or evaluation only"). LTX-2.3 (video with sound) is not in any option until Lightricks confirms the "directly competes" clause in writing.

*Treated as decided, unless the owner says otherwise:*
- stable-diffusion.cpp is the engine on Windows and macOS; MLX is only a possible 0.2 Mac fast path.
- One `sd-cli` process per job, with no local server until it can require a key.
- When a local job needs the graphics card, the chat model is unloaded automatically, with a visible note and "Load it again". Settings → Studio can change this to Ask or Processor.
- In 0.1.0, local models come only from the signed catalog; there is no free-form Hugging Face search for picture and video models.
- Every local result carries an AI label in the file, without the prompt. HunyuanVideo, MiniMax-H3 and uncensored models are never in the catalog.

---

Relevant files:
- /home/user/harnessUI/docs/dev/0.1.0/studio-design.md: the main design this extends (V3 in §5, `MediaProvider` in §8).
- /home/user/harnessUI/docs/dev/0.1.0/studio-plan.md: the trait at about line 199, `StartRequest` at about line 1403, Task A8's V3.
- /home/user/harnessUI/llama.lock.json and /home/user/harnessUI/scripts/fetch_llama.py: `check_macos_app` at line 388, `verify_packs` at line 426.
- /home/user/harnessUI/src-tauri/tauri.macos.conf.json: `externalBin`.
- /home/user/harnessUI/crates/runtime/src/packs.rs: `ALLOWED_HOSTS` at line 22, `download_verified` at 195, `unpack_zip` at 391, `install` at 467.
- /home/user/harnessUI/crates/runtime/src/llama/lines.rs: line 29.
- /home/user/harnessUI/crates/runtime/src/llama/fit_params.rs.
- /home/user/harnessUI/crates/runtime/src/llama/backend.rs.
- /home/user/harnessUI/crates/runtime/tests/reaper_process.rs: the hidden-flag fake-binary pattern.
- /home/user/harnessUI/crates/models/src/catalog.rs: `validate` at line 274, `.gguf` check at 294, the v005 compatibility parser at about 652.
- /home/user/harnessUI/crates/models/src/fit.rs: `SPARE_BYTES` at line 14, `RAM_RESERVE_BYTES` at 22.
- /home/user/harnessUI/src-tauri/src/runtime_manager.rs: `loaded_on_device` at 227, `freed_memory` at 252, `before_change` at 316, `unload_by_user` at 365.
- /home/user/harnessUI/src-tauri/src/downloads.rs.
- /home/user/harnessUI/src-tauri/src/hub.rs.
- /home/user/harnessUI/src-tauri/src/packs_cmd.rs.
- /home/user/harnessUI/src/components/BackendDownloads.tsx.
- /home/user/harnessUI/src/components/CloudMark.tsx and /home/user/harnessUI/src/components/FitBadge.tsx.
- /home/user/harnessUI/catalog/models.json: schemaVersion 1, sequence 2.
- /home/user/harnessUI/.github/workflows/ci.yml: the 150 MB check at line 260.
- /home/user/harnessUI/ARCHITECTURE.md: D1 at line 49 ("about 50 MB" installer).

Upstream facts checked on 2026-11-17:
- stable-diffusion.cpp release master-1106-532d2c1.
- examples/server/README.md: network options are only `--listen-ip`, `--listen-port`, `--max-pending-jobs` and `--completed-ttl-seconds`; there is no key or auth.
- examples/cli/README.md: outputs embed a generation record; `--disable-image-metadata` omits it.

Scratch copies of those READMEs:
- /tmp/claude-0/-home-user/f8f6f1ef-33e4-57b5-aed7-1e790c206f77/scratchpad/sdapi/README.md
- /tmp/claude-0/-home-user/f8f6f1ef-33e4-57b5-aed7-1e790c206f77/scratchpad/sdapi/api.md
- /tmp/claude-0/-home-user/f8f6f1ef-33e4-57b5-aed7-1e790c206f77/scratchpad/sdapi/cli.md