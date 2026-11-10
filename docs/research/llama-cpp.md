# llama-cpp — research

## Summary

Verified on 2026-11-10 against the upstream source (git clone of ggml-org/llama.cpp at tags b12242, b12110 and v0.9.0, kept in <temp workspace>), the GitHub release pages, the winget manifest, and the official Hugging Face install bucket. Nothing was written to the repository working directory.

Not verified: the file lists inside the GitHub zips and tarballs. This session's proxy blocks GitHub asset and API downloads for that repo, so those lists are derived from the release workflow and CMake files. No llama.cpp binary was executed, so behaviour claims come from reading source.

The points that matter most for ARCHITECTURE.md:

- **Release model changed.** llama.cpp now has semver stable tags; the latest is v0.9.0 (2026-11-04), which maps to nightly build b12110. Binaries are attached only to nightly `bNNNN` prereleases; the latest is b12242 (2026-11-10).
- **Defaults have moved.** Default port is 9931 (not 8080), `--jinja` is on, `-c` defaults to 0 (model context, then reduced by `--fit on`), `-ngl` defaults to `auto`, `--parallel` auto means 4 slots, and `--cache-ram` defaults to 8192 MiB.
- **llama-server is no longer one file.** It is a thin executable over `llama-server-impl` plus about 6 link-time shared libraries and dynamically loaded `ggml-*` backends. On macOS the GitHub build resolves them via `@rpath` = `@loader_path`.
- **A second official channel exists.** The HF bucket `ggml-org/install.sh` ships single-file `llama` binaries (run as `llama serve`). I downloaded and inspected the macOS Metal one: it links only system frameworks, needs macOS 13.3, and is ad-hoc signed.
- **Load progress is not available in standalone mode.** `/health` is only 503 or 200, and the server suppresses the old log dots. Progress (0..1 with stages) exists only in router mode via `GET /models/sse`, or through the internal router-child stdout protocol.
- **Tool calling was rewritten.** A differential "auto-parser" plus PEG parser replaced the old native handlers, and the "Generic" fallback is gone. A template without tool support yields no tool calls at all.
- **A missing GPU runtime does not crash the zip builds.** A backend DLL that fails to load is skipped silently in Release builds and the server runs on CPU.
- **Licensing is simple.** llama.cpp is MIT; curl is no longer a dependency (cpp-httplib with static BoringSSL); Windows zips ship `libomp.dll` with its licence file; the Vulkan loader is not bundled; CUDA runtime DLLs fall under the NVIDIA EULA.

## Findings

### llama.cpp now uses semver stable releases on top of nightly builds; latest stable is v0.9.0 = nightly b12110, latest nightly is b12242.

Confidence: high

Stable tags run v0.1.0 (2026-08-17) through v0.7.0 (10-15), v0.8.0 (10-28) and v0.9.0 (2026-11-04, commit cd959bae, marked Latest). The v0.9.0 release object carries only `nightly-tag.txt` (7 bytes, content b12110) plus source archives. Binaries live on the nightly prereleases `bNNNN`, each with 38 uploaded assets; b12242 was published 2026-11-10. Tag b12110 and v0.9.0 resolve to the same commit. CMake version is 0.9.0 with ggml 0.29.0; nightly builds append `-dev` because `LLAMA_BUILD_IS_DEV` defaults ON. Download URL pattern: https://github.com/ggml-org/llama.cpp/releases/download/<tag>/<asset>. Server flags are identical between b12110 and b12242 except one added MoE flag.

Source: docs/release.md, .github/workflows/make-release.yml and release-publish.yml at b12242; git ls-remote tags; https://github.com/ggml-org/llama.cpp/releases and /releases/tag/v0.9.0

### Exact prebuilt asset names and download sizes for the targets we need.

Confidence: high

For b12110 (v0.9.0): Windows x64 Vulkan `llama-b12110-bin-win-vulkan-x64.zip` 35.9 MB; Windows x64 CPU `llama-b12110-bin-win-cpu-x64.zip` 23.6 MB; Windows x64 CUDA 12 `llama-b12110-bin-win-cuda-12.4-x64.zip` 274 MB plus runtime `cudart-llama-bin-win-cuda-12.4-x64.zip` 373 MB; Windows x64 CUDA 13 `llama-b12110-bin-win-cuda-13.4-x64.zip` 161 MB plus `cudart-llama-bin-win-cuda-13.4-x64.zip` 404 MB; macOS arm64 Metal `llama-b12110-bin-macos-arm64.tar.gz` 15.9 MB. For b12242 the names are the same with the new tag, except CUDA 13 moved to 13.5 (`...-win-cuda-13.5-x64.zip` 163 MB, `cudart-llama-bin-win-cuda-13.5-x64.zip` 407 MB); Vulkan is 36.1 MB, CPU 23.8 MB, CUDA 12.4 276 MB, macOS arm64 16 MB. The cudart zips have no build tag in their name, and the CUDA minor version changes between builds, so resolve assets by pattern. The release page shows a SHA-256 per asset and links a build attestation. Sizes were read from the rendered asset list through a summarising fetch.

Source: https://github.com/ggml-org/llama.cpp/releases/expanded_assets/b12110 and /b12242; .github/workflows/release.yml and release-publish.yml

