# Local image and video generation for harnessUI Studio: engines and models (research as of 2026-11-17, read-only)

Labels used below:
- **[D]** documented by the vendor
- **[M]** measured by a named third party
- **[E]** my own estimate, extrapolated from the [M] anchors
- **[U]** unconfirmed

I edited no repo files. All scratch work is in /tmp/claude-0/-home-user/f8f6f1ef-33e4-57b5-aed7-1e790c206f77/scratchpad.

## 0. Bottom line

- **Engine:** use **stable-diffusion.cpp (`sd-server`) on both Windows and macOS**, run as a second pinned sidecar exactly like llama-server.
  - It is MIT-licensed, needs no Python, and uses the same ggml/GGUF stack the team already ships.
  - It has the widest model list (image models plus Wan 2.1/2.2, LTX-2.3/2.5, HunyuanVideo 1.5).
  - It has a native async HTTP API with job progress, previews, cancel and model load/unload.
  - It has offload, tiling and segmented execution for low-VRAM machines.
- **Mac fallback:** keep an MLX engine in mind for 0.2 if sd.cpp's Metal speed on the M5 Air is poor. Do a one-day benchmark on the owner's M5 Air before promising any times in the UI.
- **Default video model:** Wan 2.2 TI2V-5B with a 4-step "Turbo/FastWan" distillation, at 480p by default.
- **Default image models:** FLUX.2 [klein] 4B and Z-Image Turbo (both Apache-2.0).
- **On CPU-only machines, hide video.** Images work there but are slow.

## 1. Engines

### 1.1 stable-diffusion.cpp (leejet/stable-diffusion.cpp)

**Releases** [D]
- There are no semver releases. Rolling builds are tagged `master-<n>-<sha>`.
- The latest is **master-1106-532d2c1**, released 16 Nov 2026. Its note is "refactor(api): adopt opaque options, tensors, and results (#2365)". Builds come out every 1–2 days.
- The README warns: "API and command-line option may change frequently."
- Sources: https://github.com/leejet/stable-diffusion.cpp/releases/tag/master-1106-532d2c1 , https://sourceforge.net/projects/stable-diffusion-cpp.mirror/files/ , https://raw.githubusercontent.com/leejet/stable-diffusion.cpp/master/README.md

**Prebuilt binaries for master-1106** [D] (sizes are GitHub MiB, with the SourceForge MB figure in brackets)

| Archive | Size |
|---|---|
| `sd-master-532d2c1-bin-win-vulkan-x64.zip` | 28.9 MiB (30.3 MB) |
| `...-bin-win-cpu-x64.zip` | 17.5 MiB |
| `...-bin-win-cuda12-x64.zip` | 393 MiB |
| `cudart-sd-bin-win-cu12-x64.zip` (CUDA runtime) | 537 MiB |
| `...-bin-win-rocm-10.1.0-x64.zip` | 270 MiB |
| `...-bin-Darwin-macOS-26.7.2-arm64.zip` | 36.6 MiB |
| `...-Linux-Ubuntu-26.04-x86_64(-vulkan).zip` | 27.8 / 40.3 MiB |
| `...-Linux-Ubuntu-24.04-x86_64-rocm-10.1.0.zip` | 307 MiB |

Sources: https://github.com/leejet/stable-diffusion.cpp/releases/expanded_assets/master-1106-532d2c1 and https://sourceforge.net/projects/stable-diffusion-cpp.mirror/files/master-1106-532d2c1/

