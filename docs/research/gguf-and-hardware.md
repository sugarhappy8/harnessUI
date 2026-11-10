# gguf-and-hardware — research

## Summary

Researched against llama.cpp master (commit 3b4d6ab3, 2026-11-10; latest release tag b12242), crates.io, the HF API and Microsoft/Apple/NVIDIA docs. Not verified live: running the downloaded llama-server binary was blocked in this session, so the `--list-devices` output format and `/props` shape come from source reading, not a run.

**Decisions this supports**
- **GGUF reader:** hand-roll a ~200-line header-only reader. GGUF is v3, little-endian and simple. A Python prototype of the design read six real HF files correctly over HTTP Range in about 1.4 s; the KV section is about 6 MB for text models, under 2 KB for mmproj, and 106 bytes for non-first shards. Existing crates either decode whole tokenizer arrays or need mmap.
- **Memory estimate:** our own formula should only be the pre-download estimate. llama.cpp now measures memory itself without allocating: `--fit` is on by default, and every release archive I opened (Linux, Windows Vulkan, macOS arm64) ships `llama-fit-params`. Prefer its numbers once a file is local.
- **KV formula:** confirmed in source and reproduces llama.cpp's own logged numbers exactly (384 MiB at 4096 ctx and a 3456 MiB delta for Qwen3-30B-A3B). It over-estimates sliding-window models (Gemma 3) and hybrid/recurrent architectures.
- **Tool capability:** llama-server reports it in `GET /props` under `chat_template_caps` (`supports_tools`, `supports_tool_calls`, and others). A static scan for `tools` and `tool_calls` in the template mirrors those two flags.
- **mmproj detection:** `general.architecture == "clip"` is the only marker present in all real files; `general.type` is absent, `"clip-vision"` or `"mmproj"` depending on file age.
- **Hardware detection:** sysinfo 0.40.1 has a new `gpu` feature that reads DXGI `DedicatedVideoMemory` (64-bit) on Windows, so little hand-written DXGI code is needed. The most reliable GPU probe is `llama-server --list-devices`; on macOS its Metal total is `recommendedMaxWorkingSetSize`.
- **RX 9070 XT:** current llama.cpp has explicit RDNA4 code paths in the Vulkan backend. The only problem report I found is one unresolved community thread (Qwen3.5 slow on Vulkan, Windows 10, March 2026).

**Sources I could not reach:** AMD driver release notes (503), Phoronix (403), the GitHub REST API (blocked in this session), one Reddit mirror.

The prototype reader is at <temp workspace> (scratch, not in the repo).

## Findings

### GGUF current version is 3; the header layout is small and fixed, and llama.cpp accepts v2 and v3 only.

Confidence: high

Little-endian. Layout: magic bytes "GGUF" (as LE u32 0x46554747); u32 version; i64 tensor_count; i64 kv_count. Each KV: key string (u64 length + bytes, no NUL); i32 value type; value. Value types: UINT8=0, INT8=1, UINT16=2, INT16=3, UINT32=4, INT32=5, FLOAT32=6, BOOL=7 (1 byte), STRING=8, ARRAY=9, UINT64=10, INT64=11, FLOAT64=12. Array: i32 element type + u64 count + elements; llama.cpp rejects nested arrays. Tensor info: name string, u32 n_dims, i64 dims[n], i32 ggml type, u64 offset relative to the data section. Data section is aligned to `general.alignment` (u32, default 32, must be a power of 2). llama.cpp rejects version 0, 1 and >3, and detects big-endian files by (version & 0xFFFF)==0. Limits in llama.cpp: GGUF_MAX_STRING_LENGTH and GGUF_MAX_ARRAY_ELEMENTS are both 1024^3; duplicate keys and empty keys are errors; tensor names must be shorter than GGML_MAX_NAME.

Source: https://github.com/ggml-org/llama.cpp/blob/master/ggml/include/gguf.h ; ggml/src/gguf.cpp (gguf_init_from_reader) ; gguf-py/gguf/constants.py ; https://github.com/ggml-org/ggml/blob/master/docs/gguf.md

### A header-only read costs about 6 MB of sequential I/O for text models; the tokenizer arrays must be skipped, and the chat template sits after them.

Confidence: high