### Windows zips: llama-server.exe is a thin executable that needs a set of DLLs beside it; every backend zip is the CPU zip plus one backend DLL.

Confidence: medium

The publish step injects the full windows-cpu zip into each backend zip, so the Vulkan zip is the CPU zip plus `ggml-vulkan.dll`, and the CUDA zip is the CPU zip plus `ggml-cuda.dll`. Files sit flat at the zip root. Derived from CMake (not verified by extraction), llama-server.exe needs: `llama-server-impl.dll`, `llama-common.dll`, `mtmd.dll`, `llama.dll`, `ggml.dll`, `ggml-base.dll`, `libomp.dll` (LLVM OpenMP 20.1.8, with `LICENSE-LLVM-OpenMP`). Dynamically loaded backends: `ggml-cpu-{x64,sse42,sandybridge,ivybridge,piledriver,haswell,skylakex,cannonlake,cascadelake,icelake,cooperlake,zen4,alderlake,sapphirerapids}.dll`, `ggml-rpc.dll`, and the GPU DLL. Backends are searched only in the executable directory and the current working directory (or `GGML_BACKEND_PATH`). `vulkan-1.dll` is not in the zip. The cudart zip holds `cudart64_*.dll`, `cublas64_*.dll`, `cublasLt64_*.dll`. The zip also carries about 20 other tools, the unified `llama.exe`, `*-impl.dll` for other tools, and a prebuilt `llama-connect.exe` (peer-to-peer tunnel, used only with `--connect`). The winget manifest declares a dependency on `Microsoft.VCRedist.2015+.x64` and `ArchiveBinariesDependOnPath: true`. The Windows pack step does not copy LICENSE; binaries are not Authenticode-signed (no signing step in the workflows).

Source: .github/workflows/release.yml (windows-cpu, windows, windows-cuda jobs), release-publish.yml merge step, tools/server/CMakeLists.txt, common/CMakeLists.txt, ggml/src/CMakeLists.txt, ggml/src/ggml-backend-reg.cpp; winget manifest ggml.llamacpp b12121

### macOS arm64 GitHub tarball uses @rpath dylibs resolved through @loader_path, requires macOS 13.3, and is not Developer ID signed.

Confidence: medium

Build flags: `-DGGML_METAL_EMBED_LIBRARY=ON -DCMAKE_OSX_DEPLOYMENT_TARGET=13.3 -DCMAKE_INSTALL_RPATH='@loader_path' -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON -DLLAMA_BUILD_BORINGSSL=ON`, shared libs ON, no dynamic backend loading. The tarball has a top-level directory `llama-<tag>/` containing executables, dylibs and LICENSE. Derived dylib set (not verified by extraction): `libllama-server-impl.dylib`, plus versioned libraries with symlink chains such as `libllama.0.9.0.dylib` / `libllama.0.dylib` / `libllama.dylib`, and likewise `libllama-common`, `libmtmd`, `libggml` (0.29.0), `libggml-base`, `libggml-cpu`, `libggml-metal`, `libggml-blas`, `libggml-rpc`. Binaries reference the soname form (`@rpath/libllama.0.dylib`), so the dylibs must sit in the same directory as llama-server under those names; Tauri externalBin alone copies only the executable. There is no libomp on macOS and the Metal shader library is embedded. Every Mach-O file must be signed with hardened runtime for notarization.

Source: .github/workflows/release.yml (macos-cpu job); src/CMakeLists.txt, common/CMakeLists.txt, tools/mtmd/CMakeLists.txt, ggml/src/CMakeLists.txt (VERSION/SOVERSION); no codesign or notarize step in any workflow

### A second official channel ships single-file `llama` binaries (subcommand `llama serve`) from the HF bucket ggml-org/install.sh; the macOS Metal build is self-contained.

Confidence: high

llama.app/install.sh and install.ps1 download from https://huggingface.co/buckets/ggml-org/install.sh/resolve/<version>/<arch>/<os>/<backend>/<config>/llama-app(.exe).zst. `resolve/latest` returned b12182; b12110 is also present; b12242 is not. Listing API: https://huggingface.co/api/buckets/ggml-org/install.sh/tree/<prefix>. macOS: `aarch64/macos/metal/{m1,m2,m3,m4,m5,a18}/llama-app.zst`, about 8.3 MB compressed. I decompressed the b12182 m1 file: 25,126,968 bytes, links only CoreFoundation, Security, Accelerate, Foundation, Metal, MetalKit, libSystem, libc++ and libobjc, has no rpaths, minos 13.3, and an ad-hoc signature (CodeDirectory only). Windows x64: 32 CPU-feature-coded variants each for `cpu/` and `vulkan/` (for example `vulkan/kkkkk/llama-app.exe.zst`, 12.5 MB compressed, 51.0 MB exe), `cuda/{12,13}/<compute capability>/` at 27-51 MB, plus rocm. The variant is chosen by running `featcode.exe` (encodes f16c, avx2, bmi2, avx-vnni, avx512*, amx*). The Windows Vulkan exe statically imports `vulkan-1.dll` plus KERNEL32, ADVAPI32, CRYPT32, WS2_32 and UCRT api-sets, with no VCRUNTIME dependency; it is a console-subsystem binary. The unified binary's `serve` command is the same `llama_server()` entry point; `llama licenses` prints embedded licences (llama.cpp, cpp-httplib, BoringSSL, jsonhpp); `llama update` only runs when invoked.

