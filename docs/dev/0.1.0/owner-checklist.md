# 0.1.0: what only the owner's computers can show

Studio's local models need measurements and checks that the build machine cannot make: it
has no Mac, no Windows PC and no graphics card. This file starts with the one-day benchmark,
which comes first because its results decide what the app and the documents may say. The
checks of the finished app (cloud and local real runs) follow it.

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
    `vcruntime140.dll` and `vcruntime140_1.dll` next to `sd-cli.exe` from harnessUI's
    `resources\llama\main` folder.
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
    and `split_files/vae/wan_2.1_vae.safetensors` (`Comfy-Org/Wan_2.1_ComfyUI_repackaged`),
    with the same text encoder as Wan 2.2.
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
   - `sd_help.txt` (`"$SD" -h`), over the hand-written one;
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
