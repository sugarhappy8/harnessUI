# harnessUI 0.0.5: combined map of the codebase for the lead

Repository: /home/user/harnissUI. The product name is "harnessUI" (src-tauri/tauri.conf.json:3) and the version is now 0.0.4.

I combined the five reader reports and re-checked the points where they disagreed or where a decision depends on the detail. Everything listed under "Re-verified in this pass" I read myself in this pass. I ran no new builds or tests. The test runs and live binary probes quoted below were done by the readers, and I name which reader each time.

## Re-verified in this pass (by reading)

- **The load command.** `load_local_model` is at src-tauri/src/models_cmd.rs:30-91.
  - The "already loaded" check is at :71-80 and compares only three things: the path, `n_ctx == settings.ctx` (after the trained-length cap) and `settings.backend.is_none_or(== current backend)`.
  - The device is never compared.
  - With backend Auto, after the model has stepped down to the CPU, picking it again is a no-op. So today the user has no way to retry the GPU.
- **The diagnostics reload loses the image projector.** `run_diagnostics` reloads the previous model at models_cmd.rs:354-362 with the stored global settings. `set_local_model_settings` clears `mmproj` before saving (models_cmd.rs:108), so a vision model comes back without its projector. It also skips the trained-length cap, `set_thinking` and the speed check.
- **The settings struct.** `LocalModelSettings` (src-tauri/src/dto.rs:177-199): only `mmproj` has `#[serde(default)]`, and `ctx` is a plain `u32`. `settings::local_model_settings` (src-tauri/src/settings.rs:58-67) silently falls back to the defaults when the stored value cannot be read.
- **The generic settings command.** `set_setting` takes only the keys in `ALLOWED` at models_cmd.rs:193-198: theme, defaultSystemPrompt, remoteContextLength and stepsPerTurn.
- **Joining a load already running.** `same_in_progress` is defined at src-tauri/src/load_tracker.rs:50-57 and compares the whole `LocalModelSettings`. It is called from `RuntimeManager::load` (runtime_manager.rs:360) and from `join_load` (runtime_manager.rs:533-539).
- **The launch values.**
  - `RuntimeManager::load` builds `LaunchConfig` at runtime_manager.rs:398-413: `gpu_layers` is All, or Count(0) on CPU; `cache_ram_mib` is 512; `reasoning_budget` is None; a fresh port and a fresh key on every load. It calls `unload()` at :368 before anything else.
  - `GpuLayers::Auto` exists (crates/runtime/src/llama/launch.rs:37-55) but the app never uses it.
  - `build_args` is at launch.rs:114-161, and `build_env_on` (launch.rs:172-180) sets LLAMA_API_KEY.
- **Where `LaunchConfig` is written out in full:** runtime_manager.rs:398, supervisor.rs:450, server.rs:694 (the other server.rs tests copy from it with `..launch()`), launch.rs:205 plus several launch.rs tests, and crates/runtime/tests/llama_server.rs:51.
- **The start-up identity check.** server.rs:357 checks that `/props` `model_path` matches the launched file. Anything that renames the model must not change that field.
- **Telling whether a reply is being written.** `state.generation` (src-tauri/src/state.rs:24-95) has `start`, `finish`, `cancel`, `cancel_chat` and `running_in(chat)`, but no "anything running?" query. The `stop_generation` command is at chat.rs:310-316. The load path never consults it.
- **Storage.**
  - `settings` is a key/value table with JSON values (crates/store/src/schema.rs:250-254), so new keys need no migration.
  - The `models` table (schema.rs:171-181) has no settings column.
  - `audit_log.decision` has a closed CHECK list. V2 (schema.rs:260-414) rebuilt the whole table just to add 'allowed_by_mode'. So a new decision value costs a V3 table rebuild that keeps the append-only triggers.
  - `MIGRATIONS` holds V1 and V2 only (schema.rs:414).
  - The Store has `get_setting`, `set_setting` and `delete_setting` by exact key only (crates/store/src/config.rs:459-486). There is no prefix listing.