Measured with a prototype reader over HTTP Range on real files. Qwen3-8B Q4_K_M (5.03 GB): KV section ends at byte 5,933,917; largest values are tokenizer.ggml.merges 2.73 MB, tokenizer.ggml.tokens 2.59 MB (151,936 strings), tokenizer.ggml.token_type 0.61 MB. gemma-3-27b f16 shard 1: KV ends at 6,513,127 (tokens 4.41 MB, scores and token_type 1.05 MB each). `tokenizer.chat_template` came after the token arrays in these files, so you cannot stop early. Skipping: fixed-size arrays are one seek of count x element size; string arrays have no byte length, so walk them (read u64 length, seek forward) for each element. Do not parse tensor infos or touch tensor data unless you want exact per-layer sizes. The same reader works pre-download over Range requests (3-4 requests of 2 MB, about 1.4 s).

Source: Own measurement against huggingface.co/unsloth/Qwen3-8B-GGUF and ggml-org/gemma-3-27b-it-GGUF using <temp workspace>

### No existing Rust GGUF crate fits header-only-with-skip well; hand-rolling is the better choice.

Confidence: high

crates.io on 2026-11-10: gguf 0.1.2 (19,177 downloads, last update 2023-09, stale). gguf-rs 0.1.8 (24,299 downloads, updated 2026-06; has `get_gguf_container_array_size(file, max_array_size)` to cap arrays; anyhow/serde_json based). gguf-rs-lib 0.3.4 (95,505 downloads, updated 2026-11-04, MSRV 1.87; decodes all metadata into memory under a 256 MiB budget and parses every tensor info; no skip option). ggus 0.5.1 (446,531 downloads but only 12,497 recent, last update 2025-06; zero-copy over &[u8], so it needs mmap, and pulls in regex, indexmap and ggml-quants). llama-cpp-2 0.1.161 (1.9M downloads) is full llama.cpp bindings that compile the C++ library, which is redundant with a sidecar. None of these supports reading over HTTP Range.

