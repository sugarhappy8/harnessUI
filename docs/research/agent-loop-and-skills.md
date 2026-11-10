# agent-loop-and-skills — research

## Summary

Verified on 2026-11-10 against source checkouts (llama.cpp master 3b4d6ab / tag b12242, Ollama v0.40.16, vLLM v0.33.0, agentskills 83021e2 with skills-ref 0.1.0, Codex CLI 0811df3, opencode 9e2f931, Goose 72b8db8, Cline e1f499a) plus official docs. Clones are under <temp workspace> nothing was written to the repository.

Main conclusions:
1. The stream reader must be tolerant, not spec-strict. llama.cpp streams OpenAI-style argument fragments with a 32-char id. Ollama sends each tool call whole in one chunk with id `call_xxxxxxxx`. Reasoning arrives as `delta.reasoning_content` (llama.cpp, LM Studio) or `delta.reasoning` (Ollama, vLLM). llama.cpp also sends SSE comment pings and can end a stream with `data: {"error":...}` and no `[DONE]`.
2. llama.cpp now parses tool calls with a template-derived PEG auto-parser. docs/function-calling.md is partly stale: it shows `finish_reason: "tool"` and says parallel calls are off by default, while the code emits `tool_calls` and defaults `parallel_tool_calls` to the template capability. Always send `parallel_tool_calls` explicitly.
3. Ollama's `/v1` cannot set `num_ctx`, `tool_choice` or `truncate`. Its default context is VRAM-based (4k below 24 GiB, 32k for 24-48 GiB, 256k at 48 GiB or more), and it silently drops old messages on overflow.
4. llama-server has an exact counter, `POST /v1/chat/completions/input_tokens`, alongside `/apply-template`, `/tokenize`, `usage` and `timings`. Overflow returns HTTP 400 `exceed_context_size_error` carrying `n_prompt_tokens` and `n_ctx`.
5. Mature agents compact at 80-90% of the window: Codex 90%, opencode about 85% (or 16k reserve), Goose 80%, Cline about 81%. They keep roughly 20k tokens of recent content plus a structured summary.
6. The Agent Skills spec has exactly six frontmatter fields. skills-ref is stricter than the spec in places (rejects unknown fields, uses strictyaml) and looser in others (accepts Unicode letters in `name`). The official client guide recommends lenient loading with warnings.

Not verified: Jan was not researched; Open WebUI rests on a search summary only; LM Studio's way of setting context length, vLLM's `/tokenize`, and the Gemma 2/3 `tool_code` convention could not be confirmed today.

## Findings

### llama-server streaming /v1/chat/completions wire shape (current master): OpenAI-style incremental tool-call deltas, finish_reason "tool_calls", delta.reasoning_content.

Confidence: high

First chunk delta is {"role":"assistant","content":null}. Later deltas carry any of `reasoning_content`, `content`, or `tool_calls:[{index, id?, type?, function:{name?, arguments?}}]`, with one tool_call entry per chunk. `id` and `type:"function"` appear only in the header chunk for that index, together with `function.name`; following chunks carry only `index` and `function.arguments` fragments. The name is held back until arguments start or the stream ends. Tool-call id is 32 random alphanumeric chars with no `call_` prefix; completion id is `chatcmpl-` plus 32 chars. Final chunk has `delta:{}` and `finish_reason` of `stop`, `tool_calls` or `length`. `length` can cut a tool call mid-arguments, so the accumulated JSON must be validated. Every chunk has `object:"chat.completion.chunk"` and `system_fingerprint`; the stream ends with `data: [DONE]`.

Source: github.com/ggml-org/llama.cpp @3b4d6ab (tag b12242): tools/server/server-chat.cpp server_chat_msg_diff_to_json_oaicompat (L681-710); tools/server/server-task.cpp update_chat_msg (L159-228), to_json_oaicompat_chat_stream (L452-515), to_json_oaicompat_chat (L1113-1165); server-common.cpp gen_tool_call_id (L189)

### llama-server adds a usage chunk, a `timings` object, SSE comment pings, and can end a stream with an error object instead of [DONE].

Confidence: high

With `stream_options:{include_usage:true}` an extra chunk has `choices:[]` and `usage:{completion_tokens,prompt_tokens,total_tokens,prompt_tokens_details:{cached_tokens}}`. `timings` is attached to the last chunk (the usage chunk if present, otherwise the finish chunk), or to every chunk with `timings_per_token:true`. Fields: cache_n, prompt_n, prompt_ms, prompt_per_token_ms, prompt_per_second, predicted_n, predicted_ms, predicted_per_token_ms, predicted_per_second. The README states context in use = prompt_n + cache_n + predicted_n. `return_progress:true` adds `prompt_progress:{total,cache,processed,time_ms}`. After `--sse-ping-interval` seconds of silence (default 30; per-request `sse_ping_interval`, -1 disables) the server sends the comment line `:` which the parser must ignore. On a mid-stream failure it sends `data: {"error":{code,message,type}}` and closes with no `[DONE]`.

Source: llama.cpp @3b4d6ab: tools/server/server-task.cpp L494-510, L352-366; tools/server/server-context.cpp L5495-5605; tools/server/server-schema.cpp L22-45; tools/server/README.md sections 'Timings and context usage', `return_progress`, `sse_ping_interval`

### llama.cpp tool-call parsing is now a template-derived PEG auto-parser; docs/function-calling.md is partly stale, and an unparseable final output becomes a server error.

Confidence: high

docs/autoparser.md describes differential template analysis into three tool formats: JSON_NATIVE, TAG_WITH_JSON and TAG_WITH_TAGGED. Only a few templates keep dedicated handlers (Ministral/Magistral Large 3, GPT-OSS, Functionary v3.2, plus Gemma4 and MiniMax M3 mappers). When the final parse fails, common_chat_peg_parse throws `The model produced output that does not match the expected <format> format`, which reaches the client as an error rather than as plain content. docs/function-calling.md still shows a sample with `finish_reason:"tool"` and tool_calls without ids; the code emits `tool_calls` with ids. The same doc warns that extreme KV quantization such as `-ctk q4_0` substantially degrades tool calling.

