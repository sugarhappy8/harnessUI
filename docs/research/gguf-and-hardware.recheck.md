# gguf-and-hardware — re-check

An independent second pass over the research file of the same name. Where the two disagree, this file is the later and better-checked one.

## Corrections

### Caveat: the caps flags default to true and are only cleared when the probe render succeeds, so a template that throws keeps supports_tools=true.

Status: corrected

Wrong for the current source. The first probe (object arguments) returns without inference on failure, but then the string-arguments probe always runs, and on failure it sets both supports_tool_calls and supports_tools to false. A template that throws on both probes reports false, not true.

Source: llama.cpp @3b4d6ab3: common/jinja/caps.cpp lines 307-322 and 400-404

### A static scan for `tools` and `tool_calls` mirrors the server flags (63 both / 3 tools-only / 4 tool_calls-only / 5 neither of 75 bundled templates); llama-server still attempts tool calls on any model through a generic fallback.

Status: corrected

The counts reproduce exactly (75: 63/3/4/5, same file names). The generic-fallback half is stale: docs/function-calling.md still describes a Generic handler, but the code at b12242 has none. Formats are CONTENT_ONLY, PEG_SIMPLE, PEG_NATIVE, PEG_GEMMA4 and PEG_MINIMAX_M3; when the template's supports_tool_calls is false the autoparser builds a content-only parser, so tool calls are neither prompted nor parsed. There is no third 'generic-fallback' badge tier in this build.