- **The per-model key pattern.** The 0.0.4 profiles use `modelProfile:` + 16 hex digits of SHA-256("local\n<file>\n<size>\n<backend>") (src-tauri/src/evals/profile.rs:21-55).
- **Something else that reads the global context.** `context_for_defaults` (src-tauri/src/mcp_cmd.rs:966-984) reads the global `localModel.ctx`.
- **Pinned llama-server help** (the reader's saved copy: /tmp/claude-0/-home-user-harnissUI/23ef2267-cb6f-5325-b082-a4b40b3ccb19/scratchpad/help.txt):
  - `-ngl` defaults to "auto".
  - `--fit` adjusts "unset arguments" only, and `-fitc` defaults to 4096.
  - `-a/--alias` exists (line 506), and so does `--api-key-file` (line 602).
  - `-lm` takes auto, none, mmap, mlock, mmap+mlock or dio.
  - `--cache-ram` defaults to 8192.
- **The text rule.** docs/dev/0.0.4/spec.md:86-88 says user-facing text must be "plain, short, no jargon, no exclamation marks. Never put names or identifiers of AI models or assistants into code, comments, tests or docs". ARCHITECTURE.md:757 adds "No model name appears in code".
- **Tests that click or check exact labels.**
  - e2e scripts m1, m3, m5, m8 and m9 call `set_local_model_settings` and `load_local_model` with `{ctx, backend, device}` (for example e2e/m1_chat.py:137-138).
  - e2e/scripted_agent.py:299-300 clicks a fixed list of Settings tabs. Adding a tab is safe; renaming one breaks the script.
  - e2e/m2_models.py:128 clicks "Use".
  - src/components/Welcome.test.tsx:54 expects "Ready on vulkan · Vulkan0 · 65,536 context".
- **The HTTP lint.** clippy.toml disallows only `std::process::Command::new`. The lint described in ARCHITECTURE 13.6 that confines HTTP clients to one module does not exist.
- **The catalog.** catalog/models.json has 14 entries. One repo, unsloth/Qwen3.8-27B-GGUF, appears as two entries (UD-Q4_K_M and UD-Q6_K), so matching a Hugging Face repo to the catalog needs the repo and the file. Every entry says spdx apache-2.0.

## Where the readers disagreed, and how it resolves

1. **Where `same_in_progress` lives** (reader 1: load_tracker.rs:50-57; reader 5: runtime_manager.rs:532-539). Both are right. It is defined in load_tracker.rs and called from runtime_manager.rs:360 and :538.
2. **Line numbers of the `set_setting` allow-list** (193-198 against 186-194). The function starts at :185 and the list is at :193-198. This changes nothing.
3. **"Every new field needs serde(default)"** (reader 1). This needs one correction. With serde derive, a missing `Option<T>` field already reads as None, so new `Option` fields are safe in old saved JSON. New non-Option fields (bool, enum, u32) do need `#[serde(default)]`. That matters in two directions:
   - the 0.0.4 `localModel` JSON already stored on users' machines;
   - every caller that sends the old shape today: the frontend and the five e2e scripts.

   Without the default, both fail. On read the fallback is silent and the user's saved backend is lost. On a command call it is a deserialisation error.
4. **Reload behaviour.** Readers 1, 2 and 5 say that picking the model again reloads it when the context or backend changed. That is right, but incomplete. With Auto it never retries the GPU after a step-down, and the device is ignored. A Reload button therefore needs a forced path; calling `load_local_model` again is not enough.
5. **Model names in code.** Reader 4's direction-B design puts `claude` and `codex` into the policy classifier and into tool names. Reader 5 points to the text rule above, which forbids assistant names in code, tests and docs. This is a real conflict and the lead must decide. My recommendation: amend the rule for 0.0.5 so that the program names of outside apps the owner asked to integrate may appear in one module and its tests, while model names stay out of code (catalog only). The alternative is to describe the apps generically and supply the names as data, which is clumsy for a security classifier.
6. **Where comparison lives** (reader 3: mostly from the signed catalog, through a new catalog list; reader 5: a Compare tab in the Models dialog fed from Browse and Installed). The owner said "from the download huggingface thing we have", which is Models → Browse. Resolution: the comparison lives in the Models dialog and takes models from Browse, Installed and a catalog list. The catalog supplies the curated "good at" text only when a repo and file match an entry.
7. **Places where ARCHITECTURE.md and the code disagree** (several readers). Listed under "Documentation drift" below.

---

## Item 1: "allow per model on loading context window setting and it auto compresses"

### What exists
- **One global context size.** Settings → Local models → "Context size" offers 2,048 to 65,536 (src/components/SettingsDialog.tsx:433-445). Help text: "Changes apply the next time a model loads. A larger context needs more memory." (:448-450).
  - It is validated to 512..=1,048,576 (models_cmd.rs:110-114) and capped silently at the trained length (models_cmd.rs:59-69, written to the log only).
  - After the load, the real size comes from `/props` (runtime_manager.rs:446-463) and every turn budgets against it (chat.rs:462-487, 574-576).
- **Automatic shrinking of long chats already exists** (crates/agent/src/context.rs, turn/runner.rs:611-669).
  - The budget is n_ctx − min(4096, n_ctx/4) − (64 + n_ctx/256), and shrinking starts at 80% of it (context.rs:41, 97-117).
  - Stage 1 replaces old tool results with short stubs. Stage 2 summarises the oldest turns with the same model, up to 4 rounds per request. Stage 3 stubs this turn's older tool calls, never the latest.
  - A "too long" reply from the server triggers one retry. When nothing more can be cut, the turn fails with `context_full`.
  - Summaries are stored in the `summaries` table (schema.rs:100-108) and sent inside `<conversation_summary>` (prompt.rs:604-608).
  - Reader 2 ran `cargo +1.99 test -p agent --test turn_loop` and the compaction tests pass.
- **Visibility is thin.**
  - A short-lived note says "Earlier messages were summarised to make room in the context." for every stage, which is wrong for stages 1 and 3 (src/store/app.ts:237-241). Stage 2 also shows "Summarising earlier messages to make room in the context window." (runner.rs:777-779).
  - The divider described in ARCHITECTURE 8.4 was not built.
  - When a summary fails, nothing is shown (runner.rs:772-788).
- **The fit check** (crates/models/src/fit.rs:124-195) gives verdicts plus `largest_ctx` / `suggestedCtx`. It assumes an f16 cache (summary.rs:105-114) and runs only in Browse, with the global context size (hub.rs:283; the UI passes `ctx: null` at ModelsDialog.tsx:197). The suggested context is never offered to the user.
- **Catalog `recommendedCtx` per tier** reaches FirstRun but no component reads it.

### What is missing
- **A per-model value, and one backend function that resolves it** in the order: per-model, then global, then the trained-length cap, then optionally a memory check. Today every caller passes the global settings: app.ts:396-397, ModelsDialog.tsx:553-557, FirstRun.tsx:78-79, run_diagnostics, and the e2e scripts.
- **Per-model controls where models are loaded.** There is no choice sized by the trained length and the fit check, and no "Automatic: as much as fits" option.
- **A reload action.** The context is fixed when llama-server starts, so a change needs a reload (see item 2).
- **Visible shrinking.**
  - The `compacted` event should say what happened.
  - A lasting divider with "Show summary" needs a summary field on `ChatDetailDto`; today only `turns.rs` and `task.rs` read `latest_summary_on_path`.
  - A failed summary should be reported.
- **Robustness when the context gets smaller.**
  - An old summary of up to 1,024 tokens, plus loaded skills, can push the fixed overhead past the 40% tools share. The refusal then blames the tools (runner.rs:553-563). Reader 2 inferred this; it was not run.
  - The summary is never folded again (runner.rs:726-729).
  - `tool_select::other_fixed` ignores the summary.
  - Attachments are capped only at 2 MiB of text, and the document cut described in ARCHITECTURE 8.6 does not exist.

### Hard parts
- **Which identity to key by.** Use file name + size, without the backend, following the profile.rs pattern. This also covers files opened with "Open a GGUF file…", which live only in `recentLocalModels` and have no `models` row. Keying by path breaks when a file moves. Including the backend makes the setting disappear when the backend changes.
- **"What fits" while another model is loaded.** Free VRAM comes from a probe cached at start-up (hub.rs:121-188; runtime_manager.rs:163-183). Options: subtract the loaded model's footprint, re-probe after unloading, or run llama-fit-params. Reader 1 ran it on CPU: about 0.25 s, nothing allocated, and it prints the fitted `-c`/`-ngl` and a MiB breakdown.
- **Keep what must stay.** `-c` must stay explicit and `--no-context-shift` must stay (contract test, launch.rs:234). All shrinking stays inside the app.
- **Lost prompt cache.** A reload drops llama-server's prompt cache, so the first reply re-reads the whole chat, which is slow on CPU.

### Ambiguities in the owner's wording
- **"per model":**
  - (a) per model file, recommended;
  - (b) per file and backend, which is more exact for GPU memory but the setting vanishes when the backend changes;
  - (c) per chat, which forces a reload whenever the user switches chats, because only one model loads at a time.
- **"on loading":**
  - (a) remembered per model and used automatically, recommended;
  - (b) asked every time in a load dialog, which adds a click to every load;
  - (c) a load dialog pre-filled with the remembered value. Good for "Load with…", but the default path should stay one click.
- **"auto compresses":** four readings, all possible with the pinned build.
  - (a) The existing summarising of long chats, made visible and robust. Cheapest, and already automatic.
  - (b) A "save memory" mode: the memory that holds the conversation is stored at half size (`-ctk q8_0 -ctv q8_0 -fa on`), so a longer context fits. Reader 1 measured 3,584 MiB down to 1,906 MiB at 32k on CPU. Support on GPU backends is not verified.
  - (c) The app picks a context that fits automatically (`fit::largest_ctx` or llama-fit-params).
  - (d) Shrinking ahead of time, after a reply or after a reload to a smaller size, or a "Make room now" button. This costs a cache re-read at that moment.

  Recommendation: (a) + (c) as the default, (b) as a checkbox, (d) later. Confirm with the owner.
- **Sub-question for (c):** should "Automatic" be capped (at the catalog's recommendedCtx or 32,768), or truly "as much as fits"? A huge context leaves less memory for the rest of the computer.
- **Remote endpoints:** should they get per-model overrides too? Today there is one global `remoteContextLength` (settings.rs:12).

### Risks and constraints
- **Saved-data compatibility:** see disagreement 3 above.
- **Commands:** the new key needs its own validated command, because `set_setting` refuses unknown keys (13.5).
- **Platform memory rules:** the Apple unified-memory limit (fit.rs `Target::AppleUnified`) and NVIDIA spilling into system memory, reported as "Loaded but slow" (runtime_manager.rs:797).
- **Summaries are model output.** They must be shown as plain text, never markup. Pins are never shrunk.
- **The context inspector** must keep using `turns::assemble(Preview)` and `agent::preview_turn`. Any new stage must show in `NextCompaction` / `next_drop`.

### Recommended approach and files
- **Storage and resolver.** A new settings key `modelLoad:<16 hex of SHA-256("local\n<file>\n<size>")>` holds a record shared with item 2, with every non-Option field `#[serde(default)]`. A new `src-tauri/src/load_settings.rs` provides `resolve(path) -> LocalModelSettings` (per-model, then global, then projector, then trained-length cap) and `ctx_for(path)`.
- **The load command.** `load_local_model` takes `settings: Option<…>` so the e2e scripts keep working, and resolves the settings itself when it is None.
  - `run_diagnostics` goes through the same function.
  - `context_for_defaults` (mcp_cmd.rs:966-984) and the Browse fit check (hub.rs:283) read `ctx_for`.
- **The "Automatic" option** uses `fit::largest_ctx` with a KV-cache-type-aware `kv_bytes_per_token` in summary.rs and fit.rs. Optionally run llama-fit-params (`Backend.fit_params`, backend.rs:100-101) for installed models.
- **Making shrinking visible.**
  - The `TurnEvent::Compacted` stage reaches the UI with a stage-specific plain note.
  - `ChatDetailDto` gets a summary, and ChatView draws a divider: "Earlier messages are summarised for the model. Show summary".
  - Failed summaries are reported.
  - The old summary can be folded again under a smaller cap.
  - `other_fixed` counts the summary.
  - Attachments are cut to a share of the context, the way `resolve::cap_chars` cuts mentions.
- **New tests in crates/agent/tests/turn_loop.rs:** "a chat summarised at 32k continues at 4k", "an oversized old summary is folded", "an attachment larger than the context is shortened".

---

## Item 2: "More and more model loading setting and model reloading but simplify it to normal human language"

### What exists
- **Three global settings** (ctx, backend, device); only Backend and Context size have a control. There is no device picker, although ARCHITECTURE 7.2 describes one.
- **Fixed in code:** `-ngl all` (or 0 on CPU), `--cache-ram 512`, no `--reasoning-budget`, `-np 1`. Everything else is llama.cpp's default: threads auto, `-b 2048`, `-ub 512`, `-fa auto`, f16 cache, `-lm auto`, `--fit on`. Because `-c` and `-ngl` are always set, `--fit` changes nothing (reader 1, inferred from the help text).
- **Restarts and step-down.**
  - After a crash the app restarts up to 3 times in 5 minutes (supervisor.rs:103-111).
  - After a load failure it steps down ROCm/CUDA → Vulkan → CPU (supervisor.rs:137-150, `step_down_spec` 309-327), changing only the backend and device. A model that runs out of GPU memory goes entirely to the CPU.
  - Yet the out-of-memory message says "Use a smaller context, run part of the model on the CPU, or pick a smaller model." (crates/runtime/src/llama/failure.rs:55), and nothing does that.
- **No Reload or Unload button.** `ipc.unloadModel` exists (ipc.ts:136) but nothing calls it. A reload stops the server at once, even mid-reply, and the reply then ends with "Could not reach the model server: …" (inferred by reader 1, not reproduced).
- **Jargon on screen.**
  - Backend option labels are the raw lower-case backend names, such as 'vulkan' and 'cpu (unavailable)'.
  - The loading line shows llama.cpp's raw stage name with one underscore replaced, for example 'Loading model (text model) N%' (RuntimeIndicator.tsx:14).
  - When ready, the header shows 'vulkan · Vulkan1 · 8,192 context'.
  - On step-down: '<kind> failed (<reason>); trying <kind>' (runtime_manager.rs:594-599).
  - Also: 'X of Y layers on the GPU' and 'Model failed to load'.

### What is missing
- More settings, each a typed field in `LaunchConfig` and `build_args`.
- Reload, Unload, and "Apply and restart".
- A "settings changed: restart to use them" indicator.
- A safe reload while a reply is being written.
- Plain words for backends, stages and warnings.
- Partial GPU use.
- A Reload that works after a step-down.

### Hard parts
- **Two comparisons must learn every launch field:** the no-op check (models_cmd.rs:71-80) and `same_in_progress` (load_tracker.rs:55). Otherwise a reload with new settings is silently skipped or wrongly joined. Reload also needs a `force` flag.
- **Partial GPU use is not verified.**
  - It is unclear whether `-ngl auto` passed explicitly counts as "unset" for `--fit`. Omitting `-ngl` is the safer way to get llama.cpp's fitting.
  - The KV cache at q8_0 needs flash attention, and kernel support varies by backend.

  Neither can be tested here, because this machine is CPU only. They need the owner's Ryzen + RX 9070 XT (Vulkan, Windows) and the 16 GB Apple silicon Mac.
- **Keeping the model in memory** (`-lm mmap+mlock`; this build has no `--mlock` or `--no-mmap` flags) may need raised limits or privileges on Windows and macOS (inferred).
- **New `LaunchConfig` fields** touch the full literals listed above, the `args_are_exact` test and the contract tests (ARCHITECTURE 17). A constructor or Default would reduce the churn.
- **A reload during a reply.** The app needs a "busy?" query on `Generation`, then: ask the user, `stop_generation`, wait for the turn to finish winding down, reload.

### Ambiguities
- **Which settings to show?** Recommended visible set:
  - "How much it can remember" (context);
  - "Use the graphics card": "As much as fits" (no `-ngl`, `--fit on`), "All of it" (`-ngl all`), "Don't use it" (CPU);
  - "Save memory (lets it remember more, slightly lower quality)" (`-fa on -ctk q8_0 -ctv q8_0`);
  - "Keep the model in memory" (`-lm mmap+mlock`).

  Behind "More options": "Which graphics card" (device) and "How long it may think" (`--reasoning-budget`). Hidden: threads, batch, slots, rope, tensor split.
- **Per model or global** for each setting: recommended per model, with the global value as default; the backend may stay global.
- **Restart when a setting changes:** automatically, or only on "Apply and restart" (recommended)?
- **A reply is being written:** wait, stop, or ask (recommended: ask)?
- **Default GPU behaviour:** should "As much as fits" replace today's all-or-nothing as the default, and be the first retry before the CPU? The out-of-memory message already promises it.
- **Renaming existing labels** pinned by tests ('Use', 'Local models', 'Tested settings', 'Check this model', 'Ready on vulkan · Vulkan0 …'): allowed if the tests change in the same commit?

### Risks and constraints
- **Security** (ARCHITECTURE 7.3, 13.5). Settings must be typed values from fixed lists, never free-text "extra arguments" or file paths. llama-server reads `LLAMA_ARG_*` for dangerous flags (--tools, --agent, --connect, --host, --lora, --chat-template-file and others). Keep the environment scrubbing, `--offline`, `--no-slots` and `--cors-origins none`.
- **Memory on 16 GB machines.** `-np 1` and the small `--cache-ram` are deliberate (ARCHITECTURE 7.3).
- **ROCm** must keep the `HIP_VISIBLE_DEVICES` masking (runtime_manager.rs:717-727).

### Recommended approach and files
- **Backend.**
  - Extend `LaunchConfig` and `build_args` (launch.rs) with: `gpu: Auto|All|None`, `flash_attn` and `kv_type` together as "save memory", `load_mode`, and a reasoning budget from a fixed list. Update `args_are_exact` and the contract tests.
  - The supervisor tries "as much as fits" before stepping down to the CPU (supervisor.rs:137-150, 309-327).
  - New commands `reload_local_model(force)` and `model_load_info(path)` (trained length, file size, fit per candidate size, saved settings, whether it is loaded). Wire up the existing `unload_model`.
  - A busy check on `Generation` (state.rs).
- **Frontend.**
  - A new per-model panel ("Model settings") opened from a gear on Installed rows and from a popover on RuntimeIndicator, offering "Reload", "Change settings…" and "Unload to free memory".
  - Settings → Local models becomes "Defaults for all models".
  - A plain-label map for backends, stages and warnings in a new `src/lib/words.ts`, for example: "Graphics card (Vulkan)", "NVIDIA graphics card (CUDA)", "AMD graphics card (ROCm)", "Apple graphics (Metal)", "Processor only (slower)"; "Reading the model file", "Loading the image reader", "Restarting after a problem (1 of 3)", "The graphics card ran out of memory; using the processor instead".
  - When the model is loaded with older settings: "Settings changed. Restart the model to use them."
  - Copy the help-text patterns of StepsPerTurn and RemoteContext (SettingsDialog.tsx:94-155, 638-702).

---

## Item 3: "Allow for local loaded models to be able to talk to Claude or codex apps upon user request"

### What exists
Nothing built for this. The raw parts exist; reader 4 ran these:
- **The pinned llama-server already speaks every API these apps use.** It listens on 127.0.0.1 and serves `/v1/messages` and `/v1/messages/count_tokens` (Anthropic), `/v1/responses` (OpenAI Responses, needed by Codex because `wire_api` must be "responses") and Chat Completions. Tool use and streaming work.
- **Keys.** LLAMA_API_KEY accepts a comma-separated list: with two keys, both worked and a wrong key got 401. CORS answers with origin "none", so web pages cannot read responses. `/health` answers without a key.
- **Claude Code 2.1.322 worked against it**, run headless with ANTHROPIC_BASE_URL, ANTHROPIC_AUTH_TOKEN, ANTHROPIC_MODEL and CLAUDE_CODE_MAX_CONTEXT_TOKENS. Its first request was 11,088 tokens; the first token took 176 s on 4 CPU threads with a 0.6B model.
- **What blocks sharing today, by design:**
  - a new random port and a new random key on every load (runtime_manager.rs:391, 411), and the key is never shown;
  - the server runs only while a model is loaded;
  - one slot (`-np 1`);
  - a default context of 8,192;
  - the reported model id is the full GGUF path, which leaks folder names.
- **The "local model asks the outside app" route already half-works through the terminal tool.** The terminal tool can run `claude -p …` or `codex exec …`. Reader 4 ran the real classifier: these commands, even with `--dangerously-skip-permissions`, are neither network programs nor keep-asking, so under Allow all they run with no card, even after untrusted content entered the turn. The 600 s ceiling (crates/tools/src/terminal.rs:42-43) and the 1 MiB output cap get in the way. `looks_secret` removes ANTHROPIC_API_KEY and OPENAI_API_KEY from the environment.
- **The MCP route is weak.** `claude mcp serve` exposes Claude Code's tools, not the model. `codex mcp-server` has been removed. The app's MCP host is client-only: sampling refused, elicitation declined (crates/mcp-host/src/connection.rs:14). rmcp's server feature is compiled in (Cargo.toml:71) and the hidden-flag worker pattern exists (src-tauri/src/main.rs:4-24).

### Ambiguity: which direction? This must go to the owner
| Option | What it means | For | Against |
|---|---|---|---|
| A. Other apps use this model | Claude Code, Codex or Claude Desktop send their requests to harnessUI's loaded model | Verified end-to-end with Claude Code. Nothing leaves the computer. Most parts exist | Breaks the "random port, per-launch key" promise (ARCHITECTURE 13.1). Shares the one slot with the in-app chat. Small local models struggle with these apps' 11k-token prompts and tool sets. Anthropic's docs say routing Claude Code to non-Claude models is unsupported. Needs a context of 32k or more |
| A′. Other apps call this model as a helper tool | harnessUI runs as an MCP server with one tool, `ask_local_model`; those apps keep their cloud model | The apps stay at full strength | The stdio worker needs a safe way to reach the running app (no local IPC exists, and writing the key to a file puts a secret on disk). Codex's default MCP tool timeout is 60 s. More code |
| B. This model asks Claude Code or Codex for help | A new tool in harnessUI runs the installed program on a task, with a card every time | The closest literal reading ("local models … talk to … apps") | Data and files go to Anthropic or OpenAI, billed to the user. A big jump in what the model can do. Runs can be long. `claude -p` without `--bare` runs repository hooks and `.mcp.json` servers. Windows behaviour is unverified |
| C. Use Claude or OpenAI cloud models inside harnessUI | An Anthropic Messages client in Endpoints | Endpoints already handles OpenAI-compatible URLs | Probably not what the owner meant. No Anthropic client exists |
| D. The two models chat with each other ("second opinion") | A narrow form of B | Easy to explain | The same costs as B |

**What "upon user request" could mean:** a switch that is off by default (fits A), a per-chat tool toggle (fits B), or a card on every call (fits B, recommended). And which apps: Claude Desktop, Claude Code CLI, or Codex CLI, desktop app or IDE extension? Their setup paths all differ:
- Claude Code: environment variables, or the `env` block in `~/.claude/settings.json`.
- Claude Desktop: Developer Mode → Configure Third-Party Inference. Whether it accepts an `http://127.0.0.1` address is unverified.
- Codex: `[model_providers.<id>]` in `~/.codex/config.toml`. Real Codex traffic against llama-server's `/v1/responses` is unverified.

**Recommendation:** ask the owner one plain question: "Do you want Claude/Codex to use your local model, or your local model to ask Claude/Codex for help?" Without an answer, build A first: it is verified, keeps data on the computer, and is independent of B. Add B later as a narrow, always-asking tool. In either case, fix the classifier hole now (see work package 7). That fix also needs the text-rule decision.

### Risks and constraints
- **Loopback only, always.** Never 0.0.0.0, the LAN, or llama-server's `--connect`, `--tools`, `--agent` or `--mcp-servers-config`.
- **The sharing key** lives in the keychain, is off by default, shown only on request, and replaceable in one click. The app's own key stays private.
- **Who can use it.** Any program on the computer that has the key can use the model, including other user accounts and malware. Pasting the key into `~/.claude/settings.json` stores it as plain text, and the setup cards must say so.
- **A fixed port can be taken by another program first.** Then the outside app may be talking to an impostor. An app-owned proxy on a stable port, with llama-server staying on its random port behind it, avoids handing out a port the app does not control.
- **Model id.** `--alias` exists, but the start-up check compares `/props` `model_path` (server.rs:357). Inferred: an alias renames only the API id. Verify with a contract test, or let a proxy answer `/v1/models` itself.
- **Slot contention.** An outside app's 11k-token request evicts the in-app chat's prompt cache. More slots split the context (n_ctx per slot = c / np).
- **Audit.** An "outside app used the model" row must either reuse an existing `decision` value (for example 'allowed_by_grant', with `server` set to a sharing name) or pay for a V3 table rebuild.
- **13.5 rule.** Only main-window UI commands may change the sharing switch and key; never a tool, skill or MCP server.
- **Direction B needs** a dedicated Scope that is keep-asking and network, fixed argument templates (never flags chosen by the model), approved folders only, a long timeout with Stop killing the process tree, results returned as untrusted output, and a plain warning that data leaves the computer.
- **Docs to update:** ARCHITECTURE 7.3, 13.1 and 13.6, and SECURITY.md. Add contract tests for `/v1/messages`, `/v1/responses` and multi-key auth, because the release discipline in 7.1 expects these to change between llama.cpp builds.
- **Dependencies.** A proxy needs a direct HTTP server dependency: hyper 1.x is in Cargo.lock, but not as a direct dependency with server features. That brings deny.toml and THIRD-PARTY-NOTICES.md updates.

### Recommended approach and files
**A, "Let other apps use this model"** (off by default):
- A new `src-tauri/src/share/` with three parts:
  - `mod.rs`: a state machine, a keychain key through `state.secrets`, its own commands, and a settings key such as `shareWithApps`;
  - `proxy.rs`: listens on a stable 127.0.0.1 port. It allows only `/v1/messages`, `/v1/messages/count_tokens`, `/v1/responses`, `/v1/chat/completions` and `/v1/models`. It authenticates with the sharing key and forwards with the app's private key from `RuntimeManager::endpoint()` (runtime_manager.rs:103). It answers `/v1/models` itself with a short name. It answers "No model is loaded in harnessUI" while unloaded or reloading, and "busy" or waits while `state.generation` runs. It writes one audit row per request;
  - `setup_text.rs`: copy-ready blocks for Claude Code, Codex and Claude Desktop, marked "experimental".
- New UI: `src/components/settings/OtherApps.tsx` and a new Settings tab.
- A warning when the loaded context is under about 32k, linking to the item 1 setting.
- A new test file crates/runtime/tests/llama_endpoints.rs.

Because the proxy forwards with the app's own key, this needs no change to launch.rs.

**B, if chosen:** a tool group modelled on src-tauri/src/subagent.rs, off per chat, offering "ask the coding app" tools only when the program is found on PATH.

---

## Item 4: "compare models … and what they're good at, from the download huggingface thing we have"

### What exists
- **Models → Browse** (src/components/ModelsDialog.tsx:323-436) is pure Hugging Face search.
  - Results show the repo id, a lock if gated, the licence and downloads. Likes, lastModified and pipelineTag are fetched but not shown.
  - RepoDetail (188-321) shows architecture, parameters, trained context, tools, thinking and vision badges, the licence, a fit badge per variant, and the model card as plain text.
  - `parse_repo_detail` (hf.rs:704-765) downloads `tags` and `cardData` (base_model, language, pipeline_tag) and keeps only the licence.
- **The signed catalog** (14 entries) has capabilities only (tools, vision, thinking). It has no summary, strengths, parameter count or languages. Every `tested` and `memoryAtRecommendedCtx` is null, and the catalog is shown only on the first-run screen.
- **Catalog refresh is disabled in every build** because `pubkey` is "" (src-tauri/tauri.updater.conf.json), so catalog changes arrive only with new app builds.
- **Older parsers ignore unknown catalog fields.** Reader 3 ran this: extra fields were accepted, and schemaVersion 2 was rejected with "unsupported schema version 2".
- **The 0.0.4 evals.** Profiles (`modelProfile:*`) and benchmark runs (`<app data>/benchmarks/*.json`, newest 50) exist, and `CompareTable` (evals/Benchmarks.tsx:89) puts two runs side by side. None of them carries a repo or catalog id.
- **Installed models have no fit verdict.** `register_model` ignores `GroupMeta.catalog_id` (downloads.rs:508-547).

### What is missing
- A compare view and a way to pick models for it.
- "Good at" data from any source.
- Plain wording for 'Q4_K_M', 'MoE', 'tools', 'gated' and raw architecture names.
- Hugging Face facts saved at download time, so the comparison works offline.
- One fit verdict at the same context for every column.
- A link from eval results to models.

### Hard parts
- **Honest "good at" for arbitrary Hugging Face repos.** GGUF re-uploads rarely carry useful tags, and uploader facts are self-declared.
- **Rate limits.** Each `hub_repo` call costs a detail call, a 6 MiB header fetch and the model card. Compare from views already fetched; never bulk-fetch.
- **Fuzzy matching.** A catalog match needs the repo and file (one repo has two entries). Imports have no repo and no sha256. Profiles hash file + size + backend.

### Ambiguities
- **Which models can be compared?**
  - (a) Hugging Face search results only: the literal wording, online only.
  - (b) installed models plus the catalog: works offline.
  - (c) all three, recommended. Hugging Face columns marked online-only.
- **Where "good at" comes from:**
  1. Curated, signed catalog fields such as `summary` and `goodAt` from a fixed vocabulary. Trustworthy and offline, but covers only 14 models; someone has to write and approve the text; it arrives only with app builds.
  2. Hugging Face uploader tags, pipeline_tag, base model and languages, mapped to plain phrases and labelled "the uploader says". Covers any repo, but sparse and untrusted.
  3. The user's own results (Tested settings, benchmarks) on this computer. Honest, but only for models downloaded and run, and tiny and noisy (ARCHITECTURE 13.14).
  4. The local model summarising the model card. Covers any repo, but needs a loaded model and is slow. It feeds untrusted text to a model, which readers 3 and 5 advise against. If built, no tools, plain-text output, labelled "written by your model, may be wrong".
  5. Publisher benchmark numbers (`evalResults`, `model-index`). Self-reported and usually missing on GGUF re-uploads. Leave out.

  Recommendation: 1 + 2 + 3; 4 off or later; 5 excluded.
- **What "compare" looks like:** a side-by-side table (recommended), or "try the same question on both" (an arena)? The arena means reloading between models because only one loads at a time; it could reuse the benchmarks.

### Risks and constraints
- **Signed catalog.**
  - Keep `schemaVersion` 1 and make every field optional (`#[serde(default)]`), or every existing parser rejects the catalog.
  - Bump `sequence` (hub.rs:891).
  - release.yml signs the file and checks the uploaded copy is byte-identical (release.yml:160-217, 261-265).
  - The signed test fixture (crates/models/tests/fixtures/catalog) cannot be re-signed because its secret key was discarded. Optional fields leave it valid.
- **No model names in code.** All "good at" text comes from the catalog JSON.
- **Offline rule** (13.6): opening Browse fires a search at once (ModelsDialog.tsx:353-358). An offline comparison relies on the bundled catalog plus facts saved at download.
- **Untrusted text.** Tags, cardData and the card are plain text only, and never override signed catalog facts.
- **Licences.** All three Gemma entries say apache-2.0 while Hugging Face links a separate Gemma licence (docs/research/huggingface.md:212). Fix that before showing a licence column side by side.
- **Network.** All Hugging Face requests keep going through HfClient.

### Recommended approach and files
- **Catalog.** Optional `summary`, `goodAt` (fixed keys mapped to plain phrases, unknown keys ignored), `parameters`, `activeParameters`, `publisher`, `languages`, `released` in crates/models/src/catalog.rs. Content in catalog/models.json with sequence 2, written by the owner or a maintainer.
- **Hugging Face.** Keep pipeline_tag, base model, languages and licence name/link in `parse_repo_detail` (crates/models/src/hf.rs). Show likes and last updated in search results.
- **Saved at download.** Store the catalog id and these facts in `gguf_summary` JSON in `register_model` (downloads.rs). StoredSummary fields default, so no migration is needed.
- **Your results.** A Store prefix query (crates/store/src/config.rs) plus a `list_model_profiles` command (evals/commands.rs), matched by file name + size.
- **Commands.** `catalog_list` (entries with fit, installed state and local results) and `compare_facts` in src-tauri/src/hub.rs, using one fit function at one chosen context.
- **UI.**
  - A "Compare" tab in the Models dialog (new `src/components/models/Compare.tsx`), with "Add to comparison" in RepoDetail, Browse and Installed, up to 3-4 models.
  - Rows phrased as questions: "Good at", "Fits this computer?", "Download size", "Can use tools", "Can read images", "Thinks before answering", "Longest conversation it was made for", "Licence", "Downloads", "Your test results". Each row notes its source.
  - Table style from Benchmarks, with `overflow-x-auto` and a caption.

---

## Documentation drift found (for the lead)

| Section | Says | Code |
|---|---|---|
| ARCHITECTURE 7.2 | A device picker | None (no device control in src) |
| ARCHITECTURE 7.3 | `-ngl <auto\|n>` and an always-present `--reasoning-budget` | `-ngl all` or 0, no budget |
| ARCHITECTURE 8.2 | Catalog sampling overrides | Not applied |
| ARCHITECTURE 8.4 | "Earlier messages were summarised" divider | Not built |
| ARCHITECTURE 8.6 | Documents cut to a token budget | No cut |
| ARCHITECTURE 13.6 | A lint keeps HTTP clients in one module | No such lint |
| ARCHITECTURE 11 | Catalog `tested` and measured memory | All null |

## Decisions the owner needs to make (in plain words)

1. **Item 3:** Should Claude and Codex use your local model, or should your local model ask Claude and Codex for help? Is "experimental, may stop working after their updates" acceptable?
2. **"Auto compresses":** summarising long chats, saving memory so more fits, picking the largest size that fits automatically, or several of these?
3. **Changing a model's settings:** restart it right away, or wait for an "Apply and restart" button? If a reply is being written: ask first?
4. **Which options to show.** Recommended: memory size, graphics card use, save memory, keep in memory.
5. **Who writes "what each model is good at"** for the catalog, and from what sources? Should the updater key be set up so the catalog can update between releases?
6. **May screen labels pinned by tests be renamed?** And may the names of the outside apps appear in code (the text rule)?

## Suggested work packages

**Shared files that every package touches** (the lead integrates these, or each package appends only):
- src-tauri/src/lib.rs (command list)
- src/lib/ipc.ts and src/lib/types.ts
- src-tauri/src/dto.rs and src-tauri/src/settings.rs (key constants)
- Cargo.toml and src-tauri/Cargo.toml
- the tab lists in SettingsDialog.tsx and ModelsDialog.tsx
- src/store/app.ts
- e2e/*.py

**Lead only:** crates/store/src/schema.rs (at most one V3 for the whole release), ARCHITECTURE.md, CHANGELOG.md, SECURITY.md, docs/.

**WP0, lead groundwork, done first and small:**
- Split ModelsDialog.tsx into `models/Installed.tsx`, `Browse.tsx`, `RepoDetail.tsx` and `Downloads.tsx`.
- Move SettingsDialog `Runtime()` into `settings/LocalModels.tsx`.
- Add the shared DTOs and stubbed commands: `LoadSettings`, `ModelLoadInfo`, `reload_local_model`, `model_load_info`, `set_model_load_settings`, catalog and compare DTOs, share status.
- Add a `LaunchConfig` constructor.
- Create the `src/lib/words.ts` skeleton.
- Amend the text rule.
- Decide on audit and V3.

**WP1, load engine (Rust; items 1 and 2):**
- Owns: crates/runtime/src/llama/launch.rs, server.rs (test literal only), server/supervisor.rs, failure.rs, crates/runtime/tests/llama_server.rs, a new crates/runtime/src/llama/fit_params.rs (runs llama-fit-params), src-tauri/src/runtime_manager.rs, load_tracker.rs, models_cmd.rs (load, reload, unload, diagnostics), a new src-tauri/src/load_settings.rs, crates/models/src/fit.rs and summary.rs (KV cache type), and the `Generation` busy query in state.rs.
- One-line edit: mcp_cmd.rs `context_for_defaults`.
- Must verify the GPU flags on the owner's hardware.

**WP2, load UI and plain words (items 1 and 2):**
- Owns: a new src/components/ModelSettings.tsx, settings/LocalModels.tsx, RuntimeIndicator.tsx (popover), Sidebar.tsx chip text, Welcome.tsx with Welcome.test.tsx, FirstRun.tsx (import), models/Installed.tsx, src/lib/words.ts, and app.ts `pickModel` / `openModelFile`.
- Depends on WP1's command contract from WP0.

**WP3, conversation memory (item 1, "auto compresses"):**
- Owns: crates/agent/src/context.rs, turn/runner.rs, turn/mod.rs, turn/runner/preview.rs, crates/agent/tests/turn_loop.rs, src-tauri/src/turns.rs (AppSink and `compacted`), context_cmd.rs, tool_select.rs (`other_fixed`), crates/store/src/tools.rs (summary for chat detail), the chat detail DTO in chat.rs, prompt.rs and attachments.rs (document cap), ChatView.tsx (divider, notes, composer text), ContextInspector.tsx, and the `compacted` handler in app.ts.
- Independent of WP1.

**WP4a, comparison data (item 4):**
- Owns: crates/models/src/catalog.rs, catalog/models.json (structure only; the owner writes the content), crates/models/src/hf.rs, src-tauri/src/hub.rs (including the hub.rs:283 switch to `load_settings::ctx_for`), src-tauri/src/downloads.rs (`register_model`), the prefix query in crates/store/src/config.rs, and evals/commands.rs (list profiles).
- Calls WP1's fit API; it does not edit fit.rs.

**WP4b, comparison UI (item 4):**
- Owns: a new models/Compare.tsx, models/Browse.tsx, models/RepoDetail.tsx, a new src/lib/abilities.ts (plain phrases), and reuses FitBadge.tsx.
- The "Add to comparison" control on Installed rows is added at integration, because WP2 owns Installed.tsx.

**WP5, let other apps use this model (item 3 A, if chosen):**
- Owns: a new src-tauri/src/share/ (mod.rs, proxy.rs, setup_text.rs), a new src/components/settings/OtherApps.tsx, and a new crates/runtime/tests/llama_endpoints.rs (contract tests for `/v1/messages`, `/v1/responses` and multi-key auth).
- Reads `RuntimeManager::endpoint()` only, so it does not touch launch.rs.
- Needs the new Settings tab (shared file) and an HTTP server dependency (shared Cargo.toml, deny.toml, notices).

**WP6, the local model asks a coding app for help (item 3 B, if chosen):**
- Owns: a new src-tauri/src/outside_agent.rs (modelled on subagent.rs), the new Scope in crates/policy/src/engine.rs, registration in toolset.rs and tool_catalog.rs, and the toggle in ToolsPanel.tsx.

**WP7, terminal classifier hardening (recommended whatever the item 3 answer):**
- Owns: crates/policy/src/allow_all.rs (`NETWORK_PROGRAMS`, l.119-157) and crates/policy/src/command.rs (bypass flags become keep-asking), with tests.
- Small. It needs the text-rule decision first.

**Order:**
1. WP0.
2. In parallel: WP1, WP3, WP4a, WP5 and WP7. WP2 and WP4b start against the WP0 stubs.
3. Then WP6 if the owner chooses B.
4. The lead integrates the shared files and e2e, and writes the documents.

**Cross-package dependencies:**
- WP5's "needs at least 32k" warning uses WP1's per-model context.
- WP4's fit column uses WP1's cache-type-aware fit.
- WP2 depends entirely on WP1's commands.