Source: https://llama.app/install.sh and install.ps1; HF bucket API listing; llvm-objdump and llvm-readobj on the downloaded b12182 binaries; app/llama.cpp

### Current llama-server flags and defaults for the ones we need (identical at b12110 and b12242).

Confidence: high

`-m, --model`; `-mm, --mmproj FILE` (also `--no-mmproj-offload`, `--image-min-tokens`, `--image-max-tokens`); `-c, --ctx-size` default 0 = from model; `-fit, --fit [on|off]` default on, adjusts unset arguments to fit device memory, with `-fitc` minimum context 4096 and `-fitt` margin 1024 MiB; `-ngl, --n-gpu-layers N|auto|all` default auto; `-dev, --device <dev1,..>` (`none` disables offload) and `--list-devices`; `-t, --threads` default -1; `-np, --parallel` default -1 = auto, which becomes 4 slots with unified KV; `--host` default 127.0.0.1 (also accepts UNIX socket paths ending in .sock); `--port` default 9931 (0 picks any free port); `--api-key KEY` (comma-separated, env `LLAMA_API_KEY`) and `--api-key-file`; `--ui, --webui, --no-ui, --no-webui` default enabled; `--jinja, --no-jinja` default enabled; `--chat-template`, `--chat-template-file`, `--chat-template-kwargs`; `--reasoning-format none|deepseek|deepseek-legacy` (help text says default auto, but the struct default is DEEPSEEK); `-rea, --reasoning [on|off|auto]`, `--reasoning-budget N`, `--reasoning-effort`; `--cache-reuse N` default 0; `--context-shift` default disabled; `-fa, --flash-attn [on|off|auto]` default auto; `-cram, --cache-ram` default 8192 MiB; `-ctxcp` default 32 checkpoints per slot; `-to, --timeout` default 3600 s; `--sse-ping-interval` default 30 s; `--slots` default enabled; `--props` and `--metrics` default disabled; `--cors-origins` default `*` with credentials; `--offline`; `--sleep-idle-seconds`; `--media-path`; `--log-jsonl`; `-lv`. Most have `LLAMA_ARG_*` env equivalents and arguments win over env. Experimental agent features are off by default: `--tools`, `--mcp-servers-config` / `--mcp-servers-json` (stdio MCP only), `--agent`, `--connect`.

Source: tools/server/README.md auto-generated help table at b12242 and b12110; common/common.h (port = 9931, use_jinja = true, reasoning_format = DEEPSEEK); tools/server/server.cpp server_prepare_params

### Router (multi-model) mode exists and is entered by starting llama-server with no model.

Confidence: high

Sources are the cache (`LLAMA_CACHE`), `--models-dir PATH` (multimodal models go in a subfolder whose projector filename starts with `mmproj`), or `--models-preset file.ini`. Related flags: `--models-max` (default 4), `--models-memory-margin`, `--models-autoload` (default on). Endpoints: `GET /models` (status values unloaded, loading, loaded, sleeping, downloading, incomplete, plus `failed` and `exit_code`), `POST /models/load`, `POST /models/unload`, `GET /models/sse`, `POST /models` (download), `DELETE /models`. Requests are routed by the `model` field in POST bodies or the `?model=` query on GETs. The router spawns one child llama-server per model bound to 127.0.0.1.

Source: tools/server/README.md section 'Using multiple models'; tools/server/server.cpp server_run_router

### /health returns 503 while loading, and so does every other API endpoint; only /health, /v1/health and UI assets skip the API key.

Confidence: high

While the model loads, a middleware returns HTTP 503 with `{"error":{"message":"Loading model","type":"unavailable_error","code":503}}` for all non-UI paths. When ready, `GET /health` returns 200 `{"status":"ok"}` with no progress field. The HTTP socket is bound before model load. `/props`, `/models`, `/v1/models` and `/slots` require the key when one is set. The key is accepted as `Authorization: Bearer <key>` or `X-Api-Key`; failure is 401 `{"error":{"message":"Invalid API Key","type":"authentication_error","code":401}}`. Errors use the OpenAI shape with types invalid_request_error (400), authentication_error (401), not_found_error (404), server_error (500), not_supported_error (501), unavailable_error (503) and exceed_context_size_error (400).

Source: tools/server/server-http.cpp (middleware_validate_api_key, middleware_server_state); server-context.cpp get_health; server-common.cpp format_error_response; README /health section

### /v1/chat/completions streaming: tool_calls deltas, optional usage chunk, timings object and prompt-progress are all present.

Confidence: high