Source: llama.cpp @3b4d6ab3: common/chat.h lines 241-250; common/chat-auto-parser-generator.cpp lines 115-130; common/chat-diff-analyzer.cpp line 261; own scan of models/templates/*.jinja

### `llama-server --list-devices` prints `  <name>: <description> (<total> MiB, <free> MiB free)` per non-CPU device (Vulkan0 / CUDA0 / MTL0) and a backend that cannot load is skipped with a "failed to load" log.

Status: corrected

The printf format, exit(0), device names, Vulkan memory-budget logic and Metal recommendedMaxWorkingSetSize are verified from source (still not run live). Two corrections. (1) 'Non-CPU' includes accelerator devices: macOS builds enable the BLAS backend by default (libggml-blas is in the tarball), so expect a row like `BLAS: Accelerate (0 MiB, 0 MiB free)`; integrated GPUs are listed too, with all heaps counted. The parser must match on name prefix. (2) Release builds call ggml_backend_load_best with silent=true (NDEBUG), so a ggml-vulkan.dll that fails to load for lack of vulkan-1.dll produces no log line; the only signal is the missing device row.

Source: llama.cpp @3b4d6ab3: common/arg.cpp lines 1148-1171; ggml/src/ggml-backend-reg.cpp lines 500-600; ggml/src/ggml-blas/ggml-blas.cpp lines 321-357; ggml/CMakeLists.txt lines 95-98; ggml/src/ggml-vulkan/ggml-vulkan.cpp lines 15450-15484

### Windows release: llama-server.exe is a 9.7 KB launcher needing llama-server-impl.dll, llama.dll, ggml*.dll, mtmd.dll, libomp.dll; ggml-vulkan.dll needs the MSVC runtime and vulkan-1.dll; a machine without the VC++ runtime or a Vulkan driver silently falls back to CPU.

Status: corrected

The layout is right (llama-server.exe is 9,728 bytes; the zip has 53 files, 106 MB unpacked; ggml-vulkan.dll is 42 MB and imports vulkan-1.dll, which is not shipped). The VC++ part is wrong: every binary imports the MSVC runtime, not only the Vulkan backend. llama-server.exe and llama-fit-params.exe import VCRUNTIME140.dll; llama-server-impl.dll, llama.dll, llama-common.dll, ggml-base.dll, ggml.dll, mtmd.dll and the CPU backends import MSVCP140.dll and VCRUNTIME140(_1).dll; none of these are in the zip. Without the VC++ 2015-2022 runtime the sidecar does not start at all. Only a missing Vulkan loader degrades to CPU.

Source: Own PE import-table parse of llama-b12242-bin-win-vulkan-x64.zip (<temp workspace>); .github/workflows/release.yml windows job pack step

### sysinfo 0.40.1 (MSRV 1.95, windows >=0.62,<0.63) has a new default-on `gpu` feature: Gpus::new_with_refreshed_list(), total_memory() = DXGI DedicatedVideoMemory, used_memory() = QueryVideoMemoryInfo(LOCAL).CurrentUsage; constructors return Result; macOS total_memory() is None and available_memory over-counts.

Status: corrected

Everything checks out against the crate (0.40.0 published 2026-10-28, 0.40.1 on 2026-10-30; gpu in default features; adapters without a PCI match dropped; macOS available = active + inactive + free). One trap not flagged: DXGI CurrentUsage is 'the application's current video memory usage', so used_memory() on Windows reports this process's VRAM use (about zero for the app), not system-wide use. total minus used is not free VRAM. Compatibility is fine: tauri 2.12.5, tao 0.37.2, wry 0.57.1 and webview2-com 0.39.1 all require windows ^0.62; Tauri's own MSRV is 1.90, so sysinfo raises the toolchain floor to 1.95.

Source: sysinfo-0.40.1: Cargo.toml, CHANGELOG.md, src/windows/gpu.rs lines 105-172, src/unix/apple/gpu.rs line 236, src/unix/apple/system.rs lines 192-199; https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_4/ns-dxgi1_4-dxgi_query_video_memory_info; crates.io dependency API for tauri/tao/wry

### Known RX 9070 XT issue: one unresolved Reddit report (Qwen3.5 slow on Vulkan, Windows 10, b8196); the Windows ROCm build needs a separately installed runtime and, for ROCm 7.14, Adrenalin 26.6.4+.

Status: corrected

The Reddit snapshot says what was reported (4B: 71 t/s HIP vs 18 t/s Vulkan, b8196, no fix). Discussion #27047 (2026-08-14) does say ROCm 7.14 and Adrenalin 26.6.4, but that is stale: b12242 builds the Windows HIP backend against ROCm 10.0.0 (asset llama-b12242-bin-win-rocm-10.0-x64.zip). The zip adds ggml-hip.dll, amdhip64_7.dll, rocm_kpack.dll and amd_comgr.dll; rocblas/hipblaslt are still resolved from PATH, so a separate ROCm install is still required. I did not find the Adrenalin minimum for ROCm 10.0. The Vulkan-by-default conclusion stands.

Source: https://reddit.sentinel-team.org/posts/1rkky2n/snapshots/2026-03-04T15%3A10%3A50.627029Z; https://github.com/ggml-org/llama.cpp/discussions/27047; llama.cpp .github/workflows/release.yml lines 940-1100

### AMD driver release notes and Phoronix's RX 9070 XT Windows benchmark say nothing that contradicts Vulkan as the default.

Status: unverifiable

Phoronix returned HTTP 403 to me as well, and I found no primary llama.cpp issue for RX 9070 XT + Vulkan + Windows via search. Evidence for the target card on Windows remains one community thread plus source reading; only a run on the real card will settle it.

Source: https://www.phoronix.com/review/llama-cpp-windows-linux/5 (403); web search

## Things the research missed

### The official macOS arm64 binaries require macOS 13.3, not 13.0.

release.yml builds with -DCMAKE_OSX_DEPLOYMENT_TARGET=13.3 and the shipped llama-server Mach-O has LC_BUILD_VERSION minos 13.3.0. The app's 'macOS 13+' target must become 13.3+ (set the bundle minimum system version accordingly) or the project must build llama.cpp itself. The librdma.dylib reference in the binaries is a weak link, so it does not raise the floor further.

Source: llama.cpp .github/workflows/release.yml lines 103-118; own Mach-O parse of llama-b12242-bin-macos-arm64.tar.gz (<temp workspace>)

### Every probe and the server inherit environment variables that can change security-relevant behaviour, and the release archives ship network helpers.

llama-server reads LLAMA_ARG_* for every flag, including LLAMA_ARG_TOOLS (built-in exec_shell_command, write_file and others; default off), LLAMA_ARG_CONNECT (experimental peer-to-peer tunnel; default off) and LLAMA_ARG_UI_MCP_PROXY. GGML_BACKEND_PATH makes ggml load an arbitrary backend library. Both archives contain llama-connect (11.5 MB on Windows) and ggml-rpc-server. For an offline, no-telemetry app: spawn the sidecar, --list-devices and llama-fit-params with a scrubbed environment (drop LLAMA_ARG_*, GGML_*, keep an allowlist), and do not bundle llama-connect or ggml-rpc-server (ggml-rpc.dll can also be dropped on Windows, where backends load dynamically; on macOS libggml-rpc is a hard link dependency).

Source: llama.cpp @3b4d6ab3: common/arg.cpp lines 3417-3440, 3486-3491; ggml/src/ggml-backend-reg.cpp lines 594-598; .github/workflows/release.yml line 38 (-DGGML_RPC=ON -DLLAMA_CONNECT=ON); own archive listings

### On Windows, backend choice is decided by which ggml-*.dll files sit next to llama-server.exe, in a fixed load order.

Each Windows backend zip is the CPU zip plus one backend DLL (release-publish.yml merge step), and ggml_backend_load_all loads cuda, hip, ... vulkan, ... cpu in that order from the executable directory, de-duplicating the same card by device id (first one wins). So one sidecar directory can carry several backends, CUDA beats Vulkan on NVIDIA when both are present, and HIP would beat Vulkan on AMD if ggml-hip.dll were shipped. Pass --device explicitly (for example Vulkan0) rather than relying on defaults, also to avoid default layer-splitting across two discrete GPUs. Size matters for the NSIS installer: the Vulkan DLL is 42 MB, while CUDA needs a 276 MB zip plus a 373 MB runtime zip, which argues for an on-demand download of the CUDA backend.

Source: llama.cpp @3b4d6ab3: .github/workflows/release-publish.yml lines 77-112; ggml/src/ggml-backend-reg.cpp lines 571-598; src/llama.cpp lines 225-285; release asset sizes

### A hand-rolled GGUF reader parses untrusted files and needs hard budgets, not just string-length caps.

Counts are 64-bit and string arrays have no byte length, so a crafted file can claim 2^60 elements and make the 'walk the length prefixes' loop run forever, or drive unbounded Range fetching in the model browser. Bound kv_count and tensor_count, use checked multiplication for count x element size, cap total header bytes (real headers are under 7 MB; 64 MiB is generous), stop at end of file, and fuzz the parser. llama.cpp itself only caps at 1024^3 per string/array, which is too loose for a UI thread.

Source: llama.cpp ggml/src/gguf.cpp lines 20-21, 270-360; own measurement of real header sizes

### What happens when the estimate is wrong differs by platform and should shape the yellow/red wording.

The Vulkan backend allocates device-local memory only by default, so over-commit should surface as a failed allocation at load. On NVIDIA under Windows, the driver's CUDA sysmem fallback (introduced in 536.40 per secondary sources) can silently spill into shared system memory and run many times slower instead of failing. The server also proceeds to load when --fit reports failure. A 'loaded but slow' state is therefore possible and the post-load tokens-per-second self-test the researcher proposes is the right detector. I could not confirm the default policy from an NVIDIA primary page.

Source: llama.cpp ggml/src/ggml-vulkan/ggml-vulkan-buffers.cpp lines 220-250; common/common.cpp lines 1238-1281; secondary: https://docs.cognex.com/deep-learning_400/web/EN/deep-learning/Content/Topics/optimization/gpu-disable-shared.htm

## Final recommendations

- Keep the core decisions: hand-roll a header-only GGUF reader (Read+Seek, reusable over HTTP Range), classify by metadata (architecture == "clip" for mmproj, split.no > 0 for secondary shards), use the KV formula only as a pre-download upper bound, and treat llama.cpp's own measurement (llama-fit-params -fitp on, --fit) as the authority once a file is local. All of these survived re-checking against source at 3b4d6ab3 / b12242.
- Fix the Windows packaging assumption before writing ARCHITECTURE.md: the VC++ 2015-2022 runtime (vcruntime140.dll, vcruntime140_1.dll, msvcp140.dll) is a hard dependency of llama-server.exe and every DLL beside it, and is not in the release zip. Ship those three DLLs app-local in the sidecar directory or have the NSIS installer install the redistributable; otherwise the sidecar will not start on a clean Windows machine. vulkan-1.dll is the only dependency whose absence degrades to CPU, and it does so with no log line in release builds, so the app should check for the Vulkan loader itself to explain a missing GPU.
- State the macOS floor as 13.3 (upstream binaries have minos 13.3.0), or commit to building llama.cpp in CI with a lower deployment target and test it.
- Redesign the tool-capability badge as two states, not three. In b12242 there is no generic tool-call fallback: if /props chat_template_caps.supports_tool_calls is false, tool calls are not parsed. For such models either disable agentic tools with a clear message, offer a tool-capable template override via --chat-template-file, or implement the app's own prompt-plus-JSON-schema tool protocol. The static `tools` / `tool_calls` scan remains a good pre-load predictor, and the server's flags can be trusted more than the research said, because a template that throws in both probes reports false.
- Hardware detection: use `llama-server --list-devices` as the source of truth for usable backends and free memory, parsed by device-name prefix (MTL, Vulkan, CUDA) so the macOS `BLAS: Accelerate (0 MiB, 0 MiB free)` row and integrated GPUs are handled deliberately. Use sysinfo 0.40.x only for RAM, CPU brand and GPU name/vendor/total VRAM; never derive free VRAM from its used_memory() on Windows, which is this process's usage. sysinfo's GPU API is two weeks old and needs rustc 1.95 (Tauri 2.12.5 needs 1.90 and already uses windows ^0.62, so there is no dependency conflict); keep the 40-line direct DXGI fallback in the design.
- Spawn the sidecar and both probes with a scrubbed environment and explicit flags: strip LLAMA_ARG_* and GGML_* (LLAMA_ARG_TOOLS, LLAMA_ARG_CONNECT and GGML_BACKEND_PATH are the dangerous ones), never pass --tools / --connect / --ui-mcp-proxy, and leave llama-connect and ggml-rpc-server out of the bundle. Always pass -c, -np, --device and --cache-ram explicitly, and give llama-fit-params the same -c / -np / -ngl as the server so its numbers match.
- Ship Vulkan as the default GPU backend on Windows (ggml-vulkan.dll in the sidecar directory, no ggml-hip.dll beside it), and treat CUDA as an optional on-demand download because of its size. The Windows ROCm build is now ROCm 10.0 and still needs a separately installed runtime, so it stays out of scope.
- Treat RX 9070 XT support as unproven until run on the card: source shows deliberate RDNA4 tuning, but the only field evidence is one unresolved slow-Qwen3.5 report from March 2026 and the vendor and Phoronix pages were unreachable. Make one real run on Windows 11 with the RX 9070 XT (and one on Apple Silicon) a gate before freezing the --list-devices, /props and llama-fit-params parsers, and keep the post-load tokens-per-second self-test.
- Harden the GGUF reader for untrusted input: bounded counts, checked arithmetic, a total header byte budget, end-of-file checks, and a fuzz test. Also read key_length_swa / value_length_swa, key_length_mla and sliding_window_pattern if tier-1 estimates for Gemma-style and MLA models are meant to be close; otherwise label tier 1 clearly as an upper bound.

## Claims confirmed unchanged

- llama.cpp master is commit 3b4d6ab3 (2026-11-10) and the latest release tag is b12242; release assets are named llama-b12242-bin-win-vulkan-x64.zip, -macos-arm64.tar.gz, etc. — My own clone is at 3b4d6ab3f796 (2026-11-10) and git describe gives b12242. The release page lists llama-b12242-bin-win-vulkan-x64.zip (36.1 MB), -bin-win-cpu-x64.zip, -bin-win-cuda-12.4-x64.zip (276 MB), -bin-win-cuda-13.5-x64.zip (163 MB), -bin-win-rocm-10.0-x64.zip (255 MB) and -bin-macos-arm64.tar.gz (16 MB). The Windows CUDA runtime zips carry no tag in the name: cudart-llama-bin-win-cuda-12.4-x64.zip (373 MB) and cudart-llama-bin-win-cuda-13.5-x64.zip (407 MB).
- GGUF is v3, little-endian, with the stated header layout, type ids 0-12, no nested arrays, v1 and >3 rejected, 1024^3 limits, default alignment 32. — Matches ggml/include/gguf.h (GGUF_VERSION 3, type enum, GGUF_DEFAULT_ALIGNMENT 32) and ggml/src/gguf.cpp (version 0/1/>3 rejected, (version & 0xFFFF)==0 endianness check, empty and duplicate keys rejected, GGUF_MAX_STRING_LENGTH and GGUF_MAX_ARRAY_ELEMENTS both 1024^3).
- A header-only read costs about 6 MB for text models; tokenizer arrays must be skipped and tokenizer.chat_template comes after them. — Reproduced with my own independent Range reader on unsloth/Qwen3-8B-GGUF Q4_K_M: version 3, 32 KVs, KV section ends at byte 5,933,917 after 3 requests; tokens 2,588,260 B, merges 2,731,560 B, token_type 607,756 B; chat_template sits after the token arrays; general.file_type = 15.
- No existing Rust GGUF crate fits; versions: gguf 0.1.2, gguf-rs 0.1.8, gguf-rs-lib 0.3.4, ggus 0.5.1, llama-cpp-2 0.1.161. — All versions, dates and download counts match crates.io today. Two small gaps that do not change the hand-roll decision: gguf-rs-lib 0.3.4 also has a stream reader with a 16 MiB metadata cap, and the survey did not cover mlmf-gguf 0.7.26 (sans-IO metadata reader, but first published 2026-10-03, so too new to depend on) or llmfit-core 1.1.20 (hardware detection and model fitting, but pulls in ureq, an HTTP client).
- Metadata key names and the three typing traps (head_count scalar-or-array, optional head_count_kv, optional key_length/value_length); file_type values 1/7/15/17/18/30/32, table 0-42 plus 1024. — Key names and file_type values match gguf-py/gguf/constants.py and load_hparams (get_key_or_arr for head_count and head_count_kv; key_length defaults to n_embd/n_head). The list is incomplete for KV sizing: {arch}.attention.key_length_swa / value_length_swa, key_length_mla / value_length_mla and sliding_window_pattern also exist and change the per-layer cache size.
- Split GGUFs are named <prefix>-%05d-of-%05d.gguf (1-based); only shard 1 has model metadata; llama.cpp must be given the first shard. — Format string, split.no / split.count / split.tensors.count keys, the 'illegal split file idx ... model must be loaded with the first split' error and the regex ^(.+)-([0-9]{5})-of-([0-9]{5})$ are all present as described.
- mmproj files are identified by general.architecture == "clip"; general.type varies; clip.vision.projection_dim equals the text model embedding size; file names contain "mmproj". — MODEL_ARCH.MMPROJ maps to "clip"; the converter writes GGUFType.MMPROJ and add_vision_projection_dim(n_embd_text). My own header read of ggml-org/gemma-3-4b-it-GGUF mmproj-model-f16.gguf shows architecture clip, no general.type, projector_type gemma3, projection_dim 2560, KV section 713 bytes. HF listings confirm mmproj-model-f16.gguf, mmproj-F16/BF16/F32.gguf, mmproj-<org>_<model>-f16.gguf and <Model>.mmproj-Q8_0.gguf. Audio-only projectors carry clip.audio.projection_dim instead, so the pairing check needs both keys.
- GET /props returns chat_template_caps (supports_tools, supports_tool_calls, ...) taken from the tool_use template when present, plus chat_template, modalities, model_path, build_info, total_slots. — Field names and the tool_use preference match server-context.cpp and common_chat_templates_get_caps. chat_template_tool_use is only emitted when jinja is on and that template exists.
- KV cache per layer = (key_length + value_length) x head_count_kv x n_ctx x 2 bytes at f16; n_ctx padded to 256; server default 4 slots with unified KV; 384 MiB / 3456 MiB (Qwen3-30B-A3B) and 144 KiB per token (Qwen3-8B). — Tensor shapes, n_embd_k_gqa = n_embd_head_k x n_head_kv, GGML_PAD(n_ctx, 256), and the '-np auto -> n_parallel = 4, kv_unified = true' path are in source. Arithmetic re-done: 48 x 256 x 4 x 2 x 4096 = 384 MiB, delta to 40960 = 3456 MiB (both appear in tools/fit-params/README.md); 36 x 256 x 8 x 2 = 147,456 B per token. With an explicit -np N and no unified KV, n_ctx is divided across slots.
- The plain KV formula over-estimates sliding-window and hybrid/recurrent models. — SWA cache size is GGML_PAD(min(n_ctx, n_swa x (unified ? n_seq_max : 1) + n_ubatch), 256) unless --swa-full, and MLA models keep K only. Which layers are SWA depends on sliding_window_pattern or per-architecture defaults, so an exact tier-1 number needs per-architecture logic.
- --fit is on by default (target 1024 MiB per device, min ctx 4096), respects an explicit -c, and llama-fit-params ships in release archives with a `-fitp on` per-device output; host memory is assumed unlimited and mmproj is not measured. — Defaults confirmed in common/common.h; fit.h states only default-valued parameters are changed and n_ctx only when 0. I listed the real b12242 archives: llama-fit-params(.exe) plus llama-fit-params-impl are in both the Windows Vulkan zip and the macOS arm64 tarball. Three nuances: (1) host memory is only 'unlimited' when a GPU device exists; with zero devices --fit compares projected use with total host RAM and shrinks the context. (2) -fitp exists only in llama-fit-params and prints the placement as given (default -ngl means everything on GPU), so pass the same -np/-c/-ngl as the server. (3) The server ignores the fit status and loads anyway when fitting fails.
- Partial offload puts the last N layers plus the output layer on GPU; the input embedding stays on CPU; KV follows its layer; server defaults n_batch 2048, n_ubatch 512, mmproj offloaded, --cache-ram 8192 MiB, 32 checkpoints. — i_gpu_start = max(n_layer_all + 1 - n_gpu_layers, 0), the 'very little benefit to offloading the input layer' comment, KV on model.dev_layer(il), and all listed defaults are present.
- DXGI_ADAPTER_DESC1.DedicatedVideoMemory is 64-bit (usize in windows 0.62.2) and correct above 4 GB; WMI AdapterRAM is uint32. — windows-docs-rs for 0.62.2 shows DedicatedVideoMemory, DedicatedSystemMemory and SharedSystemMemory as usize, VendorId and Flags as u32. The AdapterRAM uint32 limit was not re-fetched; it is long-standing Microsoft documentation.
- nvml-wrapper 0.13.0 loads nvml.dll at runtime and fails cleanly; llama.cpp ships Windows CUDA 12.4 and 13.5 builds; driver minimum is 525 for CUDA 12.x and 580 for 13.x, 551.61 for 12.4 GA on Windows. — LIB_PATH is "nvml.dll", Nvml::init() returns Result, and sys_driver_version / sys_cuda_driver_version / device_count / device_by_index exist. release.yml builds cuda 12.4 and 13.5 for x64. NVIDIA's table gives 13.x >= 580 and 12.x >= 525, 12.4 GA Windows >= 551.61, and states the driver is backward compatible with older toolkits.
- On Apple Silicon the Metal budget is recommendedMaxWorkingSetSize, documented as an approximation; llama.cpp uses it as the MTL0 total. — Apple's abstract matches the quote word for word; ggml-metal-device.m sets total to recommendedMaxWorkingSetSize and has the 'possible to allocate more' comment. The 65-75 percent figure and iogpu.wired_limit_mb remain community lore, as the researcher said.
- RX 9070 XT: ggml-vulkan detects AMD_RDNA4 via VK_EXT_shader_float8 plus subgroup sizes 32/64, has RDNA4-specific paths, and is built with Vulkan SDK 1.4.357.0; the listed GGML_VK_* variables exist. — Detection logic, the 'Some quants are not performant on RDNA4' and 'except RDNA4, which regressed in benchmarks' comments, VULKAN_VERSION 1.4.357.0 and all nine named environment variables are present. By default the backend requests device-local memory only; GGML_VK_ALLOW_SYSMEM_FALLBACK is opt-in.