Source: llama.cpp @3b4d6ab: docs/autoparser.md; common/chat.cpp L1786-1845; common/chat.h L242-248; docs/function-calling.md

### llama-server's `parallel_tool_calls` default is the template capability, not `false` as the docs say.

Confidence: high

Code: `inputs.parallel_tool_calls = json_value(body, "parallel_tool_calls", caps.at("supports_parallel_tool_calls"))`. docs/function-calling.md says parallel calling is 'disabled by default'. A client that wants one call per turn must send `parallel_tool_calls:false` on every request. Input validation: each assistant tool_call in history needs `type`, `function.name` and `function.arguments` (string or object both accepted), and a message must have `content` or `tool_calls`. `tool_choice` defaults to `auto`.

Source: llama.cpp @3b4d6ab: tools/server/server-common.cpp L1375-1382, L1466; common/chat.cpp L400-470; docs/function-calling.md line 25

### Ollama /v1/chat/completions (v0.40.16): tool calls arrive whole, reasoning is in `reasoning`, and there is no tool_choice, parallel_tool_calls or options passthrough.

Confidence: high

The request struct accepts only model, messages, stream, stream_options, max_tokens, seed, stop, temperature, frequency_penalty, presence_penalty, top_p, response_format, tools, reasoning / reasoning_effort, logprobs, top_logprobs, and the Ollama extension `keep_alive`. The docs list `tool_choice` as unsupported. Each streamed tool call is a complete object {id, index, type:"function", function:{name, arguments:<full JSON string>}} in one delta; do not assume fragments. Ids are `call_` plus 8 chars of [a-z0-9]. Reasoning is `delta.reasoning` / `message.reasoning`. The finish chunk has an empty delta, and `tool_calls` replaces `stop` only if a tool call was sent. `system_fingerprint` is `fp_ollama`. `stream_options.include_usage` is supported, and usage may include `prompt_tokens_details.cached_tokens`.

Source: github.com/ollama/ollama @f1ee567 (latest tag v0.40.16): openai/openai.go L32-50, L111-132, L209-217, L303-321, L355-448; server/routes.go L2872-2879; https://docs.ollama.com/api/openai-compatibility.md

### Ollama default context is VRAM-based (4k / 32k / 256k), cannot be raised through the OpenAI-compatible API, and overflow is silently truncated.

Confidence: high

docs/context-length: below 24 GiB VRAM 4k, 24-48 GiB 32k, 48 GiB or more 256k; 'agents and coding tools should be set to at least 64000 tokens'. The FAQ still says the default is 4096. Ways to raise it: `OLLAMA_CONTEXT_LENGTH=64000 ollama serve`, the app settings slider, a Modelfile `PARAMETER num_ctx N` followed by `ollama create`, or native `/api/chat` with `options.num_ctx`. The OpenAI-compat doc says 'The OpenAI API does not have a way of setting the context size'. The native API's `truncate` defaults to true: chatPrompt drops oldest messages and keeps system messages plus the latest. `/v1` has no `truncate` field, so an OpenAI-compatible client never gets an overflow error.

Source: https://docs.ollama.com/context-length.md; https://docs.ollama.com/faq.md; https://docs.ollama.com/api/openai-compatibility.md ('Setting the local context size'); ollama @f1ee567 envconfig/config.go L229-230, L338; api/types.go L114-120; server/prompt.go L20-85; server/routes.go L3290

### LM Studio: OpenAI-style fragment streaming, a prompt-based default tool format for models without native support, and context info through its REST API.

Confidence: medium

Docs show streamed tool calls with the first delta carrying index, id (example '814890118'), name and empty arguments, then argument fragments; `finish_reason` is `tool_calls`. Models without native support use the prompt format `[TOOL_REQUEST]{"name":...,"arguments":{...}}[END_TOOL_REQUEST]`, which LM Studio parses back into tool_calls. `reasoning_content` as a separate field is opt-in (App Settings > Developer, since 0.3.9). Documented /v1/chat/completions parameters: model, top_p, top_k, messages, temperature, max_tokens, stream, stop, presence_penalty, frequency_penalty, logit_bias, repeat_penalty, seed. Context length: `GET /api/v0/models` gives `max_context_length` and `state`; newer `GET /api/v1/models` gives `models[].max_context_length`, `loaded_instances[].config.context_length` and `capabilities.trained_for_tool_use`. How to set context length per request or at load was not verified.

Source: https://lmstudio.ai/llms.txt (Tool Use and REST API v0 sections); https://lmstudio.ai/docs/developer/rest/list; https://lmstudio.ai/docs/developer/openai-compat/chat-completions

### vLLM (v0.33.0): tool calling needs server flags, reasoning is in `reasoning`, and `/v1/models` exposes `max_model_len`.

Confidence: medium

Auto tool choice requires `--enable-auto-tool-choice --tool-call-parser <name>`, optionally `--chat-template`. Without constrained decoding, 'arguments may occasionally be malformed or violate the function's parameter schema'; `--tool-strict-level function|parameter` or per-tool `strict:true` enables structural-tag constraints. `tool_choice:"required"` is supported from 0.8.3. The message field is `reasoning`; `reasoning_content` is deprecated and renamed on input. `GET /v1/models` returns `data[].max_model_len`. Mistral templates on the Transformers backend require 9-digit tool call ids. The response-template (`hf`) parser emits each streamed tool call whole and drops calls that fail to parse. The vLLM `/tokenize` endpoint was not verified (the docs page redirected).

Source: github.com/vllm-project/vllm @dc74b8c (latest tag v0.33.0): docs/features/tool_calling.md; vllm/entrypoints/openai/models/serving.py L64-75; vllm/entrypoints/openai/chat_completion/protocol.py L88, L586-602