SSE `data:` chunks end with `data: [DONE]`. The first chunk has delta `{role:"assistant",content:null}`. Deltas may carry `reasoning_content`, `content`, or `tool_calls:[{index,id,type:"function",function:{name,arguments}}]`; id and name come first, then argument fragments. The final chunk has `finish_reason` of `stop`, `length` or `tool_calls`. With `stream_options:{include_usage:true}` an extra last chunk has `choices:[]` and `usage` (`prompt_tokens`, `completion_tokens`, `total_tokens`, `prompt_tokens_details.cached_tokens`). `timings` is attached to the last chunk and to non-streaming responses: `cache_n`, `prompt_n`, `prompt_ms`, `prompt_per_token_ms`, `prompt_per_second`, `predicted_n`, `predicted_ms`, `predicted_per_token_ms`, `predicted_per_second`; tokens in context = prompt_n + cache_n + predicted_n. `timings_per_token:true` adds timings to every chunk. `return_progress:true` emits `prompt_progress:{total,cache,processed,time_ms}` during prompt evaluation. SSE comment pings (`:` followed by a blank line) are sent every 30 s of silence. Mid-stream errors arrive as `data: {"error":{...}}` and the stream ends without `[DONE]`. `logprobs` with tools and stream is rejected. `tool_choice` is read as a string (auto, none, required). A prompt that exceeds the context returns 400 exceed_context_size_error with the message 'request (N tokens) exceeds the available context size (M tokens), try increasing it'.

Source: tools/server/server-task.cpp (to_json_oaicompat_chat, to_json_oaicompat_chat_stream), server-chat.cpp server_chat_msg_diff_to_json_oaicompat, server-context.cpp stream loop, server-common.cpp oaicompat_chat_params_parse, server-schema.cpp; README timings section

### /props exposes the chat template, per-slot context, modalities and tool-support capabilities; /v1/models, /slots, /tokenize, /apply-template and token-count endpoints behave as listed.

Confidence: high

`GET /props` returns: `default_generation_settings.n_ctx` (per-slot context) and `.params`, `total_slots`, `model_alias`, `model_path`, `model_ftype`, `modalities:{vision,video,audio}`, `media_marker`, `chat_template`, `chat_template_tool_use` (when present), `chat_template_caps`, `bos_token`, `eos_token`, `build_info`, `is_sleeping`, `endpoint_slots/props/metrics`, `ui`, `ui_settings`. `chat_template_caps` keys: `supports_tools`, `supports_tool_calls`, `supports_parallel_tool_calls`, `supports_system_role`, `supports_string_content`, `supports_typed_content`, `supports_preserve_reasoning`, `supports_reasoning_effort`, `supports_object_arguments`, `reasoning_effort_levels`. `GET /v1/models` has a single entry with `architecture.input_modalities` / `output_modalities` and `meta{vocab_type,n_vocab,n_ctx,n_ctx_train,n_embd,n_params,size,ftype}`, plus a legacy `models[0].capabilities` of [completion, multimodal]. `GET /slots` (on by default; `?fail_on_no_slot=1` gives 503 when busy) lists per slot `id`, `id_task`, `n_ctx`, `is_processing`, `params`, `next_token`. `POST /tokenize` takes `content`, `add_special`, `parse_special`, `with_pieces` and returns `{tokens:[...]}`. `POST /apply-template` accepts a full chat-completions body including `tools` and returns `{prompt}`. `POST /v1/chat/completions/input_tokens` returns `{object:"response.input_tokens",input_tokens:N}`. Also available: `/v1/responses`, Anthropic-compatible `/v1/messages`, and `/v1/chat/completions/control`.

Source: tools/server/server-context.cpp (get_res_props, get_res_model_info, post_apply_template); common/jinja/caps.cpp; common/chat.cpp common_chat_templates_get_caps; tools/server/README.md

### Standalone llama-server gives no parseable model-load progress; progress exists only in router mode or via the internal router-child protocol.

Confidence: high

The server always installs its own load-progress callback, which replaces libllama's default dot printer, and that callback only emits when a state callback is set. The state callback is set only when the process is router-managed. Standalone, the observable signals are the `/health` 503 to 200 transition and INFO lines on stderr such as `loading model '<name>'`, `load_tensors: offloaded N/M layers to GPU`, `model loaded`, `listening on http://127.0.0.1:<port>`; the log prefix is the truncated function name and the wording has changed across versions. In router mode, `GET /models/sse` emits `{model, event:"model_status", data:{status:"loading", progress:{stages:["text_model","spec_model","mmproj_model"], current, value}}}` with value from 0 to 1, throttled to about one per 200 ms, then `status:"loaded"`. The docs note mmap can misreport progress and suggest `--load-mode none` for exact values. Internally, any llama-server started with env `LLAMA_SERVER_ROUTER_PORT` set acts as a managed child: it writes lines `cmd_child_to_router:state:{"state":"loading|ready|sleeping","payload":{...}}` on its original stdout (logs go to stderr), exits gracefully on a stdin line `cmd_router_to_child:exit`, and calls exit(1) on stdin EOF. The env value itself is not read. This protocol is described only in README-dev.md. `--log-jsonl` writes logs as `{type:"log",time,level,msg}` lines on stdout.

Source: tools/server/server-context.cpp (load_progress_callback, load_model); tools/server/server.cpp server_run_instance; server-common.cpp server_role; server-common.h; src/llama.cpp lines 411-426; README /models/sse; README-dev.md 'Router mode: how child <--> router communicates'; common/log.cpp

### Tool calling now runs through a differential auto-parser plus PEG parser; the old Generic fallback no longer exists.