- **The macOS zip is a universal binary.** CI builds it with `-DCMAKE_OSX_ARCHITECTURES="arm64;x86_64"`; the file name says "arm64" only because of the build machine's architecture [D]. Source: https://github.com/leejet/stable-diffusion.cpp/blob/master/.github/workflows/build.yml (around line 346).
- **Metal in the macOS prebuilt:**
  - In Dec 2025 the maintainer said the precompiled build had no Metal ([issue #1030](https://github.com/leejet/stable-diffusion.cpp/issues/1030)).
  - Today CI still doesn't pass `SD_METAL`, but the ggml fork turns Metal on by default on Apple ([CMakeLists](https://raw.githubusercontent.com/leejet/ggml/master/CMakeLists.txt) line 96).
  - I inflated part of `libstable-diffusion.dylib` (95 MB) from the master-1106 mac zip via the SourceForge mirror and grepped the bytes; I did not run it. It contains the ggml Metal backend, embedded kernels, M5 device IDs, and the M5 "tensor API" code with the `GGML_METAL_TENSOR_DISABLE/ENABLE` switches. So **Metal is compiled in**.
  - [U] It has not been run on a Mac yet. sd.cpp's own extra `SD_METAL` int8 kernels are not in the prebuilt ([CMakeLists.txt](https://github.com/leejet/stable-diffusion.cpp/blob/master/CMakeLists.txt) lines 105 and 397–400).
- **The Windows builds ship `ggml.dll`, `ggml-base.dll`, `ggml-cpu-*.dll` and `ggml-vulkan.dll`.** CI builds them with `SD_BUILD_SHARED_GGML_LIB=ON` and `GGML_BACKEND_DL=ON` (build.yml lines 390–394).
  - These are the same file names as llama.cpp's (see `llama.lock.json` `keep_files`), but they are built from leejet's ggml fork.
  - **sd.cpp must therefore live in its own folder.**
- **The CUDA runtime pack** contains `cudart64_*`, `cublas64_*` and `cublasLt64_*` from CUDA 12.8.1 (build.yml lines 417 and 482–489).

**Models** [D] (README, https://github.com/leejet/stable-diffusion.cpp/blob/master/docs/models/README.md)
- **Image:**
  - SD1.x, SD2.x, SDXL (and Turbo), SD3/3.5
  - FLUX.1 dev/schnell, FLUX.2 dev/klein, FLUX.3 Image Klein (added 2026-10-16)
  - Chroma / Chroma1-Radiance
  - Qwen Image and Qwen Image 2.1 (added 2026-09-20)
  - Z-Image and Z-Image L2P
  - Krea2, Ideogram4, HiDream-O1-Image, MiniMax-H3-Image (added 2026-11-11), MOSS-Image, LongCat, Ovis, PixArt and about 15 more
- **Image editing:** Kontext, the Qwen Image Edit series, FLUX.2/3 reference images, and others.
- **Video:** Wan2.1/Wan2.2 (T2V, I2V, FLF2V, VACE, S2V, TI2V-5B), MiniMax-H3, LTX-2.3/LTX-2.5, HunyuanVideo 1.5, LingBot-Video.
- **Video upscaling:** RelayVSR, SeedVR2.
- **Not supported:** CogVideoX, Mochi, and the small LTX-Video 0.9.x.
- **Wan 2.5/2.6/2.7 are not open weights:** Wan-AI's Hugging Face account has no Wan 2.5, 2.6 or 2.7 repos.

**Backends** [D]: CPU (AVX/AVX2/AVX512), CUDA, Vulkan, Metal, OpenCL, SYCL; ROCm/HIP is in the build guide. Per-module placement is supported, e.g. `--backend te=cpu,vae=cuda0,diffusion=vulkan0`. Source: [backend guide](https://github.com/leejet/stable-diffusion.cpp/blob/master/docs/guides/backend.md).

**Weight formats** [D]
- .ckpt/.pth/.pt, safetensors, GGUF.
- On-the-fly `--type` f16/q8_0/q5_x/q4_x, and offline conversion with `sd-cli -M convert`.
- The ecosystem also uses K-quants and INT8 "convrot".
- Source: [quantization guide](https://github.com/leejet/stable-diffusion.cpp/blob/master/docs/guides/quantization_and_gguf.md).

**Server mode** [D] (https://github.com/leejet/stable-diffusion.cpp/blob/master/examples/server/README.md , https://github.com/leejet/stable-diffusion.cpp/blob/master/examples/server/api.md)
- `sd-server` listens on 127.0.0.1:1234 by default and has a built-in web UI.
- It exposes three API families:
  - **Native async** `/sdcpp/v1/`: `img_gen`, `vid_gen`, `jobs/{id}`, `jobs/{id}/cancel`, `upscale`, `capabilities`.
  - **Model management:** `recipes`, `model-files`, `models/load`, `models/unload`, `models/status`.
  - **Compatibility:** OpenAI `/v1/images/*` and A1111 `/sdapi/v1/*`.
- **Jobs:** statuses are queued/generating/completed/failed/cancelled. Defaults are max 64 pending jobs and a 600 s TTL for finished jobs.
- **Progress:** each job carries `progress.stage` (conditioning, sampling, vae_decode, encoding…), `pass`, `completed/total` steps and `elapsed_ms`, plus an optional `preview.b64_json` PNG.
- **One model at a time:** a load or unload returns 409 while jobs are pending.
- **Video output:** the whole container comes back base64-encoded as **webm, webp or avi — no MP4**.
- **Frame alignment:** Wan/Hunyuan use 4n+1 frames; LTX uses 8n+1.
- **The CLI** prints progress with `-v`. The C API has preview callbacks and cancellation ([c_api.md](https://github.com/leejet/stable-diffusion.cpp/blob/master/docs/guides/c_api.md)).

**Image-to-video and first/last frame** [D] (see [wan.md](https://github.com/leejet/stable-diffusion.cpp/blob/master/docs/models/wan.md), [ltx2.md](https://github.com/leejet/stable-diffusion.cpp/blob/master/docs/models/ltx2.md), [hunyuan_video.md](https://github.com/leejet/stable-diffusion.cpp/blob/master/docs/models/hunyuan_video.md))
- I2V via `-i`: Wan2.1 I2V 14B, Wan2.2 I2V A14B, TI2V-5B, LTX-2.3, HunyuanVideo 1.5 I2V.
- First/last frame via `--init-img` + `--end-img`: Wan2.1 FLF2V 14B, Wan2.2 (using the I2V A14B weights), LTX-2.3.
- [U] First/last frame for TI2V-5B is not documented.
- The server takes `end_image` in `vid_gen` requests.

**Low-VRAM options** [D] ([performance guide](https://github.com/leejet/stable-diffusion.cpp/blob/master/docs/guides/performance.md))
- Tiling: `--vae-tiling` with tile size and overlap, `--temporal-tiling`, and automatic retry with smaller VAE tiles on out-of-memory.
- Offload: `--offload-to-cpu`, `--params-backend cpu|disk` per module, and automatic segmented execution with prefetch for models bigger than VRAM.
- Memory caps and attention: `--max-vram`, `--flash-attn`.
- Speed-ups: tiny decoders, e.g. TAEHV `taew2_2` for Wan 2.2 5B ([taesd.md](https://github.com/leejet/stable-diffusion.cpp/blob/master/docs/models/taesd.md)), and step caches (easycache, dbcache, taylorseer…) ([caching.md](https://github.com/leejet/stable-diffusion.cpp/blob/master/docs/guides/caching.md)).
- The Z-Image guide says it runs "on GPUs with 4GB of VRAM — or even less" ([z_image.md](https://github.com/leejet/stable-diffusion.cpp/blob/master/docs/models/z_image.md)).

**Licence** [D]: MIT ([LICENSE](https://raw.githubusercontent.com/leejet/stable-diffusion.cpp/master/LICENSE)); ggml is MIT; libwebp/libwebm are BSD.

**Who else uses it** [D]: AMD's Lemonade ships sd-cpp with cpu/cuda/metal/rocm/vulkan backends ([backend reference](https://lemonade-server.ai/docs/dev/backends-reference/)) and is adding video through sd-server ([discussion #3527](https://github.com/lemonade-sdk/lemonade/discussions/3527)).

**Known risks**
- **Speed vs ComfyUI** [M], [issue #1215](https://github.com/leejet/stable-diffusion.cpp/issues/1215), Jan 2026:
  - RTX 4090, CUDA: about 6.7 vs 10.5 it/s.
  - RX 7800 XT, Vulkan: 1.65–3.7× slower.
  - PR #1228 then gave 30–59% speed-ups on AMD.
- **Metal on M5:** with the tensor path on, Z-Image Turbo produces blank white PNGs. The workaround is `GGML_METAL_TENSOR_DISABLE=1`. The issue is still open ([#1990](https://github.com/leejet/stable-diffusion.cpp/issues/1990), Sep 2026).
- **Stale Metal warning:** the build guide still says Metal is "highly inefficient" for very large matrices ([build.md](https://github.com/leejet/stable-diffusion.cpp/blob/master/docs/build.md)).
- **Quality with tiny quants:** heavily quantized small Wan models give "blurry shapes" on Vulkan/ROCm ([discussion #1000](https://github.com/leejet/stable-diffusion.cpp/discussions/1000)). Keep 1.3B at fp16 and 5B at Q5 or above.

### 1.2 Alternatives

| Option | What it is | Fit for a Tauri app without Python |
|---|---|---|
| ComfyUI | Python + PyTorch. Official minimum is 4.85 GB disk per install ([docs](https://docs.comfy.org/installation/desktop/overview)). GPL-3.0. On Mac, FP8 fails under MPS and Wan 14B took 82 min per 2 s clip on an M1 Max ([lilting](https://lilting.ch/en/articles/ltx2-wan22-mac-local-video-gen)). | No. Too heavy. At most, "connect to an existing ComfyUI" later. |
| mflux 0.24.0 (2026-10-27) | MIT, Python/MLX, images only ([PyPI](https://pypi.org/pypi/mflux/json)) | No (Python) |
| FastVideo FastMetal | Apache-2.0, Python on MLX. 3-step INT8 Wan models. Best Mac video numbers found (see §3) ([blog](https://haoailab.com/blogs/fastmetal/)). | Python. Its distilled weights are interesting. |
| turbo-mlx | MIT. Native macOS app with a Swift `turbo-engine` binary that speaks JSON lines. Klein 4B, Z-Image, FLUX.3 Klein, LTX-2.3/2.5. About 201 stars ([repo](https://github.com/rinste/turbo-mlx)). | The best "MLX without Python" template for a possible 0.2 Mac fast path. LTX needs 32 GB. |
| mlx-video-rs | MIT, Rust/MLX. Wan 2.1/2.2 and LTX-2. 0 stars, 7 commits ([repo](https://github.com/bhubbard/mlx-video-rs)). | Too immature. |
| mlx-video-with-audio 0.1.37 | MIT, Python, LTX-2 and Wan ([PyPI](https://pypi.org/project/mlx-video-with-audio/)) | No |
| Draw Things | Core stack and gRPCServerCLI are GPLv3, macOS and NVIDIA Linux only ([README](https://raw.githubusercontent.com/drawthingsai/draw-things-community/main/README.md)) | No Windows; GPL. External-server option only. |
| Ollama image generation | Experimental, macOS only, MLX, Z-Image and Klein only, no video ([blog](https://registry.ollama.ai/blog/image-generation)) | No |
| Amuse (AMD) | Closed Windows app, ONNX/DirectML ([AMD](https://www.amd.com/en/ecosystem/isv/consumer-partners/amuse.html)) | Not embeddable |
| LTX native runtimes | Official runtime is Python/PyTorch. MLX ports exist ([dgrauet/ltx-2-mlx](https://github.com/dgrauet/ltx-2-mlx), [ltx-video-swift-mlx](https://github.com/VincentGourbin/ltx-video-swift-mlx)). | Not needed; sd.cpp covers LTX. |

### 1.3 Recommendation per OS
- **Windows:**
  - Base engine: sd.cpp Vulkan build, 28.9 MiB zip.
  - Optional packs: CUDA 12 (393 MiB + 537 MiB runtime; [U] whether llama's existing CUDA pack can share cuBLAS) and ROCm (270 MiB).
  - Download it on first use of local Studio with a SHA-256 pin, like the existing backend packs. That avoids pressure on the 150 MB installer cap.
  - Put it in its own folder (e.g. `sdcpp/`) because the ggml DLL names collide with llama.cpp's.
- **macOS:**
  - Use the universal prebuilt (Metal compiled in).
  - Set `GGML_METAL_TENSOR_DISABLE=1` for Z-Image on M5 until #1990 is fixed.
  - [E] If sd.cpp Metal on the M5 Air is more than about 2× slower than the MLX references in §3, plan a turbo-mlx-style Swift/MLX sidecar for 0.2.
- **Both:**
  - Run `sd-server` and drive `/sdcpp/v1/*`.
  - Keep the model loaded between jobs.
  - Coordinate memory with llama-server: on 12 GB GPUs and 16 GB Macs, unload the chat model before a video job. This is a design inference.

## 2. Models

Sizes come from the Hugging Face API, e.g. https://huggingface.co/api/models/leejet/Z-Image-Turbo-GGUF/tree/main. Licences and gating come from the HF model API. "gated" means a Hugging Face login or token is needed. File lists come from sd.cpp's model guides.

### 2.1 Image models

| Model | Licence | Diffusion GGUF (Q4 / Q8) | Required extras | Total download (Q4 / Q8) |
|---|---|---|---|---|
| **FLUX.2 [klein] 4B** (4 steps, also edits) [flux2.md](https://github.com/leejet/stable-diffusion.cpp/blob/master/docs/models/flux2.md) | Apache-2.0, ungated | leejet/FLUX.2-klein-4B-GGUF: Q4_0 2.46 / Q8_0 4.30 GB | Qwen3-4B (unsloth/Qwen3-4B-GGUF Q4_K_M 2.50 / Q8 4.28); flux2-vae 0.34 (Comfy-Org/flux2-klein-4B, ungated) | 5.3 / 8.9 GB |
| **Z-Image Turbo** (8 steps) | Apache-2.0, ungated | leejet/Z-Image-Turbo-GGUF: Q4_K 3.86 / Q8_0 6.58 (Q2_K 2.59) | Qwen3-4B-Instruct-2507 GGUF 2.50 / 4.28; ae.safetensors 0.34 (Comfy-Org/z_image_turbo, ungated) | 6.7 / 11.2 GB |
| **FLUX.3 Image Klein** (5.6B, released 2026-10-15; [VentureBeat](https://venturebeat.com/ai/black-forest-labs-opens-flux-3-image-klein-weights-with-free-commercial-use-limited-to-smaller-businesses)) [flux3.md](https://github.com/leejet/stable-diffusion.cpp/blob/master/docs/models/flux3.md) | FLUX Kommunity licence (free commercial use under $5M revenue); upstream gated, leejet mirror ungated | leejet/flux-3-image-klein-GGUF: Q4_K 3.51 / Q8_0 6.15 | Qwen3-VL-4B GGUF 2.50 / 4.28; ae 0.34 | 6.4 / 10.8 GB |
| FLUX.2 [klein] 9B | non-commercial, gated | Q4_0 5.62 / Q8 9.98 | Qwen3-8B 5.03 / 8.71; VAE 0.34 | 11.0 / 19.0 GB |
| Qwen-Image 2.1 (7B per BFL's comparison) | **Qwen Research licence, non-commercial** ([LICENSE](https://huggingface.co/Qwen/Qwen-Image-2.1/raw/main/LICENSE)) | leejet/Qwen-Image-2.1-GGUF: Q4_K 4.20 / Q8 7.69 | Qwen3-VL-8B 5.03 / 8.71; VAE 0.68 | 9.9 / 17.1 GB |
| Krea2 Turbo (12.9B) | Krea-2 community licence, gated | realrebelai/KREA-2_GGUFs: Q4_K_M 7.22 / Q8 13.63 | Qwen3-VL-4B; Wan2.1 VAE 0.25 | ~10 / ~18 GB |
| Chroma1-HD | Apache-2.0 | silveroxides/Chroma-GGUF: Q4_0 5.43 / Q8 9.74 | T5-XXL GGUF (city96) Q4_K_M 2.90 / Q8 5.06; ae 0.34 | 8.7 / 15.1 GB |
| Juggernaut XL v9 (SDXL) | OpenRAIL-M | single safetensors file, 7.11 GB | none | 7.1 GB |

**Quality ranking (open weights)**: Artificial Analysis' open-weights board has Ideogram 4.0, HiDream-O1 and FLUX.2 dev at the top, with Qwen-Image-2.1 leading in another category ([AA](https://artificialanalysis.ai/image/leaderboard/text-to-image?open-weights=true)). [U] The snapshot date is unknown.

### 2.2 Video models

| Model | Licence | Diffusion weights | Extras | Total download | Notes |
|---|---|---|---|---|---|
| **Wan 2.2 TI2V-5B** (T2V and I2V, 720p native, 24 fps; [README](https://huggingface.co/Wan-AI/Wan2.2-TI2V-5B)) | Apache-2.0 | QuantStack/Wan2.2-TI2V-5B-GGUF (documented in sd.cpp): Q4_K_M 3.43 / Q8 5.40. 4-step distillations: hum-ma/Wan2.2-TI2V-5B-Turbo-GGUF Q4_K_M 3.44 / Q8 5.40 plus a Turbo LoRA (0.33 GB); Green-Sky/FastWan2.2-TI2V-5B-FullAttn-GGUF q6_k 4.21 / q8 5.41 | umt5-xxl GGUF (city96) Q4_K_M 3.66 / Q8 6.04; **Wan2.2 VAE 1.41** (not the 2.1 VAE); optional taew2_2 tiny decoder | 8.5 / 12.9 GB | [U] Whether the Turbo/FastWan GGUFs load in sd.cpp: hum-ma's were "fixed 5d tensors" for ComfyUI-GGUF. The safe path is base GGUF + Turbo LoRA. |
| Wan 2.1 T2V 1.3B (480p, 16 fps) | Apache-2.0 | fp16 safetensors 2.84 (don't quantize) | umt5 + Wan2.1 VAE 0.25 | 6.8–9.1 GB | text-to-video only |
| **LTX-2.3** 22B (T2V, I2V, FLF2V, **with audio**) | LTX-2 community licence: companies with $10M+ revenue must buy a licence ([LICENSE-2](https://github.com/Lightricks/LTX-2/blob/main/LICENSE-2)) | unsloth/LTX-2.3-GGUF distilled-1.1: Q4_K_M 14.19 / Q8 22.76 (Q3_K_M 10.63) | Gemma-3-12B GGUF Q4_K_M 7.30; connectors 2.31; video VAE 1.45; audio VAE 0.36 | 25.6 GB (Q4) | LTX-2.5 needs a 15.4 GB+ Gemma-4 encoder and is gated |
| Wan 2.2 I2V / T2V A14B (MoE, best Wan quality, FLF2V) | Apache-2.0 | high-noise + low-noise files, each Q4_K_M 9.65 / Q8 15.41 (Q3_K_M 7.18) | umt5; Wan2.1 VAE; lightx2v 4-step LoRAs | ~25.6 GB (Q4) | sd.cpp examples use `--offload-to-cpu` |
| HunyuanVideo 1.5 (480p I2V) | **Tencent licence excludes the EU, UK and South Korea** ([LICENSE](https://huggingface.co/tencent/HunyuanVideo-1.5/raw/main/LICENSE)) | jayn7 GGUF: Q4_K_M 5.09 / Q8 9.00; Comfy fp8 8.33 | Qwen2.5-VL-7B GGUF 4.68 / 8.10; VAE 2.52; byT5 0.44 | 12.7 / 20 GB | [U] Whether jayn7's GGUF loads in sd.cpp |
| MiniMax-H3 | **licence excludes the EU, UK, South Korea and the USA** ([LICENSE](https://huggingface.co/MiniMaxAI/MiniMax-H3/raw/main/LICENSE)) | leejet GGUF Q4_K_M 18.78 (pruned 11.42) | Qwen3-VL-32B Q4 18.22 (or a 4B connector); VAEs 3–5 GB | 30 GB+ | not a starter model |

**Memory needs** (with sd.cpp offload; [E] unless a source is given)

| Model | Minimum | Comfortable |
|---|---|---|
| Klein 4B / Z-Image Q4 | 4–6 GB VRAM + 16 GB RAM (z_image.md says 4 GB) | 8 GB VRAM |
| Wan 2.2 5B | 8 GB VRAM + 16 GB RAM (480p) | 12–16 GB VRAM + 32 GB RAM (720p) |
| LTX-2.3 Q4 | 12–16 GB VRAM + 32 GB RAM | 24 GB VRAM |
| Wan 2.2 A14B | 12 GB VRAM + 32 GB RAM | 24 GB VRAM + 64 GB RAM |

On a Mac, FastMetal's Wan 5B peaked at 9.5 GiB on a 24 GB M5 Air [M]. turbo-mlx lists LTX-2.3 4-bit as needing 32 GB [D]. LTX-2.3 and the 14B models are therefore not realistic on a 16–24 GB MacBook Air.

## 3. Speed

### 3.1 Measured anchors
- **RX 7800 XT, Vulkan, sd.cpp, 1024², Jan 2026** ([#1215](https://github.com/leejet/stable-diffusion.cpp/issues/1215)):
  - Klein 4B, 4 steps: 15.9 s (after the fix).
  - Klein 9B: 23.7 s.
  - Z-Image Turbo, 9 steps: 34.6 s.
  - SDXL, 30 steps: 29.6 s.
- **RX 7600 XT, Vulkan** (same issue):
  - Z-Image: 8.09 s/it.
  - Klein 4B: 7.25 s/it (before the fix).
- **RTX 4090, CUDA:** Klein about 6.7 it/s (same issue).
- **M5 Max, Metal, sd.cpp:** Z-Image Turbo at 864×480, 8 steps, takes about 28 s with the tensor path off (about 20 s with it on, but the output is blank). MiniMax-H3 video, 22 frames at 864×480, 20 steps, takes about 2 min ([#1990](https://github.com/leejet/stable-diffusion.cpp/issues/1990)).
- **Radeon AI PRO R9700, Vulkan, sd-server via Lemonade:** a 33-frame 832×480 clip in 140 s with about 14 GB peak VRAM; a 9-frame 480×320 clip in about 9 s ([#3527](https://github.com/lemonade-sdk/lemonade/discussions/3527)). The model isn't named; [U] the 8.8 GB of weights matches Wan2.1 1.3B fp16 plus umt5 Q8.
- **RX 7900 XTX, ROCm:** Wan2.2 S2V 14B at 640×368, 13 frames: 6.19 s/it (Q8) and 10.99 s/it (Q4_K_M) ([PR #1925](https://github.com/leejet/stable-diffusion.cpp/pull/1925)).
- **CPU only, Ryzen 5 PRO 4650U laptop, sd.cpp** ([ai-suka](https://ai-suka.pages.dev/2026/04/12-flux2-klein-stable-diffusion-cpp-vulkan-cpu)):
  - Z-Image Q4_K, 512², 3 steps: about 370 s.
  - Same, 1024², 3 steps: about 15–20 min.
  - Klein 4B, 512², 4 steps: about 339 s.
- **MLX references on Mac:**
  - turbo-mlx on an M1 Max 64 GB: Klein 4B at 1024² about 27 s; Z-Image 4-bit about 1.5 min; LTX-2.3 4-bit, 5 s clip at 768×512 with sound, about 3.5 min.
  - FastMetal 3-step INT8 on a **MacBook Air M5 24 GB**: Wan2.2-5B at 1280×704, 81 frames, 200 s (fast mode 91 s); 1.3B at 480p, 81 frames, 156 s ([blog](https://haoailab.com/blogs/fastmetal/)).
  - Apple says MLX runs FLUX-dev 4-bit 3.8× faster on M5 than M4 ([Apple ML research](https://machinelearning.apple.com/research/exploring-llms-mlx-m5)).
- **Official Wan figure:** TI2V-5B makes a 5 s 720p clip in under 9 min on a single consumer GPU (4090-class) without optimisation ([README](https://huggingface.co/Wan-AI/Wan2.2-TI2V-5B)).

### 3.2 Expectations per tier (all [E])

Columns are images at 1024², then video with Wan 2.2 5B Turbo (4 steps, CFG 1).

| Tier | Klein 4B image | Z-Image image | 2 s 480p clip | 5 s 720p clip (121 frames) |
|---|---|---|---|---|
| RTX 4070 12 GB, CUDA | 3–6 s | 8–15 s | 20–40 s | 2–4 min |
| RTX 3060 12 GB | 5–10 s | 12–25 s | 40–80 s | 4–8 min |
| 8 GB GPU | 10–25 s | 30–70 s | 1–3 min | 8–15 min (with tiling) |
| MacBook Air M5 16–24 GB | 1–2 min | 2–5 min | 2–5 min | 10–30 min (fanless, may throttle) |
| CPU only | 5–6 min at 512² | — | not feasible | not feasible |

- **NVIDIA on Vulkan** (the Windows default build) is probably slower than CUDA, so expect the upper end of each range.
- **The base 5B model without Turbo** (20–50 steps with CFG) is about 10× slower than the table.
- **LTX-2.3 distilled, 5 s at 768×512:** about 3–8 min on a 4070 or 6–15 min on a 3060, and it needs 32 GB system RAM. Not on 8 GB GPUs or the Air.
- **CPU only:** a desktop CPU might be 2–4× faster than the laptop anchor.

## 4. Proposed starter catalog

**Images**
1. **FLUX.2 [klein] 4B** (Apache, 5.3 GB at Q4): the default for every tier including the Air and CPU. It can also edit images.
2. **Z-Image Turbo** (Apache, 6.7 GB at Q4 / 11.2 GB at Q8): photoreal. Recommended for 8 GB+ GPUs; slower on the Air.
3. **FLUX.3 Image Klein** (Kommunity licence, 6.4 GB): newest; strongest text rendering and editing. Show a licence badge.
4. **Qwen-Image 2.1** (non-commercial, 9.9 GB at Q4): "best quality" option for 12 GB+ GPUs. If you'd rather avoid licence friction, use Chroma1-HD (Apache, but less filtered) instead.

**Video**
1. **Wan 2.2 TI2V-5B with Turbo/FastWan, 4 steps** (Apache, 8.5–12.9 GB): the default for 8 GB+ GPUs and Macs with 16 GB+ (24 GB recommended). Default to 480p and 2–3 s; offer 720p/5 s with the time warnings from §3.
2. **LTX-2.3 distilled Q4** (25.6 GB, with sound): for 16 GB+ VRAM and 32 GB+ RAM. Hide it on Macs under 32 GB.
3. *(Advanced)* **Wan 2.2 I2V A14B Q4 + 4-step LoRAs** (about 26 GB): best image-to-video and first/last-frame quality for 24 GB GPUs.

**Wording for the UI:**
- MacBook Air M5: "a 2-second 480p clip takes about 2–5 minutes; 5 seconds at 720p can take 10–30 minutes and the Mac will get warm."
- 3060-class PC: "about 1 minute for a short clip; 4–8 minutes for 5 s at 720p."
- CPU-only machines: video is unavailable; images take several minutes.

## 5. Things to settle in a one-day benchmark
- Does the Metal path in the macOS prebuilt actually run on the owner's M5 Air, and how fast are Klein 4B, Z-Image and Wan 5B Turbo there (tensor path on vs off)?
- Do the hum-ma Turbo and Green-Sky FastWan GGUFs load in sd.cpp, or is base GGUF + Turbo LoRA needed? Can the FastMetal 3-step weights be converted?
- Vulkan vs CUDA speed on the owner's Windows GPU (model unknown).
- WebM (VP8) playback in WKWebView and WebView2, and whether to add MP4 export.
- How sd-server and llama-server share VRAM/RAM.
- The full terms of the FLUX.3 Kommunity licence (upstream is gated, so I couldn't read it).
- How much installer headroom is left, in case you'd rather bundle the Vulkan sd.cpp (28.9 MiB zip) than download it.
- Gated upstream repos (FLUX.1/2 VAEs, FLUX.3, LTX-2.5, Krea-2) need an HF token. Prefer the ungated Comfy-Org and leejet mirrors where the licence allows.