Source: crates.io API (https://crates.io/api/v1/crates/<name>) and crate sources from static.crates.io

### The metadata keys needed all exist under these exact names; three have typing traps.

Confidence: high

general.architecture, general.name, general.type, general.file_type (u32), general.size_label, general.license, general.basename, general.finetune, general.quantized_by, general.alignment. Per-architecture: {arch}.context_length, {arch}.block_count, {arch}.embedding_length, {arch}.attention.head_count, {arch}.attention.head_count_kv, {arch}.attention.key_length, {arch}.attention.value_length, {arch}.attention.sliding_window, {arch}.full_attention_interval, {arch}.expert_count. Templates: tokenizer.chat_template, tokenizer.chat_template.{name} (llama.cpp only looks up the name "tool_use"), tokenizer.chat_templates (array of names). Traps: (1) head_count and head_count_kv may be a scalar or a per-layer array (llama.cpp reads them with get_key_or_arr; hybrid models use 0 for recurrent layers). (2) head_count_kv is optional and defaults to head_count. (3) key_length/value_length are optional and default to embedding_length/head_count, but when present they can differ: Qwen3-30B-A3B has embedding_length 2048, head_count 32 (implied 64) and key_length 128. file_type enum values checked on real files: 1=F16, 7=Q8_0, 15=Q4_K_M, 17=Q5_K_M, 18=Q6_K, 30=IQ4_XS, 32=BF16; full table is LlamaFileType in constants.py (0 through 42, 1024=GUESSED).

Source: gguf-py/gguf/constants.py ; src/llama-model.cpp load_hparams (around lines 1557-1612) ; real headers read from HF

### Split GGUFs are named `<prefix>-%05d-of-%05d.gguf` (1-based), only the first shard carries the model metadata, and llama.cpp must be pointed at the first shard.

Confidence: high

llama_split_path uses the format "%s-%05d-of-%05d.gguf" with split_no + 1. Keys: split.no (u16, 0-based), split.count (u16), split.tensors.count (i32, total across shards). Verified on ggml-org/gemma-3-27b-it-GGUF: shard 1 has 43 KVs including split.no=0, split.count=2, split.tensors.count=808; shard 2 has only those three split.* keys. The loader throws "illegal split file idx: %d ... model must be loaded with the first split" if given another shard, and derives sibling paths from the first shard's name. Total weight size is the sum of all shard file sizes. HF repos often put shards in a per-quant subfolder (for example `Q2_K/gpt-oss-120b-Q2_K-00001-of-00002.gguf`). llama.cpp's own detection regex is `^(.+)-([0-9]{5})-of-([0-9]{5})$` applied after stripping `.gguf`.

Source: src/llama.cpp (llama_split_path) ; src/llama-model-loader.cpp lines 601-640 ; tools/gguf-split/gguf-split.cpp ; common/download.cpp get_gguf_split_info ; HF API listings

### mmproj files are reliably identified by `general.architecture == "clip"`; `general.type` varies by file age.

Confidence: high

Real headers: ggml-org/gemma-3-4b-it-GGUF mmproj-model-f16.gguf has no general.type at all; ggml-org/Qwen2.5-VL-7B mmproj has general.type="clip-vision"; unsloth/Qwen3-VL-8B mmproj-F16.gguf has general.type="mmproj", which is what the current converter writes (GGUFType.MMPROJ). All three have general.architecture="clip" plus clip.has_vision_encoder (bool) and clip.projector_type (for example "gemma3", "qwen2.5vl_merger", "qwen3vl_merger"). Other keys: clip.has_audio_encoder, clip.vision.projector_type / clip.audio.projector_type for mixed-modality files, clip.vision.image_size, patch_size, embedding_length, block_count, projection_dim. Pairing check: the converter sets clip.vision.projection_dim to the text model's embedding size, and it matched in the files read (Qwen3-VL-8B 4096, gemma-3-4b 2560, Qwen2.5-VL-7B 3584). After load, the server reports `modalities: {vision, video, audio}` in /props.

Source: Real headers via HF Range reads ; conversion/base.py (MmprojModel.set_type, set_gguf_parameters) ; tools/mtmd/mtmd-impl.h ; tools/server/server-context.cpp get_res_props

### mmproj file naming in popular repos is consistent enough to match on the substring "mmproj"; F16 is the one quantization almost every repo offers.

Confidence: high

Observed names: ggml-org and lmstudio-community `mmproj-model-f16.gguf`; ggml-org newer `mmproj-<Model>-Q8_0.gguf` and `-f16.gguf`; unsloth `mmproj-F16.gguf`, `mmproj-BF16.gguf`, `mmproj-F32.gguf`; bartowski `mmproj-<org>_<model>-f16.gguf` (also bf16, f32); mradermacher `<Model>.mmproj-Q8_0.gguf` and `.mmproj-f16.gguf`; older repos `llava-phi-3-mini-mmproj-f16.gguf`, `llava-v1.5-7b-mmproj-model-f16.gguf`. Sizes: gemma-3 f16 812 MiB; Qwen2.5-VL-7B Q8_0 814 MiB vs f16 1291 MiB; Qwen3-VL-8B F16 1105 MiB vs F32 2199 MiB. llama.cpp's own auto-pick (find_best_sibling) takes .gguf files whose path contains "mmproj", prefers the deepest shared directory with the model, then the file whose quant bit number is closest to the model's (Q4 is 4, Q8 is 8, F16 is 16). The GGUF naming spec also reserves `mmproj-` and `mtp-` as sidecar prefixes.

Source: HF API https://huggingface.co/api/models/<repo> and /tree/main ; common/download.cpp find_best_sibling ; ggml docs/gguf.md naming convention

### llama-server reports tool capability in `GET /props` as `chat_template_caps`, computed by actually rendering the template with a probe conversation.

Confidence: high

Keys (booleans): supports_string_content, supports_typed_content, supports_tools, supports_tool_calls, supports_parallel_tool_calls, supports_system_role, supports_preserve_reasoning, supports_reasoning_effort, supports_object_arguments; plus reasoning_effort_levels (string array). The caps come from the tool_use template when the model has one, otherwise the default template. /props also returns chat_template, chat_template_tool_use (only with jinja on and a tool_use template present), modalities, model_path, build_info, total_slots and default_generation_settings.n_ctx. How it is computed: the template is rendered with a dummy tools array and a history containing tool_calls; supports_tools becomes false if tools[0].function.name was never read, and supports_tool_calls false if messages[1].tool_calls was never read. Caveat: the flags default to true and are only cleared when the probe render succeeds, so a template that throws during the probe keeps supports_tools=true.

Source: tools/server/server-context.cpp get_res_props (around lines 5689-5740) ; common/chat.cpp common_chat_templates_get_caps ; common/jinja/caps.h and caps.cpp ; tools/server/README.md GET /props

### A static pre-load heuristic of two regex checks mirrors the server's two tool flags; llama-server will still attempt tool calls on any model through a generic fallback.

Confidence: medium

Check A: the template references the `tools` variable (regex \btools\b), mirroring supports_tools. Check B: it references `tool_calls`, mirroring supports_tool_calls. On llama.cpp's 75 bundled templates: 63 have both, 3 have tools only (LFM2-8B-A1B, LFM2.5-Instruct, granite-3.3-2B), 4 have tool_calls only (DeepSeek-R1 distills, DeepSeek-V3.1, firefunction-v2), 5 have neither (gemma-2, Phi-3.5-mini, SmolLM3, translategemma, rwkv-world). Run the checks on `tokenizer.chat_template.tool_use` first if it exists, since the server prefers it. When tools are sent to a template without native support, the server uses a generic handler (docs: "Generic support may consume more tokens and be less efficient") and logs a warning when the template renders tool calls but does not describe tools. jinja is on by default (`--jinja`, default enabled). The HF API `?expand[]=gguf` returns chat_template, architecture and context_length without downloading, so the heuristic can run in a model browser.

Source: models/templates/*.jinja scanned locally ; docs/function-calling.md ; common/chat.cpp around lines 1525-1556 ; https://huggingface.co/api/models/unsloth/Qwen3-8B-GGUF?expand[]=gguf

### Exact KV cache size: per layer, K is (key_length x head_count_kv) x n_ctx cells and V is (value_length x head_count_kv) x n_ctx cells, each at the cache type's element size (f16 = 2 bytes by default).

Confidence: high

Source: `ggml_new_tensor_3d(ctx, type_k, n_embd_k_gqa, kv_size, n_stream)` with n_embd_k_gqa = n_embd_head_k(il) * n_head_kv(il), likewise for V. So KV_bytes = n_ctx x sum over layers of (key_length + value_length) x head_count_kv x 2 for f16. n_ctx is padded to a multiple of 256. Type defaults are GGML_TYPE_F16 for both; `-ctk`/`-ctv` accept f32, f16, bf16, q8_0, q4_0, q4_1, iq4_nl, q5_0, q5_1. Server default is 4 slots sharing one unified KV buffer (`-np -1` becomes n_parallel=4, kv_unified=true), so total cells still equal n_ctx. Check: Qwen3-30B-A3B (48 layers, head_count_kv 4, key and value 128) gives 384 MiB at 4096 and a 3456 MiB difference between 40960 and 4096; both numbers appear verbatim in llama.cpp's fit-params README log. Qwen3-8B (36 layers, kv heads 8, 128/128) is 144 KiB per token: 576 MiB at 4096, 4608 MiB at 32768.

Source: src/llama-kv-cache.cpp lines 209-234 ; src/llama-hparams.cpp n_embd_k_gqa ; src/llama-context.cpp lines 426-440 and 4455-4456 ; tools/server/server.cpp lines 126-130 ; tools/fit-params/README.md

### The plain KV formula over-estimates sliding-window and hybrid models, sometimes by several times.

Confidence: high

Sliding-window models (Gemma 3 has gemma3.attention.sliding_window=1024): with the server default swa_full=false, SWA layers get a small cache of size PAD(min(n_ctx, n_swa x (unified ? n_seq_max : 1) + n_ubatch), 256) cells instead of n_ctx. Hybrid/recurrent architectures (qwen35, qwen3next, lfm2, granite hybrid, nemotron_h, jamba and others): only attention layers get a KV cache; recurrent layers are marked by per-layer head_count_kv == 0 or by {arch}.full_attention_interval. MLA-style models keep K only. So the formula is an upper bound for these; exact numbers should come from llama.cpp itself.

Source: src/llama-kv-cache-iswa.cpp lines 69-80 ; src/llama-arch.cpp llm_arch_is_hybrid ; src/models/*.cpp (is_recr_impl) ; src/llama-model.cpp create_memory

### llama.cpp now measures required memory itself without allocating, and exposes it three ways; this beats any hand-written estimate.

Confidence: high

(1) `--fit` is on by default: it adjusts unset arguments to free device memory, leaving `--fit-target` (default 1024 MiB per device) free and never lowering context below `--fit-ctx` (default 4096). An explicit `-c N` is respected ("context size set by user ... no change"); explicit `-c 0` disables context reduction. (2) `llama-fit-params` ships in the b12242 archives I listed (llama-fit-params.exe plus llama-fit-params-impl.dll in the Windows Vulkan zip; also in the macOS arm64 and Ubuntu tarballs). Default mode prints fitted arguments such as `-c 4096 -ngl 48 ...`; `-fitp on` prints one line per device, `<device> <model MiB> <context MiB> <compute MiB>`, then a `Host ...` line. (3) Router mode with `--models-memory-margin N` exposes per-model `memory` {status, min, max, effective} in `GET /models`, in bytes per device. Documented caveats: "Host (CPU) memory is assumed to be unlimited" and mmproj is "not included in the measurements yet". Example magnitudes from the docs: a model of about 4.6 GiB had 512 MiB context at 4096 and 288 MiB compute on GPU; Qwen3-30B-A3B f16 had 898 MiB compute. On exit the server prints a `llama_memory_breakdown_print` table (total, free, model, context, compute per device).

Source: common/arg.cpp lines 2860-2917 ; common/common.h lines 494-502 ; tools/fit-params/fit-params.cpp and README.md ; common/fit.cpp common_fit_print ; tools/server/README.md "Memory-based unloading" ; archive listings of llama-b12242-bin-win-vulkan-x64.zip, -macos-arm64.tar.gz, -ubuntu-x64.tar.gz

### Partial offload places the last N layers plus the output layer on the GPU; the token-embedding layer always stays on the CPU, and each layer's KV cache follows its layer.

Confidence: high

Offloadable layer count is block_count + 1 (log line: "offloaded %d/%d layers to GPU"). GPU layers start at index max(n_layer + 1 - n_gpu_layers, 0). Source comment: "there is very little benefit to offloading the input layer, so always keep it on the CPU". KV buffers are allocated on `model.dev_layer(il)` when KV offload is on (default). So with k of L+1 layers offloaded, VRAM holds roughly k/(L+1) of the block weights and of the KV cache plus the compute buffer, and RAM holds the rest plus the embedding table. `-ngl` accepts a number, `auto` (default) or `all`. Server defaults that affect the estimate: n_batch 2048, n_ubatch 512, mmproj offloaded to GPU by default (`--mmproj-offload`), and `--cache-ram` 8192 MiB of host RAM for the prompt cache with up to 32 context checkpoints per slot.

Source: src/llama-model.cpp lines 1766-1782 and 2103-2111 ; src/llama-kv-cache.cpp lines 213-220 ; common/arg.cpp ; common/common.h lines 471-476, 619, 654-657

### `llama-server --list-devices` is a self-contained probe for usable GPU backends and their total and free memory.

Confidence: medium

It calls ggml_backend_load_all(), prints `Available devices:` and then one line per non-CPU device in the format `  <name>: <description> (<total> MiB, <free> MiB free)`, or `  (none)`, and exits 0. Device names are `Vulkan0`, `CUDA0`, `MTL0`. Vulkan total is the sum of device-local heaps; free is min(heapBudget, heap size) minus heapUsage via VK_EXT_memory_budget. Metal total is `recommendedMaxWorkingSetSize` and free is that minus currentAllocatedSize. Windows release builds load backends dynamically from the executable's directory with LoadLibraryW under SEM_FAILCRITICALERRORS, so a missing dependency does not pop a dialog; the backend is skipped with a "failed to load" log. Static import scan of ggml-vulkan.dll from b12242 shows it imports vulkan-1.dll (not shipped in the zip; it comes with GPU drivers) and the MSVC runtime (msvcp140.dll, vcruntime140.dll, vcruntime140_1.dll). Default device selection uses discrete GPUs and only falls back to integrated GPUs when no discrete GPU exists; the same card seen by two backends is de-duplicated by device id. Not run live in this session.

Source: common/arg.cpp common_print_available_devices (lines 1148-1171) ; ggml/src/ggml-vulkan/ggml-vulkan.cpp lines 15450-15484 ; ggml/src/ggml-metal/ggml-metal-device.m lines 1501-1510 ; ggml/src/ggml-backend-reg.cpp and ggml-backend-dl.cpp ; src/llama.cpp lines 186-285 ; static inspection of llama-b12242-bin-win-vulkan-x64.zip

### On Windows, DXGI gives correct dedicated VRAM above 4 GB; WMI's AdapterRAM cannot.

Confidence: high

DXGI_ADAPTER_DESC1.DedicatedVideoMemory is SIZE_T ("the number of bytes of dedicated video memory that are not shared with the CPU"); in the windows crate 0.62.2 it is `usize`, so 64-bit in an x64 process. Win32_VideoController.AdapterRAM is declared `uint32`, units bytes, mapped from registry HardwareInformation.MemorySize, so it cannot represent more than about 4 GiB; a 16 GB card will not report correctly. When calling DXGI directly, skip software adapters (Flags bit DXGI_ADAPTER_FLAG_SOFTWARE; Microsoft's vendor id is 0x1414) and expect integrated GPUs to show small DedicatedVideoMemory with large SharedSystemMemory. Vendor ids: 0x10DE NVIDIA, 0x1002 AMD, 0x8086 Intel.

Source: https://learn.microsoft.com/en-us/windows/win32/api/dxgi/ns-dxgi-dxgi_adapter_desc1 ; https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-videocontroller ; https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Graphics/Dxgi/struct.DXGI_ADAPTER_DESC1.html

### sysinfo 0.40.1 covers RAM, CPU brand and (new in 0.40.0) GPU name, vendor and dedicated VRAM via DXGI, with some API and MSRV changes to plan for.

Confidence: high

Version 0.40.1 (updated 2026-10-30), MSRV rustc 1.95, depends on `windows >=0.62, <0.63`. The `gpu` feature is new in 0.40.0 and on by default. `Gpus::new_with_refreshed_list()` then per GPU `vendor()`, `model()`, `total_memory()`, `used_memory()`, `usage()`. Windows implementation: CreateDXGIFactory1 with IDXGIFactory6, EnumAdapters1, GetDesc1; total_memory = DedicatedVideoMemory; used_memory = IDXGIAdapter3::QueryVideoMemoryInfo(DXGI_MEMORY_SEGMENT_GROUP_LOCAL).CurrentUsage; adapters that do not match a PCI device are dropped. On macOS `total_memory()` always returns None. Constructors now return Result (`System::new_all() -> Result<Self, Error>`). RAM: `total_memory()` and `available_memory()` in bytes (Windows: GlobalMemoryStatusEx ullTotalPhys / ullAvailPhys; macOS total from sysctl HW_MEMSIZE). macOS trap: available_memory is computed as (active + inactive + free) pages, which counts active pages and so overstates what is really free. Chip name: `cpus()[0].brand()` reads sysctl machdep.cpu.brand_string (for example "Apple M3 Pro"), falling back to hw.machine.

Source: cargo info sysinfo ; sysinfo-0.40.1 crate source: CHANGELOG.md, src/common/gpu.rs, src/windows/gpu.rs, src/windows/system.rs, src/unix/apple/system.rs, src/unix/apple/cpu.rs

### NVIDIA and CUDA usability can be checked with nvml-wrapper 0.13.0, which loads nvml.dll at runtime and fails cleanly when it is absent.

Confidence: medium

nvml-wrapper 0.13.0 (updated 2026-08-31, 7.75M downloads) uses libloading; `Nvml::init()` looks for "nvml.dll" on Windows and returns Err if missing. Useful calls: `sys_driver_version()`, `sys_cuda_driver_version()` (i32), `device_count()`, `device_by_index(i)` then `.name()` and `.memory_info()`. llama.cpp's release workflow builds Windows CUDA backends for CUDA 12.4 and 13.5 and publishes matching runtime zips containing cudart64_*.dll, cublas64_*.dll and cublasLt64_*.dll. NVIDIA's release notes give minimum driver branch >= 525 for CUDA 12.x and >= 580 for CUDA 13.x, with CUDA 12.4 GA listed against Windows driver >= 551.61. Checking for nvcuda.dll in System32 is a reasonable extra signal but I did not verify it against a primary source.

Source: nvml-wrapper-0.13.0 crate source (src/lib.rs) ; crates.io API ; llama.cpp .github/workflows/release.yml (windows-cuda job) ; https://docs.nvidia.com/cuda/cuda-toolkit-release-notes/index.html

### On Apple Silicon the GPU memory budget is Metal's recommendedMaxWorkingSetSize; Apple documents it as an approximation, and the exact fraction of RAM is not published.

Confidence: medium

Apple: "An approximation of how much memory, in bytes, this GPU device can allocate without affecting its runtime performance." llama.cpp uses it as the Metal device total and notes in a comment that "it's possible to allocate more than `recommendedMaxWorkingSetSize`"; it logs a warning when current allocation exceeds it. The 65-75 percent figure is community lore (a forum user's "I believe Apple has a hard coded limit of 75% of physical memory"; other reports of about two thirds on smaller machines); I found no Apple table. The value can be raised with the undocumented `sysctl iogpu.wired_limit_mb` (resets on reboot) per community guides. Because `--list-devices` prints the real value for MTL0, the app does not need to guess the fraction.

Source: https://developer.apple.com/documentation/metal/mtldevice/recommendedmaxworkingsetsize ; ggml/src/ggml-metal/ggml-metal-device.m ; https://developer.apple.com/forums/thread/732035 (community)

### RX 9070 XT (RDNA4) on Vulkan: current llama.cpp recognises RDNA4 explicitly and tunes for it; detection depends on driver-exposed extensions.

Confidence: high

ggml-vulkan classifies an AMD device as AMD_RDNA4 when it exposes VK_AMD_shader_core_properties, VK_KHR_shader_integer_dot_product and VK_EXT_subgroup_size_control with subgroup size min 32 / max 64, and also VK_EXT_shader_float8; without shader_float8 it is treated as RDNA3 or RDNA2. On AMD's proprietary driver, KHR_cooperative_matrix is enabled only for RDNA3 and RDNA4. There are RDNA4-specific paths ("Some quants are not performant on RDNA4, those fall back to FP16 matmul"; a flash-attention KV path "gated to AMD, except RDNA4, which regressed in benchmarks"). The official Windows Vulkan backend is built with Vulkan SDK 1.4.357.0. Useful environment variables: GGML_VK_VISIBLE_DEVICES, GGML_VK_DISABLE_COOPMAT, GGML_VK_DISABLE_COOPMAT2, GGML_VK_DISABLE_F16, GGML_VK_PREFER_HOST_MEMORY, GGML_VK_ALLOW_SYSMEM_FALLBACK, GGML_VK_FORCE_MAX_ALLOCATION_SIZE, GGML_VK_FORCE_MAX_BUFFER_SIZE, GGML_VK_SUBALLOCATION_BLOCK_SIZE (default 1 GiB). I found no minimum Adrenalin version stated for Vulkan in llama.cpp docs.

Source: ggml/src/ggml-vulkan/ggml-vulkan.cpp (get_device_architecture lines 3-56; lines 2439-2441, 4088-4290, 8270-8273, 16552-16563) ; .github/workflows/release.yml

### Known RX 9070 XT issues are thin: one unresolved community report of slow Qwen3.5 on Vulkan under Windows, and a Windows ROCm build that is not self-contained.

Confidence: low

Reddit r/LocalLLaMA snapshot dated 2026-03-04 (llama.cpp b8196, Windows 10, "newest" Adrenalin after a clean install): Qwen3.5 4B Q8 ran 18 t/s on Vulkan vs 71 t/s on HIP; 9B Q8 33 vs 49; GPU drew about 90 W instead of 220 W+. Qwen3 4B was fine and faster on Vulkan (102 vs 85 t/s). No root cause or fix in the thread, and I could not confirm whether later builds changed it. Separately, llama.cpp publishes a Windows ROCm/HIP build whose GPU targets include gfx1200 and gfx1201 (RDNA4), but per llama.cpp discussion #27047 it needs a separately installed ROCm runtime (hipblas.dll, rocblas.dll, rocsolver.dll "are no longer bundled") and, for ROCm 7.14, Adrenalin 26.6.4 or newer. AMD's own driver note on Vulkan llama.cpp returned HTTP 503 and Phoronix's 9070 XT Windows benchmark returned 403, so neither was read.

Source: https://reddit.sentinel-team.org/posts/1rkky2n/snapshots/2026-03-04T15%3A10%3A50.627029Z ; https://github.com/ggml-org/llama.cpp/discussions/27047 ; llama.cpp .github/workflows/release.yml (windows-rocm job)

## Recommendations

- Hand-roll the GGUF header reader in Rust (about 200 lines, BufReader + seek_relative): accept versions 2 and 3, reject big-endian, keep scalars, strings and arrays of up to about 64 elements, skip larger arrays (one seek for fixed-size types; walk length prefixes for string arrays), stop after the KV section, and cap key length and string length defensively. Make it generic over Read+Seek so the same code can run over HTTP Range for pre-download inspection.
- Classify files from metadata, not filenames: mmproj if general.architecture == "clip" (treat general.type values "mmproj" and "clip-vision" as confirmation only); secondary shard if split.no > 0 (hide it and load via the first shard); model size = sum of all shard file sizes. Pair an mmproj to a model by clip.vision.projection_dim == {arch}.embedding_length.
- Tool-capability badge in two stages: before load, scan tokenizer.chat_template.tool_use (if present) or tokenizer.chat_template for the `tools` variable and `tool_calls`, giving native / partial / generic-fallback; after load, overwrite with GET /props chat_template_caps.supports_tools and supports_tool_calls, and take vision from /props modalities.
- Memory estimation in two tiers. Tier 1 (browser, pre-download): weights = total file size; KV = n_ctx x sum over layers of (key_length + value_length) x head_count_kv x 2 bytes, handling per-layer arrays, optional keys, sliding_window and full_attention_interval; compute buffer as a stated heuristic of roughly 0.3-1 GiB; add mmproj file size when used. Tier 2 (file is local): run the bundled `llama-fit-params -m <file> -c <ctx> -fitp on` and use its per-device model/context/compute MiB, adding mmproj size yourself.
- Suggested verdict thresholds (design proposal, anchored to llama.cpp's 1024 MiB default fit margin). Discrete VRAM, using free VRAM from --list-devices: green if weights + KV + compute (+ mmproj) fits with at least 1 GiB to spare at the chosen context; yellow if it only fits by lowering context (not below 4096) or by partial offload with the remainder fitting in available RAM minus about 2 GiB; red otherwise. Apple unified memory, using the MTL0 total: green if need is at most that total minus 1 GiB; yellow up to the total or about 85 percent of RAM; red above. CPU-only: green if need is under about 60-70 percent of total RAM and under available RAM minus 2 GiB; yellow up to total RAM minus about 2-4 GiB; red above.
- Always pass an explicit `-c` to llama-server so `--fit` does not silently shrink the context to 4096, and decide deliberately between `-ngl auto` (let --fit place layers) and an explicit count. For a single-user desktop app set `-np 1` and lower `--cache-ram` from its 8192 MiB default on machines with 16 GB RAM or less.
- Hardware detection: use sysinfo 0.40.x for RAM, CPU brand and Windows GPU name/vendor/dedicated VRAM (pin the `windows` crate to 0.62 to share the dependency), and treat `llama-server --list-devices` (run once at startup, cached) as the authority on which backends work and how much memory is free. Do not use WMI AdapterRAM. On macOS do not use sysinfo's available_memory for verdicts; use total RAM and the MTL0 total.
- Default AMD cards (including the RX 9070 XT) to the Vulkan backend rather than the Windows ROCm build, since the ROCm build needs a separately installed runtime. Show the Adrenalin driver version in diagnostics, recommend current drivers, and add a short tokens-per-second self-test after first load so a slow path (as in the Qwen3.5 report) is visible to the user.
- mmproj choice: default to the F16 file (present in nearly every repo, about 0.8-1.3 GB); offer Q8_0 when it exists and VRAM is tight; skip F32 (twice the size). When several exist, follow llama.cpp's rule: same directory as the model first, then closest quant bit number.

## Risks

- Nothing was run live: executing the downloaded llama.cpp binaries was not permitted in this session, so the --list-devices line format, /props JSON and llama-fit-params output are from source at commit 3b4d6ab3 / tag b12242. Verify with one real run on Windows and macOS before freezing parsers.
- llama.cpp moves fast (tag b12242 on 2026-11-10; --fit, llama-fit-params, router memory fields and chat_template_caps are all recent). Pin the bundled build and parse defensively; stdout formats are not a stable API.
- The Windows release is not a single executable: llama-server.exe is a 9.7 KB launcher that needs llama-server-impl.dll, llama.dll, ggml*.dll, mtmd.dll and libomp.dll beside it, and ggml-vulkan.dll needs the MSVC runtime and the system vulkan-1.dll. A sidecar copied alone will not start, and a machine without the VC++ runtime or a Vulkan driver silently falls back to CPU.
- The static tool heuristic can be wrong in both directions (templates that mention tools only in comments, or that describe tools through other variable names), and the server's supports_tools can stay true when its probe render throws. Treat the badge as a hint and keep the generic fallback usable.
- The KV formula is an upper bound for sliding-window, hybrid, recurrent and MLA architectures and new architectures appear often; a tier-1 estimate can show red for a model that actually fits. The compute-buffer figure is a heuristic based on two documented examples.
- llama.cpp's own measurements exclude mmproj and assume host RAM is unlimited, so a model can pass --fit and still push a 16 GB machine into swap, especially with the 8 GiB default prompt cache.
- sysinfo's GPU API is weeks old (added in 0.40.0), requires rustc 1.95 and ties you to windows 0.62.x; if that conflicts with Tauri's dependency set, the fallback is about 40 lines of direct DXGI (EnumAdapters1 + GetDesc1).
- Free VRAM on Windows changes with other applications and the desktop compositor; a verdict computed at startup can be stale by load time. Re-query before loading.
- RX 9070 XT evidence is weak: one unresolved community report and source reading; AMD's and Phoronix's pages were unreachable. Performance on specific model families may still be poor on Vulkan under Windows.
- The Apple memory fraction (about two thirds to three quarters of RAM) is unofficial; only the value read at runtime should drive verdicts.