Confidence: high

With Jinja on (the default), the server renders the model's template (or its `tool_use` variant, or `--chat-template-file`) and analyses it by diffing renders to classify the tool format as JSON_NATIVE, TAG_WITH_JSON or TAG_WITH_TAGGED and to find reasoning and content markers. It then builds a PEG parser and a GBNF grammar. Dedicated handlers remain for a few templates (GPT-OSS/Harmony, Functionary v3.2, Ministral/Magistral, Gemma 4, MiniMax M3). Chat formats are only Content-only, peg-simple, peg-native, peg-gemma4, peg-minimax-m3. The tool parser is built only when tools are present, `tool_choice` is not none, and the template's `supports_tool_calls` is true; otherwise output is parsed as plain content. Lazy grammar: with `tool_choice:auto` the grammar is lazy and triggered by the tool section or per-call start marker (for example `<tool_call>`); with `required` the grammar applies from the start; with no detectable trigger marker and `auto` there is no grammar constraint. Lazy grammar is not applied while the reasoning-budget sampler is inside a thinking block. `parallel_tool_calls` defaults to the template's `supports_parallel_tool_calls` capability. Tool-call ids are generated when the template has none. docs/function-calling.md still describes Generic and 'disabled by default' and is marked outdated by its own TODO. The repo ships 76 templates under models/templates/.

Source: docs/autoparser.md; common/chat-auto-parser-generator.cpp generate_parser and build_parser; common/chat.cpp; common/chat.h enum common_chat_format; common/sampling.cpp grammar_should_apply; tools/server/server-common.cpp line 1466

### Known tool-calling failure modes, and the status of streaming with tools.

Confidence: medium

(1) If final output does not match the parser, the server logs `unparsed peg-native output: ...` and returns a 500 server_error with message `The model produced output that does not match the expected peg-native format`; in streaming this arrives as an error event after partial deltas were already sent. (2) Thinking models (Qwen3.5/3.6, GLM-4.7-Flash) emitting the tool call inside the thinking block: issue #20837 is still open; reported workarounds are disabling thinking or `--reasoning-format none`. (3) Text before `<tool_call>` produced 'Failed to parse input at pos N' in spring 2026 builds (issue #20260, closed as not planned). (4) A template that supports tool calls but does not describe tools logs 'Template supports tool calls but does not natively describe tools...'; an undeterminable format logs 'failed to determine tool format' and tool calls stop working. (5) Extreme KV quantisation (for example `-ctk q4_0`) degrades tool calling per the docs. (6) Object-form `tool_choice` logs a type warning per a user report in the changelog issue. Streaming with tools is a first-class path: partial PEG parsing produces incremental deltas and the test suite parametrises tool tests over NORMAL and STREAMED for about 16 small models (Qwen2.5 1.5B/7B, Llama 3.2 1B/3B, Hermes 2/3, Mistral Nemo, Phi-3.5, gemma-2-2b and others).

Source: common/chat.cpp line 1839-1841 and 1551-1555; common/chat-auto-parser-generator.cpp; https://github.com/ggml-org/llama.cpp/issues/20837 and /issues/20260; changelog issue #9291; tools/server/tests/unit/test_tool_call.py; docs/function-calling.md

### Reasoning is returned in `reasoning_content` (message and streaming delta) by default.

Confidence: high

The effective default is the DeepSeek style: thoughts are extracted into `message.reasoning_content` and `delta.reasoning_content`. `none` leaves them inline in content; `deepseek-legacy` keeps the tags in content and also fills reasoning_content. Per request: `reasoning_format`, `chat_template_kwargs` (for example `{"enable_thinking":false}`), `reasoning_effort` (`none` disables thinking; other values go to the template), `reasoning_budget_tokens` / `thinking_budget_tokens`, and `reasoning_control:true` with `POST /v1/chat/completions/control {id, action:"reasoning_end"}` to cut thinking short. Server-side: `--reasoning on|off|auto`, `--reasoning-budget N` (-1 unlimited, 0 immediate end), `--reasoning-preserve`. Reasoning in history is accepted for templates that support it.

Source: tools/server/README.md (reasoning flags and Chat Completions options); common/common.h; tools/server/server-common.cpp lines 1488-1551; server-chat.cpp

### Vision input is `image_url` content parts with base64 data URIs and requires `--mmproj`; limits are listed.

Confidence: high

Content part `{type:"image_url", image_url:{url}}` where url is `data:image/...;base64,<data>`, raw base64, an http(s) URL, or `file://` (only with `--media-path`). The data-URI check requires a prefix of `data:image/`, `data:video/` or `data:audio/`, a `base64` suffix, and exactly one comma. Without a projector the request fails with 'image input is not supported - hint: if this is unexpected, you may need to provide the mmproj'. Decoding uses stb_image (jpeg, png, tga, bmp, gif); WebP is only decoded through an external ffmpeg. Remote URLs are fetched by the server itself with a 10 MB cap and 10 s timeout, and this code path does not consult `--offline`. HTTP body limit is cpp-httplib's 100 MB default. Image token cost is model-dependent (`--image-min-tokens`, `--image-max-tokens`); for projectors needing non-causal attention the image must fit in one ubatch (default 512) and the server caps image_max_tokens with a warning; the docs note some models need a larger context such as `-c 8192`. Capability check: `/props.modalities.vision` or `/v1/models` `architecture.input_modalities` containing `image`. Audio (`input_audio`) and video (`input_video`, needs ffmpeg) also exist. Multimodal support is labelled experimental.

