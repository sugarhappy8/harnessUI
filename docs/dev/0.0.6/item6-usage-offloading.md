# Item 6: usage view and finer offloading controls (sketch, 2026-11-12)

Sketch for the owner's decision; nothing is built. Claims marked *unverified* could not be
checked against official documentation.

## What LM Studio offers (for comparison)
- **GPU offload**: a layer slider, saved per model ([load](https://lmstudio.ai/docs/cli/local-models/load),
  [per model](https://lmstudio.ai/docs/app/advanced/per-model)).
- **Memory estimate and guardrails**: the estimate follows the sliders; a warning before a
  likely out-of-memory load, which can be overridden ([0.3.27](https://lmstudio.ai/blog/lmstudio-v0.3.27)).
- **Force model expert weights onto CPU** ("Try this for low VRAM machines", [0.3.23](https://lmstudio.ai/blog/lmstudio-v0.3.23)).
- **Offload KV cache to GPU memory**, flash attention, batch size ([REST load](https://lmstudio.ai/docs/developer/rest/load)).
- **Keep model in memory**, **Try mmap**, cache quantisation ([SDK](https://lmstudio.ai/docs/typescript/api-reference/llm-load-model-config)).
- **Hardware page**: per-GPU switches, even or priority split; priority and "limit to
  dedicated memory" are CUDA only, so not for the owner's AMD card ([0.3.14](https://lmstudio.ai/blog/lmstudio-v0.3.14)).
- *Unverified*: a "CPU Thread Pool Size" control and a resource monitor (third-party guides only).

## What llama-server (pinned b12110) can do
From [the b12110 server README](https://raw.githubusercontent.com/ggml-org/llama.cpp/b12110/tools/server/README.md):
- Layers on the graphics card `-ngl N|auto|all`; experts on the processor `--cpu-moe` (all)
  or `--n-cpu-moe N` (first N layers); `--n-cpu-ffn N` for dense layers; an expert cache
  `--moe-cache-mib`; tensor overrides `-ot` (never shown: free text).
- Conversation memory (KV cache) on the graphics card `-kvo` (default) / `-nkvo`.
- Threads `-t`, `-tb` (automatic by default).
- Several cards `-sm`, `-ts`, `-mg`, `-dev`; flash attention `-fa on|off|auto`; loading
  `-lm auto|none|mmap|mlock|mmap+mlock|dio`.
- **Fit** `-fit on|off` (adjusts only arguments left unset: context, layers, experts to the
  processor), `-fitt` (memory to leave free per device, default 1024 MiB), `-fitc` (smallest
  context). `llama-fit-params` ([README](https://raw.githubusercontent.com/ggml-org/llama.cpp/b12110/tools/fit-params/README.md))
  prints the fitted arguments and per-device use **without loading** ("48 layers (34
  overflowing), 19187 MiB used, 1199 MiB free"). It is already shipped with the app and not
  run yet: the best basis for an exact before-load picture.
- **What it reports afterwards:** the log line "offloaded X/Y layers to GPU" (already parsed,
  `crates/runtime/src/llama/protocol.rs`), the fit projection line, per-device buffer sizes
  (model, KV, compute; *unverified* whether the model line needs `-lv 4` on b12110), a memory
  table at exit. `/props`, `/metrics`, `/slots` give no placement figures.
- On Windows the app ships the **Vulkan** build (the owner's RX 9070 XT); a ROCm pack is an
  optional download (`llama.lock.json`).

## Reading usage on this computer only (no telemetry)
| System | Graphics card busy | Graphics memory used | Notes |
|---|---|---|---|
| Windows (any card) | PDH `\GPU Engine(*)\Utilization Percentage`, busiest engine per card (Task Manager's rule) | PDH `\GPU Adapter Memory(*)\Dedicated Usage` (whole system); `\GPU Process Memory(pid_N*)` for llama-server | No admin rights expected (*unverified*; counter names from community sources, [Task Manager blog](https://devblogs.microsoft.com/directx/gpus-in-the-task-manager/)). Through the `windows` crate the app already uses. DXGI's figure covers only our own process; AMD's ADLX has a proprietary licence: not used. |
| macOS (Apple silicon) | IORegistry `IOAccelerator` → `PerformanceStatistics` → "Device Utilization %" | "In use system memory" (shared memory) | No admin; undocumented keys that can change (*unverified* on the newest macOS) |
| Linux, AMD | sysfs `gpu_busy_percent` | `mem_info_vram_used` / `_total` | [kernel docs](https://docs.kernel.org/gpu/amdgpu/driver-misc.html) |
| Linux/Windows, NVIDIA | NVML | NVML | library loaded at run time if present |
| Linux, Intel; anything else | — | — | "Not available" |
Processor and memory: `GetSystemTimes`/`GetProcessTimes` (Windows), `host_processor_info`
(macOS), `/proc/stat` (Linux); memory is already read in `crates/models/src/hardware.rs`. No
admin anywhere. The `sysinfo` crate is not needed (its Windows GPU memory is the
per-process figure).

## What the app has today
`GpuUse` All / As much as fits / Don't use it (`crates/runtime/src/llama/launch.rs`), the
device list with free memory at probe time (`devices.rs`), the parsed "offloaded" line and a
"part on the processor" note (`RuntimeIndicator.tsx`), a before-load estimate
(`crates/models/src/fit.rs`, `src-tauri/src/load_settings.rs`), and the settings dialog
(`src/components/ModelSettings.tsx`). Nothing sets threads, experts, KV placement or the fit
margin; nothing reads live usage.

## Proposed shape (keeps D4's plain defaults)
1. **Before loading, in Settings for this model:** a bar each for "Graphics card memory"
   and "Computer memory", split into the model, conversation memory and working space, with
   what is free now; "28 of 36 layers on the graphics card". From `llama-fit-params` when it
   answers quickly, else the app's own estimate (labelled "estimate"). A warning before a
   likely out-of-memory load with **Load anyway**.
2. **Fine control** (a closed section under More options, a typed value for each, no free
   text): **Layers on the graphics card** (Automatic, or 0…N); **Expert parts on the
   processor** (MoE models only: None / Some N / All); **Conversation memory on the graphics
   card** (on/off); **Processor threads** (Automatic or 1…cores); **Fast attention**
   (Automatic/On/Off, split out of Save memory); **Leave free on the graphics card** (0.5 /
   1 / 2 GB). Each with a one-line help in plain words; "Reset fine control".
3. **Live usage:** a strip under the runtime indicator (or in its menu): "Graphics card 63% ·
   11.2 of 16 GB (model 9.8 GB) · Processor 12% · Memory 14 of 32 GB"; polled every 1-2 s only
   while visible; "Not available" per figure where the system gives none. After a load:
   where the model actually went (from the log).

## D4 amendment (proposed)
D4's plain controls stay the default and unchanged. "Never shown" becomes: batch sizes,
slots, rope, tensor overrides, free-text arguments, paths. Layers, experts, KV placement,
threads, flash attention and the fit margin are shown **only** under Fine control. Split
across several cards (`-ts`) stays out until someone has two cards to test.

## Questions for the owner
- Fine control inside each model's settings (closed by default), or a separate "Advanced"
  switch in Settings that reveals it everywhere?
- Live usage always visible under the model name, or only when the menu is open?
- Which of the six fine controls do you want first (all six is about a day more than three)?