### Fallback catalogue A - JSON object inside a tag or after a marker.

Confidence: high

(1) Hermes 2/3, Qwen2.5, Qwen3, QwQ, Granite 4.0/4.1: `<tool_call>\n{"name":..., "arguments":{...}}\n</tool_call>`, repeated for parallel calls. (2) Mistral Nemo / 7B v0.3: `[TOOL_CALLS][{"name":...,"arguments":{...}}, ...]`. (3) Granite 3.x: `<|tool_call|>` followed by a JSON list. (4) Llama 3.1/3.2/3.3 custom tools: bare `{"name": "fn", "parameters": {...}}`; note the key is `parameters`, not `arguments`. (5) LM Studio default: `[TOOL_REQUEST]{...}[END_TOOL_REQUEST]`. (6) Phi-4-mini: `functools[{...}]`. (7) Apertus style: `{"fn_name": {args}}` with the name as key. (8) llama.cpp JSON_NATIVE covers `{"name":X,"arguments":{...}}` with optional enclosing tags.

Source: llama.cpp @3b4d6ab models/templates (Qwen-Qwen2.5-7B-Instruct, ibm-granite-granite-4.1, ibm-granite-granite-3.3-2B-Instruct, mistralai-Mistral-Nemo-Instruct-2407, meta-llama-Llama-3.1-8B-Instruct L74-90) and docs/autoparser.md; vllm @dc74b8c vllm/tool_parsers/hermes_tool_parser.py, granite4_tool_parser.py, phi4mini_tool_parser.py; https://lmstudio.ai/llms.txt

### Fallback catalogue B - function name outside the JSON, XML-style parameters, or non-JSON call syntax.

Confidence: medium