Source: tools/server/server-common.cpp (handle_media, oaicompat_content_load_media); tools/server/README.md multimodal section; docs/multimodal.md; tools/mtmd/mtmd-helper.cpp; vendor/cpp-httplib/httplib.h; tools/server/server-context.cpp lines 1378-1390

### Process facts: binds 127.0.0.1 by default, port-in-use is a logged bind failure with exit code 1, and graceful shutdown differs per OS.

Confidence: high

Default host is 127.0.0.1 and default port 9931. Bind failure logs `couldn't bind HTTP server socket, hostname: <h>, port: <p>` then `exiting due to HTTP server error` and returns 1. The server sets SO_REUSEADDR explicitly; on Windows that option can let a second process bind an in-use port without error (OS semantics, not tested here). Exit code 1 also covers argument errors (`error: invalid argument: ...`, `error while handling argument ...`, `invalid device: X`), model load failure, a second interrupt, and stdin EOF in managed-child mode. Exit 0 follows a graceful stop and `--list-devices`, `--help`, `--version`. Signals: SIGINT and SIGTERM trigger graceful shutdown on Unix; on Windows only CTRL_C_EVENT is handled, so a plain kill is a hard TerminateProcess. Handlers are installed only after the model has loaded. There is no HTTP shutdown endpoint. Assertion failures (`GGML_ABORT`, including CUDA runtime errors printed as `CUDA error: ...`) call abort(). Logs go to stderr by default. `--list-devices` prints `Available devices:` then lines `  <Name>: <description> (<total> MiB, <free> MiB free)` or `  (none)` on stdout and exits 0.

Source: common/common.h; tools/server/server-http.cpp start() and socket options; tools/server/server.cpp (signal_handler, server_install_signal_handlers, server_run_instance); common/arg.cpp (parse_device_list, common_print_available_devices); ggml/src/ggml-cuda/ggml-cuda.cu ggml_cuda_error

### Missing or outdated Vulkan or CUDA runtime does not crash the GitHub zip builds; the backend is skipped silently and the server runs on CPU.

Confidence: medium

With dynamic backend loading, `ggml_backend_load_all` runs with silent = true in Release builds, so a backend DLL that cannot be loaded (for example `ggml-vulkan.dll` without `vulkan-1.dll`, or `ggml-cuda.dll` without `cudart64_*.dll` / `cublas64_*.dll` or without an NVIDIA driver) produces no error line. The Vulkan backend also returns no registration if instance init throws (for example 'ggml_vulkan: Error: Vulkan 1.2 required.'), and logs 'ggml_vulkan: No devices found.' at info level. Observable signatures: no `loaded <X> backend from <path>` info line, `--list-devices` shows `(none)` or lacks `Vulkan0` / `CUDA0`, and the load log lacks `offloaded N/M layers to GPU`. Passing `--device Vulkan0` when it is absent fails fast with `invalid device: Vulkan0` and exit 1. By contrast, the HF single-file Vulkan exe statically imports `vulkan-1.dll`, so the Windows loader refuses to start it when the DLL is missing; the same applies to any missing import-linked DLL in the zip layout (llama.dll, ggml.dll, libomp.dll, VC runtime). The official installer handles this with probe executables (`vulkan-probe`, `cuda-probe`; cuda-probe exit code 5 means GPU present but CUDA Toolkit not installed).

Source: ggml/src/ggml-backend-reg.cpp (ggml_backend_load_all_from_path, load_backend, ggml_backend_load_best); ggml/src/ggml-vulkan/ggml-vulkan.cpp; common/arg.cpp; llvm-readobj imports of the HF Vulkan exe; https://llama.app/install.ps1

### Out-of-memory signatures at load are log lines followed by exit code 1; default `--fit on` reduces the chance.

Confidence: medium

Device allocation failure chain on stderr: backend line such as `ggml_backend_cuda_buffer_type_alloc_buffer: allocating X MiB on device 0: cudaMalloc failed: out of memory`, or Metal `failed to allocate buffer, size = X MiB` / `failed to allocate Metal buffer of N bytes (out of memory)`; Vulkan allocation failure returns null without its own line. Then `failed to allocate <buft> buffer of size N`, `llama_model_load: error loading model: unable to allocate <buft> buffer`, `failed to load model`, server `failed to load model, '<path>'` or `failed to create_context with model '<path>'`, and finally `exiting due to model loading error` with exit code 1. Context-stage failures read `failed to allocate compute pp buffers` or `failed to initialize the context: ...`. With `--fit on` and unset `-c` / `-ngl`, context (down to 4096) and layer count are reduced to fit free device memory, leaving a 1024 MiB margin per device; host RAM is treated as unlimited. Runtime CUDA errors after load abort the process. In router mode a failed child shows `status:{value:"unloaded",failed:true,exit_code:N}`.

