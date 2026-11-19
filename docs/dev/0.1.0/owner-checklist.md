# 0.1.0: what only the owner's computers can show

Studio's local models need measurements and checks that the build machine cannot make: it
has no Mac, no Windows PC and no graphics card. This file starts with the one-day benchmark,
which comes first because its results decide what the app and the documents may say. The
checks of the finished app (cloud and local real runs) follow it.

## Before you tag v0.1.0

The release waits for all of these (the plan's one gate is Task L17). Nothing in CI or
`release.yml` checks them, so tick them here.

- [ ] **The benchmark is applied.** The **Results** table below is filled in (rows 1–8), and
      Task L17 has written it into the catalog: every generator in `catalog/models.json` has
      its measured `speed` points (today every one is `speed: []`), the video entry is
      confirmed or replaced, and the catalog is re-signed.
- [ ] **The real runs are done.** The **Local real runs** on the MacBook Air and the Windows
      PC, and the **Cloud real runs**, with their answers recorded as fixtures.
- [ ] **Legal review of the AI labels.** Whether harnessUI counts as a "provider", and the
      curated download list as a "GenAI hosting platform", under EU AI Act Art. 50(2) and
      California SB 942 as amended ([studio-local-design.md](studio-local-design.md), "Get
      legal review before release").
- [ ] **The website goes with it.** The website's privacy and terms pull request (the
      `harnessUI-website` repository, the branch of the same name) is merged on release day,
      with the terms' "0.0.x are early test builds" line brought up to date.
- [ ] **The CHANGELOG's 0.1.0 heading carries the release day.**
- [ ] **The updater key is in the repository's secrets**, as for every release since 0.0.8.

## Local benchmark (do this before any time is shown or written)

The one-day benchmark of addendum L7 ([studio-local-design.md](studio-local-design.md)). Until
its **Results** table below is filled in and applied (plan, Task L17), the app shows "Time
shown after your first run here" and no local time appears in the app, the README, the
CHANGELOG or the website.

### What you need

- **The engine: the pinned build only**, `master-1106-532d2c1` (build 1106, `sd.lock.json`).
  - **Mac:** `sd-cli` inside the app from a CI disk image of this branch (the `macos-dmg`
    artifact), installed as usual: `/Applications/harnessUI.app/Contents/MacOS/sd-cli`.
  - **Windows:** the engine harnessUI installs with the first local model, in
    `%LOCALAPPDATA%\io.github.sugarhappy8.harnessui\engines\sd-1106\`. Before the app can
    install it: download `sd-master-532d2c1-bin-win-vulkan-x64.zip` from the
    [release page](https://github.com/leejet/stable-diffusion.cpp/releases/tag/master-1106-532d2c1),
    check that `certutil -hashfile sd-master-532d2c1-bin-win-vulkan-x64.zip SHA256` prints
    `3e66222c87ca27074e1fd8ce79706eef17a8aaf39949906aa1bee7e060f22ecc`, unzip it into a new
    folder and copy `msvcp140.dll`, `msvcp140_codecvt_ids.dll`, `vcomp140.dll`,
    `vcruntime140.dll` and `vcruntime140_1.dll` next to `sd-cli.exe` from the installed app's
    `llama\main` folder (in the folder harnessUI was installed into, where
    `src-tauri/tauri.windows.conf.json` puts llama.cpp's files; not a source checkout's
    `resources\llama\main`).
- **The model files**, as the provisional starter entries pin them (plan, Task L7): each
  downloaded once into one folder, run from that folder.
  - FLUX.2 klein 4B: `flux-2-klein-4b-Q4_0.gguf` (`leejet/FLUX.2-klein-4B-GGUF`),
    `Qwen3-4B-Q4_K_M.gguf` (`unsloth/Qwen3-4B-GGUF`) and `split_files/vae/flux2-vae.safetensors`
    (`Comfy-Org/Flux-2-Klein-4B`).
  - Z-Image Turbo: `z_image_turbo-Q4_K.gguf` (`leejet/Z-Image-Turbo-GGUF`), the Q4_K_M file of
    `unsloth/Qwen3-4B-Instruct-2507-GGUF` and `split_files/vae/ae.safetensors`
    (`Comfy-Org/Z-Image-Turbo`).
  - Wan 2.2 5B, fast: `FastWan2.2-TI2V-5B-q6_k.gguf` and the Q5_K_M file of
    `Green-Sky/FastWan2.2-TI2V-5B-FullAttn-GGUF`, `umt5-xxl-encoder-Q5_K_M.gguf`
    (`city96/umt5-xxl-encoder-gguf`) and `VAE/Wan2.2_VAE.safetensors`
    (`QuantStack/Wan2.2-TI2V-5B-GGUF`).
  - Only if item 2 needs it, Wan 2.1 T2V 1.3B: `split_files/diffusion_models/wan2.1_t2v_1.3B_fp16.safetensors`
    (`Comfy-Org/Wan_2.1_ComfyUI_repackaged`), and the decoder the catalog pins,
    `split_files/vae/wan_2.1_vae.safetensors` from `Comfy-Org/Wan-2.1` at revision `123acf1`
    (`catalog/models.json`, family `wan2.1-t2v-1.3b`), with the same text encoder as Wan 2.2.
- **How to run and record.** One command per run, from the model folder, written to a file with
  the engine's normal and error output together, as the app reads them:
  - Mac (Terminal): `"$SD" <arguments> > run.txt 2>&1`, with
    `SD=/Applications/harnessUI.app/Contents/MacOS/sd-cli`. For a measured run put
    `/usr/bin/time -l` before `"$SD"`: its report lands at the end of the same file (read
    "peak memory footprint").
  - Windows: **Command Prompt**, not PowerShell (PowerShell re-encodes the output and loses the
    `\r` redraws): `"%SD%" <arguments> > run.txt 2>&1`, with `SD` set to the engine's
    `sd-cli.exe`. Peak memory: the highest "Dedicated GPU memory" in Task Manager →
    Performance → GPU while it runs, and `sd-cli.exe`'s memory in Task Manager → Details.
  - Write the prompt into `prompt.txt` (no newline at the end) and pass it with
    `--prompt-file prompt.txt`, as the app does. Always add `--seed 42 --disable-image-metadata`.
    Run without `-v`.
  - Read from each file: the load time ("loading tensors completed, taking … s"), seconds per
    step (the progress line's `s/it` at the end of sampling, or "sampling completed, taking … s"
    divided by the steps), the decode time ("decode_first_stage completed, taking … s"), and
    the whole run's seconds. Close other apps; note the chat model's state for each run.
- **The arguments for each starter** (the provisional defaults; item 2 may change the video
  ones):
  - FLUX.2 klein 4B: `--diffusion-model flux-2-klein-4b-Q4_0.gguf --llm Qwen3-4B-Q4_K_M.gguf --vae flux2-vae.safetensors --cfg-scale 1.0 --steps 4 --sampling-method euler -W 1024 -H 1024 -o klein.png`
  - Z-Image Turbo: `--diffusion-model z_image_turbo-Q4_K.gguf --llm Qwen3-4B-Instruct-2507-Q4_K_M.gguf --vae ae.safetensors --cfg-scale 1.0 --steps 8 -W 1024 -H 1024 -o zimage.png`
  - Wan 2.2 5B fast, 2 s at 480p: `-M vid_gen --diffusion-model FastWan2.2-TI2V-5B-q6_k.gguf --t5xxl umt5-xxl-encoder-Q5_K_M.gguf --vae Wan2.2_VAE.safetensors --cfg-scale 1.0 --steps 4 --flow-shift 3.0 --sampling-method euler -W 832 -H 480 --video-frames 49 --fps 24 -o clip.webm`;
    4 s: `--video-frames 97`; 5 s at 720p: `-W 1280 -H 704 --video-frames 121`.
  - Wan 2.1 1.3B, only if item 2 needs it (its family defaults, about 2 s at 480p): `-M vid_gen --diffusion-model wan2.1_t2v_1.3B_fp16.safetensors --t5xxl umt5-xxl-encoder-Q5_K_M.gguf --vae wan_2.1_vae.safetensors --cfg-scale 6.0 --steps 20 --flow-shift 3.0 --sampling-method euler -W 832 -H 480 --video-frames 33 --fps 16 -o clip21.webm`.

### The runs

1. **Mac** (the MacBook Air M5; write down its memory size). Metal from the universal build:
   `"$SD" --list-devices` names the Metal device, and each run below uses it. Measure FLUX.2
   klein 4B and Z-Image Turbo at 1024×1024 (Z-Image twice: once with
   `GGML_METAL_TENSOR_DISABLE=1` before the command and once without), and Wan 2.2 5B fast at
   2 s and 4 s at 480p and 5 s at 720p, then a second 5 s 720p run straight after the first
   (it shows throttling: the Air has no fan).
2. **The fast Wan 2.2 version.** The provisional entry (Task L7) is FastVideo's
   FastWan2.2-TI2V-5B (a DMD distillation, Apache-2.0) as Green-Sky's GGUF. hum-ma's Turbo is
   not used unless its base (`quanhaol/Wan2.2-TI2V-5B-Turbo`) states a licence. Check that it
   loads in the pinned build, take its steps (3 or 4), cfg and flow shift from its model card,
   and compare Q5_K_M with Q6_K (time, and how sharp the clip looks). **Decides (Task L17):**
   - it loads: it stays, with those defaults;
   - no fast version loads: the video entry is `wan21-t2v-1.3b` alone;
   - it loads, but a 2 s 480p clip on the Air takes more than 10 minutes, or it does not fit a
     6 GB card at 480p: `wan21-t2v-1.3b` is **added** as a fourth entry, tiered for Apple
     silicon and small cards, and the Wan 2.2 entry gets `"runsOnApple": false` only when Wan
     2.1 runs acceptably on the Air (measure it as in item 1 when this applies).
3. **Windows.** Vulkan speed on the owner's graphics card for every starter (as in item 1;
   `"%SD%" --list-devices` names the card), and, if it is an NVIDIA card, the same runs with
   CUDA for comparison (a CUDA build of the same release from its release page, run by hand;
   CUDA is not offered in 0.1.0). Then one 8 GB and one 12 GB card when they are available.
4. **Memory and speed.** For each starter entry's `memory[]` and `speed[]`: the peak memory of
   every preset run above, and seconds per step, load and decode seconds, each speed point with
   `measuredOn` (the date and the machine). Also make this build run out of memory once on
   purpose (Wan 2.2 at 720p on the smallest card, or with other programs holding the memory)
   and keep that output for Task L3's patterns.
5. **Playback.** Open a WebM (VP8) clip from item 1 in harnessUI's viewer on the Mac (WKWebView)
   and on Windows (WebView2): does it play and seek? **Decides:** if WKWebView cannot play it,
   the video entries get the `video-webp-on-mac` quirk (Task L6).
6. **Sharing.** Run FLUX.2 klein 4B while harnessUI has a 4B chat model loaded, then a 9B one
   (on the Mac and on the Windows card): does it run, how much slower, and how much memory is
   left. This checks Task L12's "fits alongside" margin.
7. **One shared encoder.** Does one Qwen3-4B GGUF serve both FLUX.2 klein and Z-Image? Run
   Z-Image with `Qwen3-4B-Q4_K_M.gguf` as its `--llm` and compare the picture with the
   Instruct-2507 run (Comfy-Org ships the identical `qwen_3_4b.safetensors` for both). If it
   does, Task L17 gives both entries the same `llm` part, so the second starter downloads
   2.5 GB less.
8. **Fixtures.** Record these as raw bytes (the redirections above keep `\r` and `ESC[K`, with
   both outputs interleaved as they arrive), into `crates/runtime/tests/fixtures/`:
   - `sd_progress_flux2_klein.txt` (a FLUX.2 klein run) and `sd_progress_wan22_ti2v.txt` (a
     2 s Wan 2.2 run), over the hand-written ones;
   - `sd_stderr_out_of_memory_vulkan.txt` (item 4's out-of-memory run on Windows), over the
     hand-written one;
   - `sd_list_devices_vulkan_recorded.txt` (`--list-devices` on the Windows PC), as a **new**
     file: the hand-written `sd_list_devices_vulkan.txt` stays, because tests depend on its
     card;
   - `sd_list_devices_metal.txt` (`--list-devices` on the Mac), over the hand-written one;
   - `sd_help.txt` (`"$SD" -h`): it is already a recording from the pinned processor-only
     build, so record it again from the release build only if its output differs;
   - `sd_fixtures_build.txt`, holding the build's tag: `master-1106-532d2c1`.

   The other hand-written fixtures stay as they are.

Settled from the pinned source, so not measured: `--disable-image-metadata` also clears WebM
metadata; `-h` exits 0; `--list-devices` prints one `name<TAB>description` line per device, the
names `--backend` takes; the engine can write live previews (`--preview`, `--preview-path`),
which are left for 0.1.x.

### Results

Fill in one row per item, on the owner's machines. Task L17 applies each row and marks it
applied; a row left empty keeps the provisional value it names.

| Item | What to record | Result | Decides (Task L17) | Applied |
|---|---|---|---|---|
| 1. Mac | Memory size; Metal used (yes/no); per run: whole seconds, load, s/step, decode, peak memory. FLUX.2 klein 4B 1024²; Z-Image Turbo 1024² with and without `GGML_METAL_TENSOR_DISABLE=1`; Wan 2.2 fast 2 s 480p, 4 s 480p, 5 s 720p, the second 5 s 720p | | The Apple-silicon `speed[]` and `memory[]` points; whether Z-Image keeps the `metal-tensor-off` quirk | |
| 2. Fast Wan 2.2 | Loads in build 1106 (yes/no); steps, cfg and flow shift from its model card; Q5_K_M against Q6_K (time, sharpness); fits a 6 GB card at 480p (yes/no) | | The video entry: kept with these defaults and quant, `wan21-t2v-1.3b` alone, or `wan21-t2v-1.3b` added (with `"runsOnApple": false` on Wan 2.2 when Wan 2.1 runs acceptably on the Air) | |
| 3. Windows | The card and its memory; per starter and preset: whole seconds, load, s/step, decode, peak memory, with Vulkan (and CUDA when NVIDIA); the 8 GB and 12 GB cards when available | | The graphics-card `speed[]` and `memory[]` points | |
| 4. Memory and speed | Peak memory per preset and s/step per starter, each with `measuredOn` (date, machine); the out-of-memory output kept | | Every starter's `memory[]` and `speed[]`; a family's fallback raised where a measured peak exceeds it; Task L3's out-of-memory patterns | |
| 5. Playback | WebM (VP8) plays and seeks in WKWebView (yes/no) and in WebView2 (yes/no) | | The `video-webp-on-mac` quirk on the video entries when WKWebView cannot play it | |
| 6. Sharing | FLUX.2 klein 4B beside a loaded 4B and a loaded 9B chat model: runs (yes/no), how much slower, memory left | | Task L12's "fits alongside" margin | |
| 7. Shared encoder | One Qwen3-4B GGUF serves both FLUX.2 klein and Z-Image (yes/no) | | One shared `llm` part for both picture entries | |
| 8. Fixtures | The files of item 8 recorded (list them) | | The recordings replace the hand-written fixtures; `progress.rs` and `failure.rs` change only where a recording differs | |

## Local real runs (the MacBook Air and the Windows PC)

On each computer, from a fresh install of a build of this branch (the CI disk image or
installer). Each item says what must happen; write down anything else that does.

1. **Get local models** shows the right hardware line (the graphics card or the Mac, its memory
   and the free disk) and the recommended list, every entry free for any use.
2. **Download FLUX.2 klein 4B.** On Windows the Studio engine downloads with it, and
   SmartScreen or the antivirus does not block `sd-cli.exe`, or the block is named and the
   download offered again. On the Mac nothing asks for permission and Gatekeeper is silent.
3. **A picture with a chat model loaded.** When it does not fit beside the chat model, the chat
   model is unloaded and the card says "The chat model was unloaded to make room." with *Load
   it again*; loading the chat model again while Studio works waits for it ("Studio is using
   the graphics card. The chat model loads when it finishes.").
4. **Stop a 2 s video halfway.** The card says nothing was charged, and the computer goes quiet
   (no `sd-cli` process left, the fans and the graphics card at rest).
5. **Hugging Face search.** One search that finds a recognised quantization (its parts, the
   total size and the licence are shown before Download), and one that shows "Not supported
   yet" (no Download button).
6. **Quit while a video is being made**, once by closing the window and, on the Mac, once with
   ⌘Q: the question appears both ways. Meanwhile try *Install update*: it is refused with
   "Stop it in Studio, or install the update when it finishes."
7. **Windows without the Visual C++ redistributable** (a PC or a fresh virtual machine where it
   was never installed): the engine starts from its own folder, with the runtime files copied
   next to `sd-cli.exe` (`vcomp140.dll` among them).
8. **The AI label.** `sd-cli -M metadata --image <result>` on a picture Studio made shows the AI
   label (`trainedAlgorithmicMedia`, "Made with harnessUI 0.1.0, …") and not the prompt.

## Cloud real runs

One real run on each company, under $1 in total, in the app:

1. **OpenAI:** a low-quality GPT Image 2.6 Flare picture.
2. **OpenRouter:** a picture, and a 4 s 480p Grok Imagine Video 1.5 Lite video (about $0.08
   without a start frame).
3. **One safety refusal on each company**: the card explains it in plain words, with the
   company, whether it was charged and the request id, and nothing is retried.
4. **The costs:** compare each cost Studio shows with the company's activity page.
5. **Playback:** play an OpenRouter video in the viewer on Windows and on the Mac (it plays from
   a `blob:` address, and seeking works).
6. **How long OpenRouter keeps a finished video:** open its content address again later (an
   hour, a day) until it is gone, and write down how long it lasted.

**Record the real traffic** into `crates/agent/tests/fixtures/media/`, so the shapes Tasks A4
and A5 check come from the companies, not from their documentation. *See the exact request* in
the viewer shows each body Studio sends; to record an answer, send that body again with `curl`
and your key (`-D headers.txt -o answer.json`), within the same $1. Cut every `b64_json` down
to a few bytes, and never save a video's bytes.

- **OpenAI:** the generation answer over `openai-images-answer.json`, and its
  `moderation_blocked` error over `openai-moderation-blocked.json`.
- **OpenRouter** (dated files, new): `openrouter-image-answer-<date>.json` (the picture
  answer); `openrouter-video-create-<date>.json` (the answer that starts the job);
  `openrouter-video-status-<date>.json` (its pending, in_progress and completed answers);
  `openrouter-video-content-<date>.json` (the content endpoint's status and headers:
  `Content-Type`, `Content-Length`, and `Location` when it redirects). Then change the answers
  written inline in `crates/agent/tests/media_endpoints.rs` to the same keys and value types:
  `pictures_answer`, `submitted()`, the pending, in_progress and completed answers in
  `a_video_job_starts_checks_and_downloads` and `signed_urls_are_kept_for_the_guarded_fetcher`,
  and the redirect in `a_link_to_another_host_is_handed_back_not_followed`.
- **The request bodies OpenRouter accepted**, one picture with a reference picture and an
  extra setting and one video with a start frame, as `openrouter-request-accepted-<date>.json`,
  with the same top-level keys as `openrouter-request-examples-2026-11-17.json`. Point the
  `EXAMPLES` constant in `media_endpoints.rs` at it, so `assert_same_shape` checks the recorded
  bodies; the documentation copy stays.

Then run `cargo test -p agent` again, and change a test only where a recording differs from
what it expected.