(1) Newer Mistral (Ministral 3, Devstral 2507): `[TOOL_CALLS]name[ARGS]{json}`. (2) Functionary v3.1: `<function=name>{json}</function>`; v3.2: `>>>name\n{json}`. (3) Qwen3-Coder, Qwen3.5, Nemotron-3, StepFun, Granite 4.2: `<tool_call>\n<function=name>\n<parameter=key>\nvalue\n</parameter>\n</function>\n</tool_call>`, with string values unquoted. (4) Llama 3.x builtin tools: `<|python_tag|>name.call(arg="val")`. (5) Kimi K2: `<|tool_calls_section_begin|><|tool_call_begin|>functions.NAME:IDX<|tool_call_argument_begin|>{json}<|tool_call_end|>`. (6) DeepSeek V3: `<｜tool▁calls▁begin｜><｜tool▁call▁begin｜>function<｜tool▁sep｜>NAME\n```json\n{...}\n```<｜tool▁call▁end｜><｜tool▁calls▁end｜>`; V3.1 drops the type and fence: `<｜tool▁call▁begin｜>NAME<｜tool▁sep｜>{json}<｜tool▁call▁end｜>`. (7) Gemma 4: `<|tool_call>call:name{key:<|"|>value<|"|>}<tool_call|>`, sometimes closed by `<turn|>`, with a bare `call:name{...}` fallback. (8) FunctionGemma: `<start_function_call>call:name{param:<escape>value<escape>}<end_function_call>`. (9) Pythonic lists (Llama 3.2 1B/3B, Llama 4, LFM2 inside `<|tool_call_start|>[fn(a="x")]<|tool_call_end|>`). (10) MiniMax: `<minimax:tool_call>` with `<tool_name>` and `<arguments>` children. (11) GPT-OSS Harmony uses `<|channel|>` routing; the exact call string is from memory. Gemma 2/3 have no tool tokens; the ```tool_code convention is from memory and was not confirmed today. DeepSeek R1 distills are described by llama.cpp as 'reluctant to call any tools'.

Source: llama.cpp @3b4d6ab models/templates (mistralai-Ministral-3-14B-Reasoning-2512 L102-114, Qwen3-Coder L60-66, moonshotai-Kimi-K2 L17-24, deepseek-ai-DeepSeek-V3.1 L38, meetkai-functionary-medium-v3.1/v3.2, meta-llama-Llama-3.1-8B-Instruct L76) and docs/autoparser.md; vllm @dc74b8c vllm/tool_parsers/deepseekv3_tool_parser.py L43-59, deepseekv31_tool_parser.py L40-56, gemma4_utils.py L74-121, functiongemma_tool_parser.py L136-146, lfm2_tool_parser.py L40-49, docs/features/tool_calling.md; https://ai.google.dev/gemma/docs/capabilities/function-calling

### Bare JSON, wrapped JSON, and double-encoded arguments are handled by Goose's local-model fallback parser, a usable reference design.

Confidence: high

Goose collects JSON candidates from the whole trimmed text plus every balanced `{...}` or `[...]` span, found by a string- and escape-aware bracket matcher. It accepts: an OpenAI message object with `tool_calls`; a top-level `{"tool_calls":[...]}`; a top-level array of calls; a single call object. A value counts as a call only if it has (`name` string AND `arguments`) or `function.name`. Arguments given as a string are parsed again as JSON (the double-encoded case); empty string or null becomes `{}`. If the text contains `<function=` it parses `<function=NAME>...<parameter=KEY>VALUE</parameter>...</function>`. Missing ids get a UUID. Whole-text JSON that is not a call returns None and stays content. An unparseable argument string becomes a tool error: `Could not interpret tool use parameters for id {id}: {error}. Raw arguments: '{raw}'`.

Source: github.com/block/goose @72b8db8: crates/goose-local-inference/src/native_tool_parsing.rs L10-250

### False-positive controls used in production parsers: position, an exact tool-name match, and a strict object shape.

Confidence: high

Ollama's stream parser derives the tool-call tag from the chat template. When that tag is just `{` or `[`, it parses tool calls only if the first non-whitespace character of the output is `{` or `[`. It then requires a name that matches one of the request's tools (waiting on partial names so `get` does not match `get_weather`), and reads arguments from an `arguments` or `parameters` key. Unmatched `{}` / `[]` content is returned to the user as text. Known gap: a call with no arguments object, `{"name":"fn"}`, is not recognised. Goose additionally requires both name and arguments, and validates names against [a-zA-Z0-9_-]+.

Source: ollama @f1ee567 tools/tools.go L52-110, L162-235, L297-300, L345-348; goose @72b8db8 crates/goose-local-inference/src/native_tool_parsing.rs L78-90, L172-185

### Invalid arguments: mature agents repair what the schema makes unambiguous, then return the error to the model as the tool result, with a consecutive-mistake cap.

Confidence: high

opencode's tool-input-repair plugin, guided by the tool's JSON Schema (max depth 6): parses a stringified root or nested object, drops extra keys on closed objects, removes null or empty-object placeholders for optional fields, coerces numeric and boolean strings, parses stringified arrays, and wraps a scalar into a single-item array. It aliases the unknown tool name `bash` to `shell` and appends 'There is no "bash" tool, so this ran as "shell". Call "shell" directly.' Cline SDK: `maxConsecutiveMistakes` default 6, where mistakes are API turn failures, invalid or missing tool-call arguments, and iterations where every tool call fails. MCP: tool execution errors go back as a result with `isError:true`; unknown tool and invalid arguments are JSON-RPC errors (-32602).

Source: github.com/sst/opencode @9e2f931 packages/core/src/plugin/tool-input-repair.ts L8-75; github.com/cline/cline @e1f499a sdk/packages/shared/src/agents/types.ts L222-282; https://modelcontextprotocol.io/specification/2025-06-18/server/tools (Error Handling)

### Step limits and loop detection, concrete numbers.

Confidence: high

Goose: `GOOSE_MAX_TURNS` default 1000 turns without user input; gateway sessions default 5; an optional tool-repetition inspector (`max_repetitions`). Cline CLI: loop detection on consecutive identical tool calls (same name plus key-sorted JSON arguments), soft threshold 3 (inject a recovery notice) and hard threshold 5 (enter the mistake-limit path); `reminderAfterIterations` default 0. opencode: per-agent `steps` is optional with no default cap; on reaching it, tools are disabled and a 'CRITICAL - MAXIMUM STEPS REACHED' prompt forces a text-only summary of work done, remaining tasks and next steps. A `doom_loop` permission key exists (ask / allow / deny). No Codex CLI step cap was verified.

Source: goose @72b8db8 documentation/docs/guides/environment-variables.md L137-140, crates/goose/src/gateway/handler.rs L25-32, crates/goose/src/tool_monitor.rs; cline @e1f499a sdk/packages/core/src/runtime/safety/loop-detection.ts, sdk/packages/shared/src/agents/types.ts L247-280; opencode @9e2f931 packages/core/src/session/runner/max-steps.ts, runner/llm.ts L256, v1/config/agent.ts L34-37, v1/config/permission.ts L27

### Tool-output truncation, concrete numbers.

Confidence: high

Codex CLI: middle truncation keeping head and tail (budget split in half), marker `…N tokens truncated…` or `…N chars truncated…`, preceded by `Warning: truncated output (original token count: N)\nTotal output lines: N`. Default exec budget `DEFAULT_MAX_OUTPUT_TOKENS = 10_000`; fallback model policy is 10_000 bytes; raw capture cap 1 MiB. opencode: `MAX_LINES = 2_000`, `MAX_BYTES = 50 KiB`, head only, full text saved to a `tool-output/tool_<id>` file kept 7 days, marker `[showing lines 1-N of M; full output saved to <file>]`. Its read tool uses 2_000 lines, 50 KiB, and 2_000 chars per line. Goose: text results over 200_000 chars are written to a file instead of going inline. Cline, when building the summarization transcript: tool results and file contents are clipped to 2_000 chars.

Source: github.com/openai/codex @0811df3 codex-rs/utils/output-truncation/src/lib.rs L25-50, codex-rs/utils/string/src/truncate.rs L38-53, L149-152, codex-rs/core/src/unified_exec/mod.rs L74-76, codex-rs/protocol/src/openai_models.rs L1228; opencode @9e2f931 packages/core/src/tool-output.ts L13-16, L69-120, tool/read-filesystem.ts L14-19; goose @72b8db8 crates/goose/src/agents/large_response_handler.rs L5; cline @e1f499a sdk/packages/core/src/extensions/context/compaction-shared.ts L35-36

### Auto-compaction thresholds, concrete numbers.

Confidence: high

Codex CLI: `auto_compact_token_limit` = 90% of the context window, and a configured value is clamped to at most 90%; `effective_context_window_percent` = 95. opencode: ceiling = window - max(floor(0.15 x window), 16_000 if window >= 32_000 else 0), so about 85% for large windows and window minus 16k for 32k-106k windows. A user `buffer` (tokens or percent) overrides this. An unknown window is assumed to be 200_000. After a provider 'too long' rejection it retries at 0.85, 0.7, 0.5, 0.35 of the rejected size. Goose: `GOOSE_AUTO_COMPACT_THRESHOLD` default 0.8 (0.0 disables). Cline SDK: usable input = 0.9 x context window, trigger at 0.9 of that (about 81% of the window), compaction target ratio 0.7, default max input 128_000 tokens. opencode sizes from the provider-measured usage of the last response plus an estimate for messages not yet measured.

Source: codex @0811df3 codex-rs/protocol/src/openai_models.rs L413-415, L483-493, L567-590; opencode @9e2f931 packages/core/src/session/compaction.ts L127-171, L1150-1178; goose @72b8db8 crates/goose-context-management/src/lib.rs L32, documentation/docs/guides/environment-variables.md L155; cline @e1f499a sdk/packages/core/src/extensions/context/compaction-shared.ts L15-37

### What is kept after compaction, and the prompts used.

Confidence: high

Codex: new history = initial context + most recent user messages up to `COMPACT_USER_MESSAGE_MAX_TOKENS = 20_000` (newest first) + the summary. Prompt: 'You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task.' It asks for progress and decisions, constraints and preferences, remaining steps, and critical data. The summary is prefixed with 'Another language model started to solve this problem and produced a summary of its thinking process...'. Cline: preserves 20_000 recent tokens; summary output capped at 8_192 tokens, raised from 4_096 because reasoning models spent the budget on thinking and returned no summary. opencode: fixed Markdown template (Objective, Requirements, Decisions, Work State with Completed / Active / Blocked, Next Move, Relevant Files up to 15, Important Context), rules to preserve exact paths, commands, error strings and identifiers, and 'Do not continue the task or call tools.' Later compactions update the existing checkpoint. Goose: if history does not fit the summarizer, it drops tool responses from the middle outward at 0, 10, 20, 50, 100%. The Agent Skills guide says activated skill content must be exempt from pruning.

Source: codex @0811df3 codex-rs/core/src/compact.rs L64, L716-760, codex-rs/prompts/templates/compact/prompt.md and summary_prefix.md; cline @e1f499a compaction-shared.ts L28-34; opencode @9e2f931 packages/core/src/session/compaction.ts L173-246; goose @72b8db8 crates/goose-context-management/src/summarize.rs L14-60; https://agentskills.io/client-implementation/adding-skills-support.md (Step 5)

### Exact token counting on llama-server: a dedicated input_tokens endpoint, /apply-template plus /tokenize, and a structured overflow error.

Confidence: high

`POST /v1/chat/completions/input_tokens` takes a chat-completion body and returns `{"object":"response.input_tokens","input_tokens":N}`; it is documented as non-OpenAI, 'added for completeness and convenience'. Also available: `/v1/responses/input_tokens` and Anthropic-style `/v1/messages/count_tokens`. `POST /apply-template` runs the same request parser as chat completions without inference and returns `{"prompt":"..."}`. `POST /tokenize` takes `{content, add_special=false, parse_special=true, with_pieces=false}` and returns `{"tokens":[...]}`. Context size: `GET /props` -> `default_generation_settings.n_ctx`; `GET /v1/models` -> `data[0].meta.n_ctx_train`; `GET /slots` -> `n_ctx`. Overflow is HTTP 400 with `type:"exceed_context_size_error"`, message 'request (N tokens) exceeds the available context size (M tokens), try increasing it', and extra fields `n_prompt_tokens` and `n_ctx`. Context shift is disabled by default, so there is no silent truncation.

Source: llama.cpp @3b4d6ab: tools/server/server.cpp L202-210; tools/server/README.md sections `/tokenize`, `/apply-template`, `/v1/chat/completions/input_tokens`, `/props`, `/v1/models`; tools/server/server-context.cpp L6431-6497, L4090-4105; tools/server/server-task.cpp L1546-1553; server-common.cpp L126-129

### Finding the context length of a remote endpoint.

Confidence: high

Ollama: `POST /api/show {"model":...}` returns `model_info["<arch>.context_length"]` (the trained maximum, e.g. `gemma4.context_length: 131072`), a `parameters` text block that may contain `num_ctx N`, and `capabilities` (completion, tools, thinking, vision). The context actually allocated is `GET /api/ps` -> `models[].context_length`, or the CONTEXT column of `ollama ps`. The trained maximum is not the usable window. LM Studio: `/api/v0/models[/{model}]` -> `max_context_length`; `/api/v1/models` -> `max_context_length` plus `loaded_instances[].config.context_length` (the docs example shows 4096 loaded against 262144 maximum). vLLM: `/v1/models` -> `data[].max_model_len`.

Source: https://docs.ollama.com/api-reference/show-model-details.md; ollama @f1ee567 api/types.go L945-955; https://docs.ollama.com/context-length.md; https://lmstudio.ai/llms.txt (REST API v0); https://lmstudio.ai/docs/developer/rest/list; vllm @dc74b8c vllm/entrypoints/openai/models/serving.py L64-75

### Cheap token estimators in production agents differ: Codex uses bytes/4, Cline uses chars/3.

Confidence: high

Codex: `APPROX_BYTES_PER_TOKEN = 4`, count = ceil(bytes / 4). Cline: `CHARS_PER_TOKEN = 3`, max(1, ceil(chars / 3)); its comment says dense content tokenizes 2-3 times denser than the 3-chars assumption, and `MAX_INPUT_UNDERESTIMATE_FACTOR = 4` caps how far real provider counts may scale the budget down. opencode estimates images at 1_500 tokens and PDFs at 2_000. Code, JSON and non-English text run well below 4 chars per token, so chars/4 undercounts tool-heavy transcripts.

Source: codex @0811df3 codex-rs/utils/string/src/truncate.rs L6, L94-107; cline @e1f499a sdk/packages/shared/src/llms/tokens.ts L8-11, sdk/packages/core/src/extensions/context/compaction-shared.ts L20-27; opencode @9e2f931 packages/core/src/session/compaction.ts L151-152

### Spotlighting (Microsoft, arXiv:2403.14720) is the main published prompt-level defence; its strongest variant is unsuitable for small models.

Confidence: high

Three techniques. Delimiting: wrap untrusted text in markers and tell the model not to obey anything inside; the authors call it the weakest because an attacker who knows the delimiters can include them. Datamarking: interleave a marker character between words (they suggest a private-use code point such as U+E000) and tell the model the document is marked that way. Encoding: base64 the document. Reported attack success rate on GPT-family models fell from above 50% to below 2%. The authors recommend encoding only for high-capacity models because it hurts task performance on weaker ones.

Source: https://arxiv.org/abs/2403.14720 and https://arxiv.org/html/2403.14720 (Hines et al., 2024)

### Authoritative guidance places the real controls outside the model: approval, argument validation and permissions enforced in code.

Confidence: high

OWASP cheat sheet: 'do not treat text labels or prompt wording as an enforcement boundary'; 'Validate proposed tool arguments and enforce the caller's permissions in execution code outside the model'; treat tool results, retrieved documents and conversation history as untrusted; a prompt asking for review does not stop execution, so sensitive tools must be configured to pause. MCP spec (2025-06-18): there 'SHOULD always be a human in the loop with the ability to deny tool invocations'; clients 'MUST consider tool annotations to be untrusted unless they come from trusted servers'; clients SHOULD show tool inputs before calling, validate results before passing them to the LLM, set timeouts, and log tool usage. Willison's lethal trifecta: private data + untrusted content + ability to communicate externally; 'avoid that lethal trifecta combination entirely'. Agent Skills guide: gate project-level skills on a folder trust check so a cloned repo cannot inject instructions.

Source: https://cheatsheetseries.owasp.org/cheatsheets/LLM_Prompt_Injection_Prevention_Cheat_Sheet.html; https://modelcontextprotocol.io/specification/2025-06-18/server/tools; https://simonwillison.net/2025/Jun/16/the-lethal-trifecta/; https://agentskills.io/client-implementation/adding-skills-support.md (Trust considerations)

### Agent Skills SKILL.md frontmatter: exactly six fields, with these constraints.

Confidence: high

`name` (required): 1-64 chars, lowercase alphanumerics and hyphens, no leading or trailing hyphen, no `--`, and must match the parent directory name. `description` (required): 1-1024 chars, non-empty, should say what the skill does and when to use it. `license` (optional): a licence name or a reference to a bundled file. `compatibility` (optional): 1-500 chars. `metadata` (optional): map of string keys to string values. `allowed-tools` (optional, experimental): a space-separated string such as `Bash(git:*) Bash(jq:*) Read`. The body is free Markdown. The file must be named SKILL.md and start with YAML frontmatter.

Source: https://agentskills.io/specification.md; github.com/agentskills/agentskills @83021e2 docs/specification.mdx

### The skills-ref validator (v0.1.0) differs from the spec text in ways that matter for a Rust reimplementation.

Confidence: high

Checks: only these top-level keys are allowed: name, description, license, allowed-tools, metadata, compatibility. Any other key gives 'Unexpected fields in frontmatter', so Claude-Code-style extras such as `disable-model-invocation` fail. name: strip, NFKC-normalise, length <= 64, must equal its lowercase form, no leading or trailing `-`, no `--`, every char must satisfy `isalnum()` or be `-`. That accepts Unicode letters, although the spec says a-z and 0-9. The NFKC-normalised directory name must equal the name. description: non-empty string, length <= 1024 in code points. compatibility: string, <= 500. license, allowed-tools and metadata are not type-checked; metadata keys and values are coerced to strings. Parsing: content must begin with `---`, split with `content.split('---', 2)`, YAML loaded with strictyaml (every scalar is a string; flow-style `{}` / `[]` rejected). It prefers SKILL.md and accepts lowercase skill.md. Body length is not validated. CLI: `skills-ref validate <dir>`, `read-properties <dir>`, `to-prompt <dir>...`. The README says it is 'intended for demonstration purposes only'.

Source: agentskills @83021e2 skills-ref/src/skills_ref/validator.py, parser.py, prompt.py, pyproject.toml, README.md

### Skills directory conventions and progressive-disclosure numbers.

Confidence: high

Layout: `skill-name/SKILL.md` (required), `scripts/` (executable code), `references/` (docs loaded on demand, e.g. REFERENCE.md, FORMS.md), `assets/` (templates, images, data), plus any other files. Three tiers: metadata of about 100 tokens (name and description) loaded at startup for every skill; instructions under 5000 tokens recommended, loaded on activation; resources loaded only when needed. Keep SKILL.md under 500 lines. File references are relative to the skill root and one level deep. Script commands run from the skill directory root; self-contained scripts with inline dependencies (PEP 723 with `uv run`) and version pinning are recommended.

Source: https://agentskills.io/specification.md (Directory structure, Optional directories, Progressive disclosure, File references); https://agentskills.io/skill-creation/using-scripts.md

### Client implementation guidance: where to scan, how to advertise skills, and how to activate them.

Confidence: high

Scan `<project>/.<client>/skills/`, `<project>/.agents/skills/`, `~/.<client>/skills/`, `~/.agents/skills/`; some clients also scan `.claude/skills/`. Bounds: max depth 4-6, max 2000 directories, skip .git and node_modules. Project skills override user skills on a name collision; log a warning. Lenient loading: name mismatch or over 64 chars -> warn and load; missing description or unparseable YAML -> skip. Retry YAML after quoting values that contain unquoted colons. Catalogue format: `<available_skills><skill><name>..</name><description>..</description><location>/abs/path/SKILL.md</location></skill></available_skills>`, about 50-100 tokens per skill; skills-ref `to_prompt` emits the same shape with one tag per line and html-escaped name and description. Omit the block entirely when there are no skills. Suggested instruction for a dedicated tool: 'When a task matches a skill's description, call the activate_skill tool with the skill's name to load its full instructions.' Constrain the tool's name parameter to an enum of valid skills. Wrap the result as `<skill_content name="...">body + 'Skill directory: <abs>' + <skill_resources><file>..</file></skill_resources></skill_content>`. Deduplicate activations, allowlist skill directories for file reads, and support user activation by `/skill-name`.

Source: https://agentskills.io/client-implementation/adding-skills-support.md (Steps 1-5); agentskills @83021e2 skills-ref/src/skills_ref/prompt.py

### llama-server defaults relevant to tool use (current master).

Confidence: high

Sampling: temperature 0.80, top_k 40, top_p 0.95, min_p 0.05, repeat_penalty 1.00. GGUF metadata keys `general.sampling.*` (temp, top_k, top_p, min_p, penalty_repeat, sequence, mirostat) override these defaults unless the user set the parameter explicitly. Server: `--jinja` enabled by default; `--cache-prompt` enabled and request `cache_prompt` defaults to true; `--cache-reuse` 0; `--cache-ram` 8192 MiB; `--ctx-checkpoints` 32; `--context-shift` disabled; `-c` 0 means load from the model; `-np` -1 means auto, which resolves to 4 slots with a unified KV buffer; `--reasoning-format` auto, with `none` leaving thoughts in content; `--timeout` 3600 s. Request-side controls: `chat_template_kwargs` (e.g. `{"enable_thinking": false}`), `reasoning_effort`, `reasoning_format`, `--reasoning-budget N`, `id_slot`. The default port is now 9931, not 8080. Experimental `--tools` and `--mcp-servers-config` flags exist and are off by default.

Source: llama.cpp @3b4d6ab: tools/server/README.md (options table L50-250; /completion options; chat completions options); common/common.h L645-649; common/common.cpp L1095-1135; src/llama-arch.cpp L191-202; tools/server/server.cpp L126-129

### Model-card sampling recommendations vary too much for one global default.

Confidence: high

Qwen3: thinking mode Temperature 0.6, TopP 0.95, TopK 20, MinP 0; non-thinking 0.7 / 0.8 / 20 / 0. 'DO NOT use greedy decoding, as it can lead to performance degradation and endless repetitions'. presence_penalty 0-2 against repetition. History should contain only the final output, not thinking content. Mistral Small 3.2: 'We recommend using a relatively low temperature, such as temperature=0.15'. Other families were not re-verified today.

Source: https://huggingface.co/Qwen/Qwen3-8B (Best Practices); https://huggingface.co/mistralai/Mistral-Small-3.2-24B-Instruct-2506

### Current crate versions for the pieces this design needs (crates.io, 2026-11-10).

Confidence: medium

jsonschema 0.61.0 (tool-argument validation); eventsource-stream 0.2.3 and reqwest-eventsource 0.6.0 (SSE); llm_json 1.0.3 and jsonrepair 0.1.0 (malformed JSON repair); partial-json-fixer 0.5.5; serde_yaml is 0.9.34+deprecated, with alternatives serde_yaml_ng 0.10.0, serde_norway 0.9.42, serde-saphyr 2.1.1; gray_matter 0.3.2 (frontmatter); strict-yaml-rust 0.2.2; async-openai 0.43.0; tiktoken-rs 0.12.1; rmcp 3.9.0. Versions come from `cargo search` only; APIs were not reviewed.

Source: `cargo search <crate> --limit 1` run 2026-11-10

### Open WebUI now treats native function calling as the only supported mode.

Confidence: low

The prompt-injection mode, formerly 'Default', is now 'Legacy' and unsupported. It allows only one tool-calling round, whereas Native loops until the model is done. The docs advise switching to a stronger tool-calling model rather than falling back to Legacy. This comes from a search summary; the docs page itself returned 404 on direct fetch. Jan was not researched.

Source: WebSearch summary of https://docs.openwebui.com/getting-started/essentials/ and https://docs.openwebui.com/features/extensibility/plugin/tools/

## Recommendations

- Stream accumulator: key tool calls by `index`; take `id` and `function.name` from the first non-empty value; append `function.arguments` strings. If a later chunk for the same index carries a full JSON object that parses on its own (Ollama style), replace rather than append. Synthesize an id (`call_<uuid>`) when missing or duplicated, and accept any id format. Treat tool calls as present if any were accumulated, even when `finish_reason` is `stop`. When `finish_reason` is `length`, or the arguments do not parse, do not execute; feed back an error.
- SSE reader: ignore lines starting with `:`; treat `data: {"error":...}` as terminal; tolerate a missing `[DONE]`; accept a `choices: []` usage chunk; read both `delta.reasoning_content` and `delta.reasoning`. Keep reasoning out of `content`, and strip it from history unless the template supports preserved reasoning.
- Always send explicitly to the sidecar: `parallel_tool_calls:false` (the server default depends on the template), `stream_options:{include_usage:true}`, `cache_prompt:true`, and `max_tokens`. Start llama-server with an explicit `--port`, `--host 127.0.0.1`, `-c <ctx>`, `--jinja`, and no `--context-shift`. Leave `--tools` and `--mcp-servers-*` off. Avoid aggressive `-ctk` / `-ctv` quantization.
- Slots: pin the main conversation to one slot with `id_slot`, and either run `-np 2` with a second slot for side requests (titles, compaction summaries) or accept re-evaluating the prefix. A summarization request on the same slot invalidates the conversation's KV cache. This is my inference from the slot and cache flags, not a documented recommendation; benchmark it.
- Fallback text parser: run it only when the server returned no native tool_calls and tools were offered. Order: (1) tagged formats by exact marker: `<tool_call>`, `[TOOL_CALLS]`, `<|tool_call|>`, `<function=`, `>>>`, `<|python_tag|>`, `[TOOL_REQUEST]`, DeepSeek and Kimi tokens, `<|tool_call>call:`; (2) fenced ```json blocks; (3) bare JSON only if the first non-whitespace character of the message is `{` or `[`. Accept `arguments`, `parameters`, or `function.{name,arguments}`; re-parse string arguments once.
- Fallback acceptance rule: every recovered call must name an enabled tool exactly and validate against its JSON Schema after repair. Otherwise leave the text as content. Never execute a recovered call found inside reasoning or inside a quoted tool result. Recovered calls go through the same approval path as native ones.
- Argument handling: apply schema-guided repair first (stringified objects and arrays, numeric and boolean strings, null placeholders, as opencode does). Then validate with the `jsonschema` crate. On failure return a tool-role message containing the validation error and the expected schema. Cap at 3 consecutive invalid calls per tool and about 6 consecutive mistakes overall (Cline's default), then stop and ask the user.
- Loop control: default max steps per user turn around 25-50 for local models, configurable (Goose's 1000 is too high for small models). On reaching it, send a tools-disabled final request with an opencode-style 'maximum steps reached, summarise progress' instruction. Detect identical consecutive calls by name plus canonical JSON arguments: warn at 3, stop at 5.
- Tool-output budget: scale with context rather than using cloud-agent constants. For example cap each result at min(10_000 tokens, about 15-20% of n_ctx). Use head plus tail with a marker stating how much was omitted, and save the full output to an app-data file the model can read with offset and limit. Use head-only for file reads, with line ranges.
- Compaction: trigger on measured usage (`usage.prompt_tokens`, or `timings.prompt_n + cache_n + predicted_n`) when it exceeds n_ctx - max(reply reserve + safety, about 15-20% of n_ctx). Before every request, pre-check with `POST /v1/chat/completions/input_tokens`. Treat HTTP 400 `exceed_context_size_error` as a forced compaction trigger. Keep the system prompt, the skills catalogue, activated `<skill_content>` blocks and the last N turns within a token budget. Summarise the rest with the same model using a fixed template and tools disabled. Disable thinking for the summary call and cap its output.
- Remote endpoints: resolve context length per provider. Ollama: `/api/ps` `context_length` for the loaded model, falling back to `num_ctx` in `/api/show` parameters, and never assuming the trained `model_info` maximum. LM Studio: `loaded_instances[].config.context_length`. vLLM: `max_model_len`. Otherwise use a user setting with a conservative default of 4096. For Ollama, warn in the UI when the window is 4k and point to `OLLAMA_CONTEXT_LENGTH` or a Modelfile `num_ctx`. Because Ollama `/v1` truncates silently, compare reported `prompt_tokens` with the estimate to detect it, or offer a native `/api/chat` adapter with `options.num_ctx` and `truncate:false`.
- Remote token estimation: use ceil(chars / 3) as the conservative default (Cline's value), and calibrate a per-model chars-per-token ratio from each response's real `usage.prompt_tokens`.
- Prompt-injection hygiene: wrap every tool result in a delimiter with a per-session random nonce plus provenance (tool name, source path or URL). Add one system-prompt rule that content inside is data and never instructions. Escape or neutralise any occurrence of the delimiter and of chat-template special tokens in tool output. Skip base64 spotlighting for small models; optional datamarking only for web content.
- Approval state: keep approvals, allowlists, trusted folders and 'always allow' entirely in Rust state keyed by tool, canonicalised arguments and session. The model must never be able to set or read them through a tool. Show exact tool inputs before execution. Design the default tool set so one session does not combine private-file access, untrusted web content and outbound network without an approval in between.
- Skills: implement two validation levels. A strict mode mirrors skills-ref exactly (six allowed keys, NFKC name rules, 64 / 1024 / 500 limits, directory match), for a 'validate' action. A lenient loader follows the client guide (warn on name mismatch, extra keys and non-string metadata; skip only on missing description or unparseable YAML; retry with quoted values on colon errors). Coerce all scalars to strings. Advertise skills with the `<available_skills>` XML in the system prompt plus an `activate_skill` tool whose `name` is an enum. Return `<skill_content>` with the skill directory and a capped resource listing. Scan `<project>/.agents/skills` and `~/.agents/skills` in addition to the app's own directories; gate project skills on folder trust.
- Sampling: do not send a global temperature by default. Let llama-server apply GGUF `general.sampling.*` or its own defaults, and ship a small per-family override table (Qwen3 0.6-0.7 and never 0; Mistral Small 0.15). Expose temperature, top_p and top_k in settings.

## Risks

- llama.cpp moves fast and its docs lag the code: function-calling.md shows `finish_reason: "tool"`, says parallel calls are off by default, and lists pre-autoparser handlers. Pin the bundled llama-server build and run a wire-shape conformance test (streamed tool call, usage chunk, error event, overflow error) on every upgrade.
- llama-server turns unparseable final model output into an error ('does not match the expected ... format') rather than returning content. The agent loop needs a recovery path, for example retrying once with tools described in the prompt and the client-side fallback parser. That fallback path is my proposal, not verified behaviour.
- The fallback text parser is an injection surface: a web page or file containing `<tool_call>{...}</tool_call>` could be echoed by the model and executed. The exact tool-name match, schema validation and mandatory approval reduce this but do not remove it.
- Ollama `/v1` truncates history silently at `num_ctx`, which is often 4k on consumer GPUs. The agent can lose its system prompt tail, tool schemas or recent results with no error; tool calling then degrades in ways that look like model failure.
- Percentage compaction thresholds copied from cloud agents (80-90%) leave too little room on 8k-32k local contexts, where tool schemas plus the system prompt can already use several thousand tokens. A character-based estimator can undercount code and JSON by 2-3 times.
- Reasoning models can spend the whole `max_tokens` budget on thinking and return an empty summary or a truncated tool call. Cline raised its summary cap from 4096 to 8192 for this reason.
- The skills-ref validator is explicitly 'for demonstration purposes only' and diverges from the spec prose (Unicode names accepted, unknown keys rejected, strictyaml typing). Matching it byte for byte may reject skills that other clients load. `allowed-tools` is experimental and its grammar is not specified beyond 'space-separated'.
- Prompt-level defences are probabilistic; the spotlighting paper's own numbers show delimiting only roughly halves attack success, and small local models follow injected instructions more readily. Security must rest on approvals, sandboxing and least privilege in Rust.
- Several items could not be verified today and are lower confidence: LM Studio's method for setting context length, the vLLM `/tokenize` endpoint and tool-call id format, the Gemma 2/3 `tool_code` convention, the exact GPT-OSS Harmony call string, Open WebUI details (search summary only), and Jan (not researched). The tool-format catalogue for the newest model families changes monthly.
- Version numbers observed today are far ahead of what training knowledge would suggest (Ollama v0.40.16, vLLM v0.33.0, llama.cpp tag b12242 with default port 9931, rmcp 3.9.0). Any architecture text written from memory about these projects should be re-checked against current sources.