Source: ggml/src/ggml-backend.cpp, ggml-alloc.c, ggml-cuda/ggml-cuda.cu, ggml-metal/*.m and *.cpp; src/llama.cpp, src/llama-model.cpp, src/llama-context.cpp; common/common.cpp; tools/server/server-context.cpp and server.cpp; README --fit flags

### Licence: llama.cpp is MIT; bundled runtime pieces and their terms are as follows, and curl is no longer involved.

Confidence: high

llama.cpp: MIT, 'Copyright (c) 2023-2026 The ggml authors'; ship the copyright and permission notice. The Windows zips do not include LICENSE, so add it from the repo at the pinned tag. Statically linked and embedded third parties: cpp-httplib (MIT, 2017 yhirose), BoringSSL 0.20261105.0 (its LICENSE file is embedded), nlohmann/json (MIT, licenses/LICENSE-jsonhpp); also vendored stb_image, miniaudio, sheredom subprocess.h and hash code whose licences I did not read. `llama licenses` in the unified binary prints the embedded texts. LLVM OpenMP `libomp.dll` 20.1.8 ships in Windows zips with `LICENSE-LLVM-OpenMP`; keep that file with the DLL. libcurl: `LLAMA_CURL` is deprecated and no curl is linked. Vulkan loader: `vulkan-1.dll` is not bundled; it comes from the GPU driver. CUDA runtime: NVIDIA CUDA EULA (page header v13.5) lists the CUDA Runtime (cudart) and CUDA BLAS Library (cublas, cublasLt) as redistributable under section 1.1.2 conditions: the application must have material additional functionality, the DLLs may be accessed only by your application, and your distribution terms must be consistent with the EULA. Web UI assets are embedded in the server binary.

Source: LICENSE; cmake/license.cmake; app/CMakeLists.txt and app/llama.cpp; vendor/cpp-httplib/CMakeLists.txt; ggml/src/CMakeLists.txt (GGML_OPENMP_FETCH); CMakeLists.txt (llama_option_depr LLAMA_CURL); https://docs.nvidia.com/cuda/eula/index.html; embedded 'License for ...' strings in the HF binaries

### Several defaults are unsuitable for a single-user desktop sidecar and should be overridden explicitly.

Confidence: high

(a) `--parallel` auto creates 4 slots with a unified KV cache. (b) `--cache-ram` defaults to 8192 MiB of host RAM for prompt caching, with up to 32 context checkpoints per slot. (c) No `-c` means the context is whatever `--fit` picks at load time, so it varies with free VRAM. (d) Default CORS reflects any Origin with credentials; the server logs a security warning when no API key is set. (e) The embedded Web UI is served at `/` unless `--no-ui`. (f) An `--api-key` argument is visible in the process list; the env var `LLAMA_API_KEY` or `--api-key-file` avoids that. (g) `-hf`, `-mu`, `-dr` and the `--*-default` presets download from the network; `--offline` forces cache-only. (h) `--sleep-idle-seconds N` unloads the model after idle and reloads on the next request; `/health` and `/props` do not wake it. (i) `--timeout` is 3600 s. (j) `/slots` is on by default and shows sampling parameters and state.

Source: tools/server/README.md parameter table and CORS section; tools/server/server.cpp (server_prepare_params, server_setup_features); common/common.h

## Recommendations

- Pin to stable v0.9.0 by fetching `nightly-tag.txt` from the stable release (currently b12110) and downloading that nightly's assets. Resolve asset names by pattern (the CUDA minor version and tag change), verify the SHA-256 shown on the release page, and record tag plus hashes in a lockfile. Do not track the newest nightly.
- Windows: bundle the contents of `llama-<tag>-bin-win-vulkan-x64.zip` (about 36 MB; it already includes all CPU variants for fallback). Keep llama-server.exe, llama-server-impl.dll, llama-common.dll, mtmd.dll, llama.dll, ggml.dll, ggml-base.dll, libomp.dll, all ggml-cpu-*.dll and ggml-vulkan.dll in one directory. Drop llama-connect.exe, ggml-rpc*.*, and the other tools. List the extracted zip in CI and assert the expected file set, because I could not extract it in this session.
- Windows: make sure the VC++ 2015-2022 x64 runtime is present (install the redistributable from NSIS or ship the app-local CRT DLLs beside llama-server.exe). Spawn the process with CREATE_NO_WINDOW since the binaries are console-subsystem.
- Treat CUDA as an optional post-install download into the app data directory (about 650 MB for 12.4, about 565 MB for 13.x including cudart), offered only when `--list-devices` or a probe shows an NVIDIA GPU. Keep it out of the NSIS installer and show the NVIDIA EULA if you redistribute cudart.
- macOS: prefer the official single-file binary `b12110/aarch64/macos/metal/m1/llama-app.zst` from the HF bucket as the Tauri externalBin and run it as `llama serve ...`. It has no dylibs, so there is one Mach-O to sign and notarize. If you use the GitHub tarball instead, place every dylib (under its soname, e.g. libllama.0.dylib) in the same directory as llama-server and sign each one. Set the app minimum to macOS 13.3. Confirm on an M1 that the m1 variant runs on all supported chips.
- Always launch with explicit arguments: `--host 127.0.0.1 --port <port chosen by Rust> --no-ui -m <gguf> [--mmproj <gguf>] -c <N> -np 1 --cache-ram <MiB> --offline`, with the API key in env `LLAMA_API_KEY` (random per launch). Leave `--tools`, `--mcp-servers-*`, `--agent` and `--connect` unset and host MCP in rmcp. Use `--device none` to force CPU.
- Readiness: poll `GET /health` until 200, with a process-exit watcher and a timeout. For a real progress bar, either run llama-server as a router (`--models-dir` or a preset, then `/models/load` and `/models/sse`) or set `LLAMA_SERVER_ROUTER_PORT` and parse `cmd_child_to_router:state:` lines on stdout. The second option also makes the server exit when the parent's stdin pipe closes, but it is an internal protocol: isolate it behind an adapter and re-test on every upgrade. Otherwise show an indeterminate spinner.
- At startup run `llama-server --list-devices` and parse stdout to decide GPU or CPU mode and to detect the silent-fallback case. After load, confirm offload from the `offloaded N/M layers to GPU` line and warn the user when a GPU was expected.
- Tool calling: after load read `/props.chat_template_caps` and enable tools only when `supports_tool_calls` is true; otherwise offer a curated `--chat-template-file` from models/templates/. Send `tool_choice` only as auto, none or required, and set `parallel_tool_calls` explicitly. Accumulate streamed tool_calls by index. Handle a mid-stream `{"error":...}` event and a stop finish with empty content but tool-like text in `reasoning_content` as recoverable (retry with thinking disabled). Keep a per-model regression suite.
- Use `stream_options.include_usage`, the final-chunk `timings` and `return_progress:true` for tokens/sec, context-usage and prompt-processing UI. Use `POST /v1/chat/completions/input_tokens` or `/apply-template` plus `/tokenize` for pre-flight context budgeting, and handle 400 `exceed_context_size_error`.
- Vision: gate on `/props.modalities.vision`. Convert every image to PNG or JPEG client-side, downscale, and send only `data:image/...;base64,` URIs. Never forward http(s) image URLs to llama-server, because it would fetch them itself and break the offline guarantee.
- Shutdown: on macOS send SIGTERM and wait. On Windows either use the stdin `cmd_router_to_child:exit` line (managed-child mode) or accept TerminateProcess; place the child in a Job Object with kill-on-close to avoid orphans.
- Notices: ship llama.cpp's MIT LICENSE from the pinned tag, the output of `llama licenses`, `LICENSE-LLVM-OpenMP` (Windows), and the NVIDIA EULA if cudart is redistributed.

## Risks

- The file lists inside the GitHub zips and tarballs were not verified by extraction; this session's proxy returned 403 for GitHub asset and API requests to ggml-org/llama.cpp. The Windows DLL list and macOS dylib list are derived from the release workflow and CMake at b12242. Asset sizes came through a summarising fetch of the release page.
- No llama.cpp binary was run. The silent CPU fallback, exit codes, log strings and the SO_REUSEADDR behaviour on Windows are from source reading and OS semantics, not runtime tests.
- Upstream moves fast: 100+ nightly builds between b12110 and b12242 in six days, and 2026 changed several defaults (port 9931, context and GPU-layer auto-fit, parallel auto, cache-ram, jinja on, UI flag rename, env var renames). Log wording is not stable. Any upgrade can break argument or log assumptions.
- The router-child stdout protocol (`LLAMA_SERVER_ROUTER_PORT`, `cmd_child_to_router:state:`) is internal and documented only in README-dev.md; it can change without notice.
- Tool calling with thinking-capable small models is the weakest area: issue #20837 (tool call emitted inside the thinking block) is still open, parser mismatches surface as HTTP 500 or mid-stream errors, and templates without tool support now get no tool calls because the Generic fallback was removed. docs/function-calling.md is partly stale.
- Upstream binaries are unsigned (no Authenticode; ad-hoc signature only on macOS). Expect SmartScreen or antivirus friction on Windows unless you sign the files, and notarization failures on macOS if any nested Mach-O is left unsigned or the dylib symlink layout is broken by the bundler.
- The official macOS builds require macOS 13.3, not 13.0. The GitHub macOS tarball is built without GGML_NATIVE=OFF, so its CPU code path depends on the CI runner's chip.
- Memory defaults can surprise on 8-16 GB machines: an 8 GiB prompt cache, 4 slots, 32 checkpoints per slot, and a load-time context that depends on free VRAM. A model may load with a different n_ctx on each launch unless `-c` is pinned.
- On Windows a second llama-server may bind an already used port without an error because SO_REUSEADDR is set, so a stale sidecar could shadow or split traffic. The per-launch API key and a single-instance guard mitigate this.
- The CUDA variant is large (roughly 565-650 MB) and the cudart DLLs are under the NVIDIA EULA. CUDA 12.4 and 13.x builds need matching driver generations; an old driver leads to silent CPU fallback in the zip layout.
- The HF install bucket's `latest` (b12182) differs from GitHub stable (b12110) and its layout is undocumented outside install.sh and install.ps1. Windows single-file builds are CPU-feature-specific and need `featcode.exe` to select, so they are a poor fit for an offline installer.
