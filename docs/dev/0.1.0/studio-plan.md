# Studio (0.1.0) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Studio, a section where people make pictures and short videos **on their own computer first** (local models run by stable-diffusion.cpp, picked from a recommended list or found on Hugging Face), with cloud models (OpenRouter pictures and video, OpenAI pictures, through the endpoint keys they already saved) as the backup for computers without a good graphics card; a time is shown for local runs and a price, guarded before anything is sent, for cloud runs; every result is kept on their own computer.

**Architecture:** A new `agent::media` module turns OpenRouter's public catalogues and a bundled OpenAI file into typed controls (`ParamSpec`) and prices, and sends each cloud run through the endpoint's own `OpenAiClient`, at most once. A new `runtime::sd` module pins, installs and runs stable-diffusion.cpp's `sd-cli` (one process per job, through the spawn helper), and `models::{gen_catalog, gen_search, gen_fit, gen_eta}` read the signed catalog's picture and video entries, recognise Hugging Face repositories, and say whether a model fits and how long it takes here. A `StudioManager` in `src-tauri/src/studio/` (modelled on `DownloadManager`) runs one local lane (one job at a time, sharing the graphics card with the chat model through `gpu_lease`) beside the cloud queues; it checks the spending rules for cloud runs only, follows video jobs across restarts, downloads results into a visible gallery folder, writes V3 store rows and one audit row per run. The webview renders every control from the `ParamSpec` list, so there is no React code per company or per local model.

**Tech Stack:** Rust (tokio, reqwest 0.13, rusqlite, Tauri 2, async-trait, chrono), React 19 + TypeScript + zustand + Radix, vitest; stable-diffusion.cpp (MIT) as a pinned external program; no new Rust or npm dependency.

**Spec:** docs/dev/0.1.0/studio-design.md (cloud) and docs/dev/0.1.0/studio-local-design.md (local addendum; the owner's four decisions at its top override its body and the cloud design wherever they disagree).

Every commit message ends with the attribution trailer your session requires. Run `cargo fmt --all` before each commit. `crates/agent`, `crates/store` and `crates/runtime` have `#![warn(missing_docs)]`, so every public item there needs a doc comment or clippy (`-D warnings`) fails. A task is done when its own tests pass and `cargo clippy --workspace --all-targets -- -D warnings`, `bash scripts/check-spawn.sh`, `npm run lint` and `npm run typecheck` stay clean.

**Order:** Phase A (shared types, cloud backend) → Phase L (local backend) → Phase B (webview, local first) → Phase C (docs) → Phase D (end to end). **One gate:** Task L17 applies the owner's one-day benchmark (Task L1's checklist, run on the owner's machines) to the catalog and the engine-output fixtures; it lands whenever the results are recorded, after L16; no other task waits for it, the 0.1.0 release does (until then the provisional entries and the hand-written fixtures of Tasks L3 and L7 stand, and the UI shows no time figure). The shared types the local addendum changes (`Company`, `Price`, `MediaProvider`, `FailureKind`, V3, `StartRequest`, `RunDto`, `EstimateDto`, the spending rules, the prefs) are defined with their local parts in the Phase A tasks that introduce them; Phase L adds new modules, changes existing non-Studio code where the local addendum says (packs, downloads, the runtime manager, the catalog, Hugging Face search), and adds to Studio types only what Phase A marks ("Task L15 adds `local: Option<LocalLane>`"). Phase B comes after Phase L so the webview is built local-first against the real DTOs.

---

## Global Constraints

- **Names:** the section is "Studio", its button is "Create"; code never says "generation" for it (`state::Generation` and the store's `generation` mean a chat reply). Rust types use `Creation*`, `Media*`, `Studio*`, and `Gen*` only for picture and video *models* (`GenEntry`, `gen_models`, `gen_fit`). The local group is "On this computer"; the engine is "the Studio engine" on screen.
- **Local first:** Studio leads with **On this computer**. The picker lists local models first; when there is no last-used model, the default is an installed local model whose fit word is "Fits" or "Fits after the chat model is unloaded" (not "Slow here"); the empty state and the one-time first-run suggestion lead with *Get local models*. Cloud is the backup: Studio suggests it (OpenRouter or OpenAI) when no local model fits this computer or a local run would take more than **15 minutes** ("very slow"). Studio never switches a draft between local and cloud by itself.
- **Companies:** This computer (local, stable-diffusion.cpp; pictures and video), OpenRouter (pictures and video) and OpenAI (pictures only; its video API shut down Sep 24, 2026). Anthropic, abliteration.ai and Generic endpoints get nothing in 0.1.0.
- **Hosts:** prompts and added pictures go only to the chosen endpoint's host (`openrouter.ai` or `api.openai.com`); results come from that host, or from the company's file host through the guarded fetcher **without the key**. A local run sends nothing anywhere. Local Studio's only traffic is, each on a click: Hugging Face search and listings and model files (`huggingface.co` and its file hosts, through the existing `HfClient` and download manager, with the existing Hugging Face token for gated repositories) and the Windows engine pack (GitHub release hosts, `packs::ALLOWED_HOSTS`).
- **Keys:** no new key or secret; Studio reads `provider-api-key:<id>` and, for gated Hugging Face repositories, the token the app already stores. A key goes only to its endpoint's own scheme, host and port, never to a result link on another host.
- **No automatic requests:** opening Studio, *Get local models* or Settings → Studio sends nothing. Lists are fetched only on Test, *Get the list*, *Update model lists*/*Update now*, or picking an OpenRouter picture model or pressing *Get the price* (its price); Hugging Face is asked only when the user presses Search (or Enter) or opens one result; a model restored when Studio opens fetches nothing.
- **Confirm before (cloud):** every cloud video, any cloud run estimated at **$1** or more, any cloud run whose price is unknown. The backend refuses with `studio_needs_confirmation` when `confirmed` is false and a rule applies. A local run (`Price::Free`) never asks and is never refused for it.
- **Daily limit (cloud):** **$10**, on by default, per local calendar day, enforced in the backend (`studio_daily_limit`) whatever the webview sends. A run that would take today's spending past it is not sent. An unknown price counts as passing unless at least **$5** remains and the user confirms. Deleting a result never lowers today's spending: a run deleted on the day it was made keeps a spending stub (no prompt, no settings) until the next day. Local runs are recorded at $0 and never count toward "spent today" or meet the limit.
- **Engine (local):** stable-diffusion.cpp build **`master-1106-532d2c1`** (build number **1106**), pinned in `sd.lock.json` and compiled in as `runtime::sd::lock`; changed only together with passing contract tests and engine-output fixtures re-recorded from the new build (the llama.cpp rule, ARCHITECTURE 7.1). **Windows:** the upstream Vulkan+CPU zip (`sd-master-532d2c1-bin-win-vulkan-x64.zip`, about 29 MB, 23 files) is a pack (`studio-vulkan`) downloaded together with the first local model, pinned by size and SHA-256, from GitHub hosts only, resumable, installed with a `.complete` marker into `<local data>/engines/sd-1106/`, **never** into `resources/llama/main` (its `ggml*.dll` share names with llama.cpp's). It keeps every DLL `sd-cli.exe` and `stable-diffusion.dll` import (recorded in the lock at pinning: the ggml DLLs, `libwebp.dll`, `libwebpmux.dll`, `libsharpyuv.dll`, `webm.dll`) and its two licence texts; the Visual C++ runtime, `msvcp140_codecvt_ids.dll` included, is copied from llama's installed folder. The installer does not change beyond that one 20 KB runtime file. **macOS:** `sd-cli` is built from the pinned commit in CI with its `ggml`, `libwebp` and `libwebm` submodules (arm64 + x86_64 joined with `lipo`, static, Metal library embedded, macOS 13.3 minimum), bundled as an `externalBin` beside `llama-server`, signed and notarized with the app, checked by `check-macos-app` and run on the Intel smoke job. Linux runs the engine only for development (`APP_DEV_SD_DIR`) and CI contract tests, with a CPU-only `sd-cli` built from the pinned commit in CI (the upstream Linux zip needs glibc 2.43).
- **One process per job:** each local run starts one `sd-cli` through `runtime::spawn` (Job Object or process group, scrubbed environment plus `GGML_NO_BACKTRACE=1`, the engine folder as working directory, stdin null, `PATH` ending with llama's bundled `vulkan` folder on Windows). No `sd-server` in 0.1.0 (it has no key). **One local job at a time** for the whole computer; cloud runs keep their own queues beside it. Stop kills the engine's whole process tree and deletes `.partial/<run>`; there is no timeout and no restart policy; after **10 minutes** without progress the card says so and offers *Stop* and *Keep waiting*.
- **Engine arguments:** only from typed fields: each part role has one fixed flag; numbers; whitelisted words (sampler, scheduler, `--backend <probed device>` or `cpu`, `--vae-tiling`, `--offload-to-cpu`); `--threads` when it runs on the processor; `-o <gallery>/.partial/<run>/…` (refused when the path holds a `%…d` pattern, which `sd-cli` would treat as a numbering template); `--disable-image-metadata` always (it covers WebM too). The prompt never reaches argv: it is written to `.partial/<run>/prompt.txt` (UTF-8, no trailing newline) and passed with `--prompt-file`, the negative prompt with `--negative-prompt-file` (the pinned CLI has no `--flag=value` form, and a file keeps the prompt out of other programs' sight); local prompts are capped at **4,000 characters** before a run is recorded. Extra fields (JSON) are not offered for local models; quirks come from a compiled-in whitelist (`metal-tensor-off` → `GGML_METAL_TENSOR_DISABLE=1` on macOS only).
- **Chat model and the graphics card:** when a local job fits beside the loaded chat model, both run ("Chat replies may be slower while this runs"). When it does not, Settings → Studio decides: **Unload the chat model** (default) | Ask | Use the processor for local pictures. Unloading waits for a reply being written ("Starts when the reply ends · Cancel"), then unloads through `RuntimeManager::before_change(when_idle = true)` and `unload_by_user()`, and the card says "The chat model was unloaded to make room. [Load it again]". While a local job holds the card a chat load waits ("Studio is using the graphics card. The chat model loads when it finishes · [Stop Studio job]"); while a job is still waiting for the card it holds nothing, so a chat load (a Model check's too) goes first. After the local queue empties the chat model stays unloaded ("Reload it after Studio" is off by default; when on, it reloads once, after the last queued job); the next chat load re-probes free memory first. On a Mac's shared memory "Keep in memory" on the chat model always forces an unload, whatever the policy. A job that does not fit even after an unload is refused unless it is a picture that can run on the processor. Processor only: physical cores minus one (logical cores halved on x86-64), with "chat will be slow while this runs" on the card.
- **Local models:** parts are `.gguf` and `.safetensors` only (`.ckpt`, `.pt`, `.pth` are refused everywhere); every part is pinned by repository, 40-hex revision, size and SHA-256; files land at `<models>/<owner>/<repo>/<path>`, so a shared encoder is downloaded once and deleted only when nothing else uses it. The recommended list is free-for-any-use only: **FLUX.2 [klein] 4B**, **Z-Image Turbo**, **Wan 2.2 TI2V-5B fast** (Apache-2.0, ungated; provisionally FastVideo's FastWan distillation until the benchmark confirms it), plus **Wan 2.1 T2V 1.3B** as a fourth entry for Apple silicon and small cards if the benchmark calls for it. Hugging Face search finds picture and video GGUF repositories (text-to-, image-to- and untagged ones whose names match a known family); a repository of a family harnessUI recognises gets its extra parts (text encoder, decoder) from the known-good family table in the signed catalog and shows the total size, licence and fit before download; **a family it does not recognise (including the full, non-distilled Wan 2.2 5B and FLUX.2 klein 9B) shows "Not supported yet" and has no Download button, and the backend refuses it too**; GGUF diffusion files are identified by their tensor table (most carry no `general.architecture`), after download and before every run, so a file whose name lies is never added or run; region-limited (HunyuanVideo, MiniMax-H3) and uncensored repositories show "Not offered in harnessUI"; repositories tagged `not-for-all-audiences` are hidden; a diffusion file without a published SHA-256 is not offered. A chat model's delete never removes a file a picture or video model uses, and the other way round.
- **Licences:** each part's licence is taken from the family's upstream (never a mirror's label alone), the strictest part decides, and the badge reads green "Free for any use, including commercial" | amber "Commercial use only under $X revenue" | red "Non-commercial" (or "Check the licence" when unclear). A licence's pinned SHA-256 is that of its canonical text bundled in `third_party/licenses/models/` (several upstream repositories state Apache-2.0 only in their README), and its link is the upstream page at a pinned revision. A searched repository gets the stricter of its family's licence and its own label; a label of `other` or one harnessUI does not know reads "Check the licence" (red). Every licence of every part is shown and accepted (recorded per licence SHA-256 in `license_acceptances`) before anything downloads; `studio_local_download` refuses otherwise. Red results stay hidden unless *Show non-commercial models* is on.
- **Content notice:** shown once before the first local download (and again from *Get local models*' footer and Settings → Studio), recorded with its date by `studio_accept_local_notice`; the backend refuses local downloads until it is recorded. A small always-on prompt check refuses prompts that pair sexual words with words for children ("Studio won't make this."), never retried.
- **Local results:** the same cards, gallery, viewer and actions as cloud. The Create button shows a time ("about 4 min") instead of a price; the card and viewer say "free"; no Cloud mark anywhere for a local run; the line above Create reads "On this computer: your prompt and pictures stay here" (plus " · the chat model will be unloaded to make room" when that applies). Every local file is labelled as AI-made in its metadata (XMP `DigitalSourceType = trainedAlgorithmicMedia`, "Made with harnessUI 0.1.0, <model> <revision>", a unique id and the date; XMP chunk for PNG and WebP, Matroska Tags for WebM) and **never carries the prompt**. Local video is WebM (VP8), animated WebP the fallback, never AVI; no MP4.
- **Local defaults:** pictures at the entry's default size (1024×1024 for the starters); video at **480p** (832×480 for 16:9) and **2 s** (49 frames at 24 fps for Wan 2.2, by its 4n+1 rule); 720p offered; video is hidden on a computer without a usable graphics card or Apple-silicon Mac (an Intel Mac counts as processor only). Studio always picks and sends the seed.
- **Time figures:** no local time is shown or promised unless it is measured: a first guess comes only from the catalog's `speed[]` points recorded by the one-day benchmark (owner checklist, Task L1; written into the catalog by Task L17), labelled "(first guess)"; after the first run here Studio uses this computer's own measured times; with neither, the UI says "Time shown after your first run here". No time figure is written in README, CHANGELOG or the website before the benchmark.
- **Charges:** each run records whether it was charged (`yes`, `no`, `unknown`), and spending follows it: "not sent" is never charged; OpenRouter does not bill a picture that failed or was cancelled (its image docs, "Billing and Cancellation"); an OpenAI picture that was stopped, and a video that started, may be charged; once a video job reports done its cost is recorded before the download.
- **Cost colours:** text always; colour is a second cue: under $1 plain, $1–$2.50 amber, over $2.50 red.
- **Never pay twice:** the request that creates a run or job is sent at most once. No retry on a timeout, a 5xx or a dropped connection after sending; one retry only on 429 with `Retry-After` of at most 30 s, or on a connect failure before any byte was sent. Resume, cancel, *Get it anyway* and *Check again* never send a create.
- **Safety refusals:** explained in plain words with the company, the stage when known, whether charged, and the request id; never retried automatically; no "rewrite to get past the filter" helper.
- **Queue:** up to 2 picture runs and 2 video jobs at once per endpoint; more wait, shown as "waiting". One local job at a time; a local run waiting for the graphics card is `queued` with its reason.
- **Timeouts:** connect 10 s; picture request 5 min whole; video checks every 5 s for the first minute, then every 10 s, then every 20 s, each check limited to 30 s; after 30 min of watching the card says "still not ready" with *Check again*; a keyed result download stops after 60 s without data; a download from a company's file host is limited to 10 min. Local runs are checked every 250 ms in memory, with no `PollPlan` and no 30-minute limit.
- **Caps:** picture response streamed to a temporary file, cap 192 MiB; 50 MiB per picture; 1 GiB per video; 20 MiB per added picture; the simple layer's "How many" chips stop at 4; the viewer plays a video from a `blob:` up to 256 MiB and offers *Show in folder* above that.
- **Request shape (OpenRouter, checked against its docs on 2026-11-17):** pictures as `input_references` items `{"type":"image_url","image_url":{"url":"data:…"}}`; video frames as `frame_images` items with `frame_type`; routing in `provider` (`only`/`order` lists of `provider_tag`, `allow_fallbacks`, `zdr`); extra settings under `provider.options[<provider_slug>]`, never at the top level. Video extra settings are not sent in 0.1.0 (the catalogue gives no company slug to key them by).
- **Events:** `studio-progress` (payload `RunDto`) to the main window, at most 4 per second per run; state is also readable through `studio_runs` and `studio_live`, because events are not replayed. `studio-engine-progress` (the Windows engine pack, at most every 300 ms) and `studio-quit-requested` (closing the main window, or quitting from the app menu or ⌘Q, while a local job runs). Installing an update is refused while a local job runs ("Stop it in Studio, or install the update when it finishes.").
- **Files:** gallery default `<home>/harnessUI/studio`; `<gallery>/YYYY-MM/<run_id>-<n>.<ext>`, sidecar `<gallery>/YYYY-MM/<run_id>.json` (never the key), `<gallery>/refs/<sha256>.<ext>`, `<gallery>/.thumbs/<output_id>.jpg` (512 px JPEG), `<gallery>/.partial/` for downloads and local runs in progress. Allowed types: PNG, JPEG, WebP, GIF, SVG, MP4, WebM. Cloud files are kept exactly as the company sent them; local files get the AI label above. Local model parts live in the models folder (`<models>/<owner>/<repo>/<path>`), the Windows engine in `<local data>/engines/sd-1106/`.
- **Database:** migration V3 appended to `MIGRATIONS`; V1 and V2 are never edited; at most 20,000 runs (spending stubs not counted). V3 also holds the local columns on `creation_runs`, `gen_models` and `license_acceptances` (Task A8); installed picture and video models never go into the `models` table, which feeds the chat picker.
- **Esc never cancels a Studio job.** In Studio, Esc only closes the viewer or a popup. The global stop hotkey stops chat turns and tools only.
- **CSP:** add `media-src blob:` to `src-tauri/tauri.conf.json` and to the pinned CSP test in `src-tauri/src/commands.rs`; video plays from `blob:` URLs made from `studio_output_bytes`.
- **Audit:** one row per run: server `studio`, tool `picture` or `video`, no chat, decision `allowed_without_prompt` decided by `user`, the prompt's length but never the prompt, completed when the run ends (also after a restart, through `audit_id`). A completed row is never changed (`complete_audit` is write-once); *Get it anyway* on an ended run appends a follow-up row with `parent_audit_id` set and completes that one. A local run's row says `"runsOn": "local"` and carries the catalog id, revision and engine build.
- **Preferences:** Settings → Studio changes apply at once: the manager holds the current prefs, new runs read them, and `studio_set_prefs` takes only the fields that changed.
- **Commands:** each new command is listed in `src-tauri/commands.list`, granted as `allow-<cmd>` in `src-tauri/capabilities/main.json` (main window only) and registered in `generate_handler!` in `src-tauri/src/lib.rs`; each checks `commands::only_main_window`.
- **Navigation:** sidebar button between Models and Settings (sparkle icon), shortcut Ctrl/⌘ G, window event `app:open-studio`.
- **Defaults (cloud):** video length = the shortest of at least 4 s, resolution = 720p (or the lowest offered), sound as the company defaults; Studio picks and sends a random seed whenever the model takes one.
- **Chat link:** *Send to chat* for pictures only, through the composer's 20 MB and 8-picture limits; *Open in Studio* on a chat picture. No chat tool in 0.1.0.
- **Copy:** plain English, no raw identifiers on screen, all webview wording in `src/lib/words.ts`, backend wording in `src-tauri/src/studio/errors.rs`. Examples to keep verbatim: "Daily limit reached: $X of $10 spent today", "Cloud: your prompt and added pictures go to openrouter.ai", "price unknown", "Make this video?", "On this computer: your prompt and pictures stay here", "free · private · nothing is sent", "No local models yet.", "Not supported yet", "Stopped. Your computer is no longer working on it. Nothing was charged.", "The chat model was unloaded to make room.", "Load it again", "Studio won't make this.", and the content notice in addendum L1 word for word.
- **Not in 0.1.0:** chat picture tool, fal.ai, mask painting, video extend, side-by-side models, styles, system notifications; for local: the CUDA and ROCm engine packs, `sd-server` mode and live previews, LTX-2.3, Wan 2.2 A14B, Qwen-Image 2512, FLUX.3 klein, LoRAs, upscaling, C2PA signing and invisible watermarks, keep-awake, MP4 export, a Swift/MLX Mac engine.

## Review Focus

1. **App closed mid-video, then reopened:** the video is checked and downloaded exactly once by the new manager with no second create, `sending` becomes `interrupted` ("may or may not have started"), `queued` is never sent. Pinned in A14 (`resume_after_restart_finishes_the_video_without_a_second_create` with `shutdown()`, one output row, one content download; `a_run_left_sending_becomes_interrupted_and_is_never_sent_again`; `a_queued_run_is_not_sent_after_a_restart`) and D1.
2. **The result cannot be saved** (link expired, disk full while streaming, over its cap): the failure is caught where bytes are written (`MediaError::Local`), the partial file goes, the run keeps its remote id and recorded cost, *Get it anyway* fetches again with no create, and the audit gets a linked follow-up row instead of a store error. Pinned in A5 (`content_gone_after_done_is_expired`, `a_full_disk_while_downloading_is_named`, `a_download_over_the_cap_removes_the_partial_file`) and A14 (`an_expired_result_says_so_and_keeps_the_remote_id`, `a_full_disk_keeps_the_remote_id_for_get_it_anyway`, `stop_watching_then_get_it_anyway_records_done_with_the_cost`).
3. **Money at the edges:** the day boundary, deleting results (never lowers spending), unknown prices (token-priced pictures, OpenAI per-picture) against the $10 limit, a picture stopped while sending, a run that was never sent. Pinned in A3 (`a_run_counts_on_the_day_it_was_created`, `reaching_the_limit_exactly_is_allowed`, `an_unknown_price_needs_five_dollars_left`), A13 (`three_openai_runs_in_a_row_stay_within_the_limit`, `an_openai_picture_cancelled_while_sending_may_be_charged_and_counts`, `a_run_that_was_not_sent_counts_nothing`) and A16 (`deleting_todays_results_does_not_lower_spent_today`).
4. **The request OpenRouter receives** differs from what Studio assumed (reference, frame and routing shapes, where extra settings go): the builders are checked against the request examples recorded from OpenRouter's docs, and the owner's real run saves the accepted bodies over the fixtures. Pinned in A4 (`the_picture_body_matches_openrouters_documented_shape`, `extra_settings_go_under_provider_options_for_each_company_that_lists_them`), A5 (`the_video_body_matches_openrouters_documented_shape`) and D2 (owner checklist).
5. **Nothing without a click, and one click is one run:** opening Studio with a restored OpenRouter picture model sends no request; a double click, Enter plus click, or *Again* pressed twice starts one run; two starts at once cannot both pass the limit; a refused unconfirmed press followed by the confirmed one makes one run. Pinned in B3 (`a restored model fetches no price when Studio opens`), B7 (`Create is disabled while a start is on its way`), B9 (`Again pressed twice while starting starts once`), A13 (`the_same_create_press_twice_makes_one_run`, `two_starts_at_once_cannot_both_pass_the_limit`, `a_refused_unconfirmed_press_then_the_confirmed_one_makes_one_run`) and D1 (`opening_studio_makes_no_request`).

6. **Out of graphics memory in the middle of a local run:** the engine dies with an out-of-memory message after minutes of work. Studio retries once by itself with memory saving (`--vae-tiling --offload-to-cpu`) and says so on the card; a second failure is named with "Try 480p or a shorter clip"; the half-made files go after every final failure, not only after Stop; nothing is charged; the chat model stays unloaded and the graphics card stays held across the retry. Pinned in L3 (`failures_are_named`, `memory_failures_in_this_builds_words_are_named`), L5 (`out_of_memory_retries_once_with_memory_saving`, `a_second_out_of_memory_is_named`, `a_failed_run_removes_its_partial_folder`), L14 (`out_of_memory_twice_is_a_named_failure_not_charged`), L15 (`the_card_stays_held_through_the_out_of_memory_retry`) and B8 (`an out-of-memory retry and failure are said plainly`).
7. **The chat model's graphics memory:** a local job that does not fit beside the loaded chat model never runs on top of it: it waits for a reply being written, unloads the chat model (or asks, or uses the processor, as Settings → Studio says), and names the unload with *Load it again*; a chat load during the job waits instead of making llama-server run out of memory and restart in a loop, but a chat load while a job only waits for the card goes first (no deadlock with a Model check); "Keep in memory" on a Mac always unloads; what the lease and the fit decide (processor only, memory saving, the matched device) reaches the engine's arguments; a job judged "Fits after the chat model is unloaded" can be created. Pinned in L8 (`the_word_after_an_unload_comes_before_red_and_slow`), L12 (`a_job_that_does_not_fit_waits_for_the_reply_then_unloads`, `keep_in_memory_always_unloads_on_shared_memory`, `a_chat_load_waits_while_studio_holds_the_card`, `a_chat_load_while_a_job_waits_for_the_card_goes_first`, `the_load_path_asks_the_studio_gate_first`), L15 (`the_unloaded_chat_model_is_named_on_the_card`, `the_grant_and_the_fit_reach_the_engine_job`, `the_chat_model_reloads_once_when_the_queue_empties`) and B8.
8. **A partly downloaded multi-file model:** the app closes or the network drops after two of three parts (for example the 2.5 GB encoder is done and the 3.9 GB model is not). The model is never listed or offered until every part is verified; Resume fetches only what is missing, at each part's own revision; a part already on disk from another model is checked, not fetched again; deleting one model keeps a part another still uses, and deleting a chat model keeps a file a picture model uses (Klein's encoder is a common chat download). Pinned in L11 (`generator_parts_download_at_their_own_revisions`, `a_generator_group_registers_only_when_every_part_is_verified`, `a_part_already_on_disk_is_checked_not_fetched`, `deleting_a_model_keeps_a_shared_encoder`, `deleting_a_chat_model_keeps_a_file_a_picture_model_uses`) and L9 (`file_uses_counts_both_tables`).
9. **The engine blocked by antivirus, SmartScreen or Gatekeeper:** the Windows pack's `sd-cli.exe` is quarantined after install, or the program cannot start (access denied). Studio never reports a pack with a missing program as installed, names the failure in plain words ("The Studio engine is missing or was blocked. Download it again, or allow it in your antivirus."), and offers the download again (Settings → Studio, *Get local models*' footer and the failed card); a model with a damaged part offers *Download again*; on macOS the engine is bundled, signed and notarized with the app, and `check-macos-app` fails a build whose `sd-cli` links anything outside the system. Pinned in L1 (`test_the_app_check_needs_sd_cli`), L2 (`a_pack_whose_program_was_removed_is_not_found`), L5 (`an_engine_that_cannot_start_is_named`), L14 (`no_engine_says_engine_missing`), B8 (`a missing engine or a damaged part offers the download again`), B10 (`a missing engine on Windows offers its download`) and B13 (`the footer offers the engine again when it went missing`).
10. **An unsupported family picked from search:** a popular repository of a family harnessUI does not recognise, the full (non-distilled) Wan 2.2 5B, a FLUX.2 klein 9B quant named like the 4B, or a file whose name lies, shows "Not supported yet" with no Download, or is refused when its tensor table is read; the backend refuses the same source if the webview sends it anyway; a downloaded file is identified by its tensor table before it is added, and again before every run. Pinned in L7 (`a_9b_klein_is_not_supported_yet`, `the_full_wan_2_2_is_not_the_fast_one`, `families_are_told_apart_by_their_tensor_tables`, `an_unknown_family_says_not_supported_yet`, `an_uncensored_or_region_limited_repo_is_not_offered`), L11 (`an_unsupported_search_source_is_refused_by_the_backend`, `a_searched_file_that_is_another_model_is_not_added`), L14 (`a_file_of_another_family_is_refused_before_the_engine_starts`) and B13 (`an unknown family says Not supported yet and offers no Download`).
11. **The engine runs the arguments Studio builds, on a clean computer:** the pinned `sd-cli` accepts every flag and form Studio sends (the prompt through `--prompt-file`, never `--prompt=`), finds every DLL it imports in the pack folder and the Visual C++ runtime beside it, and starts on Intel Macs and on CI's Linux. Pinned in L1 (`the_engine_keeps_every_dll_it_imports`, `the_vc_runtime_covers_every_msvc_import`, `test_the_mac_build_fetches_its_three_submodules`, the Windows CI step and the Intel smoke), L2 (`install_files_keeps_what_the_pinned_zip_needs`) and L4 (`the_pinned_engine_parses_the_exact_arguments_studio_builds`).

Also covered where they belong: switching to a model with incompatible settings (B4 notes; A1 `validate_refuses_a_field_the_model_does_not_list`; A13 refuses before sending), safety refusals (A4, A7, A13, B8), the overlay window (every Studio command checks `only_main_window`), quitting while a local job runs, by the window or by ⌘Q (L15, B8), a full disk before a local download (L11), a local prompt over 4,000 characters (L15, B7), and a stalled engine (L5, B8).

---

## File Structure

**Rust, `crates/agent`**

| File | Responsibility |
|---|---|
| `src/media/mod.rs` (new) | Media types (`MediaKind`, `Company` with `Local`, `MediaModel`, `MediaRequest`, `Started`, `JobState`, `MediaFailure` and the local `FailureKind`s, `MediaError`, `LocalIo`, `MediaLimits`, `EndpointCompany`), the `MediaProvider` trait (with a default `cancel`), `media_provider()`, re-export of `cloud::media_company` |
| `src/media/spec.rs` (new) | `ParamSpec` and its kinds; catalogue → `MediaModel` (local models pass through); `validate` |
| `src/media/price.rs` (new) | `Price` (with `Free`), `pricing_skus` and `/endpoints` parsing, `estimate`, OpenAI cost from usage |
| `src/media/openrouter.rs` (new) | `OpenRouterMedia`: catalogues, picture runs, video jobs, failure mapping |
| `src/media/openai.rs` (new) | `OpenAiImages`: bundled spec matching, generations, edits, failure mapping |
| `src/media/multipart.rs` (new) | Hand-built `multipart/form-data` body for OpenAI edits (reqwest's `multipart` feature would add `mime_guess`) |
| `src/media/specs/openai_images.json` (new) | Dated OpenAI settings, retire dates and prices |
| `src/client.rs` | `pub(crate)` helpers; `post_once`, `download_to_file`, `get_json_within`, the shared capped writer `stream_body_to` |
| `src/cloud.rs` | `media_company`, `openai_media_kind`, `scrub_text` (`pub(crate)`) |
| `src/lib.rs` | `pub mod media` and re-exports |
| `tests/media_endpoints.rs` (new) | Both adapters against `tests/common` `MockServer` |
| `tests/media_redirects.rs` (new) | Single-test binary: a non-loopback redirect to another origin carries no key |
| `tests/fixtures/media/` (new) | Copies of `docs/dev/0.1.0/research/catalog/` (catalogues, `/endpoints` answers and OpenRouter's documented request examples, all recorded 2026-11-17); OpenAI sample answers |

**Rust, other crates**

| File | Responsibility |
|---|---|
| `crates/search/src/fetch.rs` | `Fetcher::fetch_to_file`: binary, streamed, capped, same address rules, never a key |
| `crates/search/tests/fetch.rs` | Its tests (test hooks) |
| `crates/store/src/schema.rs` | V3 (cloud and local columns, `gen_models`, `license_acceptances`) |
| `crates/store/src/creations.rs` (new) | `creation_runs` / `creation_outputs` accessors, spending stubs, `local_run_times` |
| `crates/store/src/gen_models.rs` (new) | Installed picture and video models, file use counting across `gen_models` and `models`, licence acceptances |
| `crates/store/src/ids.rs` | `update_columns_by_text` (the text-key sibling of `update_columns`) |
| `crates/store/src/lib.rs`, `error.rs` | Re-exports; `StoreError::TooManyCreations` |
| `crates/runtime/src/sd/mod.rs` (new) | The Studio engine module root and re-exports |
| `crates/runtime/src/sd/lock.rs` (new) | `SdLock` from `sd.lock.json` (compiled in): pin, submodules, Windows pack and its recorded DLL imports, macOS source build, Linux dev source build |
| `crates/runtime/src/sd/engine.rs` (new) | `Engine`, `EngineLayout`, `discover` (dev, bundled, pack), `install`, `remove`, `pack_state` |
| `crates/runtime/src/sd/progress.rs` (new) | `LocalStage`, `LocalProgress`, `ProgressParser`, `overall` |
| `crates/runtime/src/sd/failure.rs` (new) | `SdFailure`, `classify` |
| `crates/runtime/src/sd/args.rs` (new) | `SdJob` → argv from typed fields only (the prompt by file); `FLAGS`; `PROMPT_MAX_CHARS`; `processor_threads` |
| `crates/runtime/src/sd/devices.rs` (new) | `--list-devices` parsing (name TAB description), matching llama's device by description |
| `crates/runtime/src/sd/run.rs` (new) | `run_job` (prompt files, spawn, progress, Stop, one out-of-memory retry, stall, the partial folder gone on every failure), `list_devices` |
| `crates/runtime/src/sd/testing.rs` (new, `#[doc(hidden)] pub`) | The fake `sd-cli` used by tests here and in `src-tauri` (precedent: `models::gguf::testing`) |
| `crates/runtime/src/packs.rs` | `InstallSpec` and `install_files`, shared by `install` (llama packs) and `sd::engine::install` |
| `crates/runtime/src/llama/lines.rs` | `LineReader::splitting_cr` (`\r` ends a line too) |
| `crates/runtime/tests/sd_cli.rs` (new, `harness = false`) | The runner against the fake engine |
| `crates/runtime/tests/sd_contract.rs` (new) | The pinned `sd-cli` built in CI: flag names from `-h`, the exact argv parsed in metadata mode, `--list-devices` (skipped without `SD_CLI_BIN`) |
| `crates/runtime/tests/fixtures/sd_*.txt` (new) | Engine output: progress (with `\r` and `ESC[K`), failures, devices, help; the pinned Windows zip's file listing; `sd_fixtures_build.txt` (the build they came from) |
| `crates/models/src/catalog.rs` | `Catalog` keeps `generators`, `generatorTiers`, `generatorFamilies` as raw JSON (old parsers ignore them) |
| `crates/models/src/gen_catalog.rs` (new) | `GenEntry`, `GenPart`, `GenLicence`, `FamilyRecord`, `GenCatalog`: lenient parsing, hidden entries, strictest licence, licence hashes checked against the bundled texts |
| `crates/models/src/gen_search.rs` (new) | Recognising a Hugging Face repository's family (names, distilled markers, parameter band) and a local file's family (its tensor table); the entry for a searched file; its licence |
| `crates/models/src/gen_fit.rs`, `gen_eta.rs` (new) | Fit verdicts for picture and video models; the fit word and its order; time estimates and rounding |
| `crates/models/src/hf.rs`, `arch.rs`, `fit.rs`, `gguf.rs` | `search_generators` (merged queries); `is_diffusion`; `Target::room` public; `TensorShape`, `read_tensor_shapes`, `testing::Writer::shaped_tensor` |
| `third_party/licenses/models/apache-2.0.txt` (new) | The canonical licence text whose SHA-256 the catalog pins for Apache-2.0 models (compiled into `models`) |

**Rust, `src-tauri`**

| File | Responsibility |
|---|---|
| `src/studio/mod.rs` (new) | Module root |
| `src/studio/spend.rs` (new) | Confirmation rules, daily limit, day boundary (pure) |
| `src/studio/gallery.rs` (new) | Folder layout, type check and rename, sidecar, references, default folder |
| `src/studio/catalog.rs` (new) | `providerMediaModels:<id>` cache, refresh, `models_for` (no network) |
| `src/studio/prefs.rs` (new) | `StudioPrefs` (`studioPrefs` setting), defaults, validation |
| `src/studio/deps.rs` (new) | `ProviderSource`, `StudioSink`, `ForeignFetch`, `Clock` traits and their app implementations |
| `src/studio/manager.rs` (new) | `StudioManager`: start, queue, run tasks, video jobs, resume, cancel, audit, events |
| `src/studio/manager_tests.rs` (new) | Manager tests with fakes |
| `src/studio/test_support.rs` (new, `#[cfg(test)]`) | `Scratch` folder (temp dir + uuid, removed on Drop, as in `downloads.rs`), PNG/MP4 samples |
| `src/studio/commands.rs` (new) | Tauri commands (thin: each calls a tested function) |
| `src/studio/errors.rs` (new) | Studio `AppError`s and their copy |
| `src/studio/wire_tests.rs` (new) | Phase D: a whole run against a local OpenRouter mock, and a local run against the fake engine |
| `src/studio/engine_cmd.rs` (new) | The Studio engine's status, download, cancel and remove; `engine_layout`, `current_engine` |
| `src/studio/local_models.rs` (new) | `LocalSource`, download plans, licence and notice checks, starting and deleting local model downloads |
| `src/studio/label.rs`, `prompt_check.rs` (new) | The AI label written into local files; the always-on prompt check |
| `src/studio/local.rs` (new) | `LocalMediaProvider` behind `MediaProvider` and `LocalProvider`: installed models as `MediaModel`s, the exact command, runs through `runtime::sd` with the lane's `LocalJobOptions` |
| `src/studio/local_cmd.rs` (new) | Get local models: lists, search, a repository's parts, fit, download (and *Download again*), delete, the notice |
| `src/studio/quit.rs` (new) | `ask_before_quit`: the one decision behind the window's close and the app's exit request |
| `src/gpu_lease.rs` (new) | One graphics-card tenant at a time: the chat model or a local job; the chat model's reload when asked |
| `src/runtime_manager.rs` | `device_bytes`, `keeps_in_memory`, the Studio gate on the load path, `waiting_for_studio` |
| `src/downloads.rs` | `GroupMeta.generator`, per-part revisions, `register_gen_model` (tensor check first), the group's picture/video label |
| `src/hub.rs` | `detect`, `current_catalog` `pub(crate)`; importing a picture or video GGUF says "Add it in Studio"; deleting a chat model keeps files picture and video models use |
| `src/updates.rs` | Installing an update is refused while a local job runs |
| `src/dto.rs` | Studio DTOs (cloud and local) |
| `src/providers.rs` | Test caches media lists; delete forgets them; `may_hold_key` `pub(crate)` |
| `src/attachments.rs`, `src/main.rs` | `sniff`/`checked_picture` `pub(crate)`; `ImageInput` also `Serialize`; the picture worker's thumbnail mode |
| `src/artifacts.rs` | `export_target` takes an extension, `pub(crate)` |
| `src/lib.rs`, `commands.list`, `capabilities/main.json` | Registration, `manage`, resume at start, the quit question while a local job runs |
| `tauri.conf.json`, `src/commands.rs` | `media-src blob:`; version test |
| `tauri.macos.conf.json` | `externalBin` adds `binaries/sd-cli` |

**Webview**

| File | Responsibility |
|---|---|
| `src/components/studio/types.ts` (new) | TS mirrors of the DTOs |
| `src/components/studio/api.ts` (new) | Typed `invoke` wrappers and `onStudioProgress` |
| `src/components/studio/fixtures/models.ts` (new, generated) | Six models and one run as the backend serialises them, written by a Rust test as typed `MediaModel[]` / `Run` literals, so a serde/TS mismatch fails `npm run typecheck` |
| `src/store/studio.ts` (new) | zustand store: view, endpoints, drafts, runs, live progress, prefs, usage |
| `src/components/studio/canonical.ts` (new) | Defaults, carrying values between models, the change note, model families |
| `src/components/studio/cost.ts` (new) | Formatting and colour of prices |
| `src/components/studio/StudioView.tsx` (new) | Shell: top bar, tabs, local-first empty states and first-run suggestion, layout |
| `src/components/studio/ModelPicker.tsx` (new) | Grouped, searchable picker: "On this computer" first (fit word, time), then cloud with the Cloud mark and price hint |
| `src/components/studio/LocalMark.tsx` (new) | "On this computer" in place of the Cloud mark |
| `src/components/studio/LocalModels.tsx`, `LocalModelRow.tsx`, `LocalSearch.tsx` (new) | Get local models: header, Pictures/Video, recommended list, Not for this computer, Hugging Face search, footer |
| `src/components/studio/DownloadLocalModel.tsx`, `LocalNotice.tsx` (new) | The licence confirmation before a download; the one-time content notice |
| `src/components/studio/QuitWhileMaking.tsx` (new) | "A video is being made on this computer. Quitting stops it." |
| `src/components/FitBadge.tsx` | `FitDot` (dot, word, line) shared by chat models and local models |
| `src/components/RuntimeIndicator.tsx`, `src/lib/types.ts` | "Studio is using the graphics card…" with *Stop Studio job* |
| `src/components/models/Installed.tsx`, `Downloads.tsx` | "Picture and video models: N (X GB) · Manage in Studio"; picture/video labels on download groups |
| `src/components/studio/ParamField.tsx` (new) | The only widget renderer, driven by `ParamSpec` |
| `src/components/studio/SettingsPanel.tsx` (new) | Right panel: simple layer, My setups, Advanced toggle |
| `src/components/studio/AdvancedDrawer.tsx` (new) | Advanced fields, Extra fields (JSON), routing, request preview |
| `src/components/studio/RefTray.tsx` (new) | Added pictures with role chips |
| `src/components/studio/PromptDock.tsx` (new) | Prompt, chips, Cloud line, Create with price |
| `src/components/studio/ConfirmRun.tsx` (new) | The confirm dialog |
| `src/components/studio/RunCard.tsx` (new) | One run: progress, results, errors, actions |
| `src/components/studio/Grid.tsx`, `Viewer.tsx` (new) | Gallery grid; full-size viewer with keys |
| `src/components/studio/StudioNotice.tsx` (new) | "Your video is ready" while in chat |
| `src/components/settings/StudioSettings.tsx` (new) | Settings → Studio |
| `src/App.tsx`, `Sidebar.tsx`, `ShortcutsDialog.tsx`, `SettingsDialog.tsx` | View switch, button and badge, shortcut rows, the new tab |
| `src/components/settings/endpointPresets.ts`, `Endpoints.tsx` | `media` flags and "Chat · Pictures · Video" |
| `src/components/Composer.tsx`, `MessageItem.tsx` | `app:compose-images`; *Open in Studio* |
| `src/lib/words.ts` | All Studio copy |

**Engine pin, build and CI:** `sd.lock.json` (new), `scripts/fetch_sd.py` and `scripts/fetch_sd_test.py` (new; `macos`, `linux`, `windows-check`, `verify-packs`, sharing `fetch_llama.py`'s helpers), `scripts/fetch_llama.py` and its test (`macos_plan(lock, pin_key)`, `check_macos_app` needs `sd-cli`, `verify_packs` covers both locks), `llama.lock.json` (`msvcp140_codecvt_ids.dll` joins `vc_runtime_files`), `scripts/intel-mac-smoke.sh` (runs the bundled `sd-cli`), `.github/workflows/ci.yml` and `release.yml` (macOS builds `sd-cli`; the Windows build job installs and starts the pinned pack; cache keys add `sd.lock.json` and `scripts/fetch_sd.py`; the `checks` job builds the dev `sd-cli` for the contract step), `catalog/models.json` (`generators`, `generatorTiers`, `generatorFamilies`; sequence 3), `scripts/notices.py`, `THIRD-PARTY-NOTICES.md`, `third_party/licenses/` (stable-diffusion.cpp and what it vendors).

**Docs, scripts, e2e:** `README.md`, `CHANGELOG.md`, `ARCHITECTURE.md` (7.1, 7.5, new 7.9, 12, 13, 13.2, 13.6, 13.7, 14, 16, 17), `SECURITY.md`, `docs/SPEC.md`, `docs/dev/0.1.0/README.md`, `docs/dev/0.1.0/owner-checklist.md` (new in Task L1), `scripts/docs_test.py`, `e2e/m8_offline.py`, `e2e/m12_studio.py` (new), `e2e/fake_sd/sd-cli` (new, a fake engine for the by-hand e2e runs); version in `Cargo.toml`, `Cargo.lock`, `package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`.

---

## Phase A: Backend

### Task A1: Media types and controls from OpenRouter's catalogues

**Files:**
- Create: `crates/agent/src/media/mod.rs`, `crates/agent/src/media/spec.rs`, `crates/agent/src/media/price.rs` (types only here)
- Create: `crates/agent/tests/fixtures/media/` with copies of the four files in `docs/dev/0.1.0/research/catalog/`: `openrouter-images-2026-11-17.json`, `openrouter-videos-2026-11-17.json`, `openrouter-image-endpoints-2026-11-17.json` (the public `GET /api/v1/images/models/{id}/endpoints` answers for gpt-image-2.6-flare, gemini-nano-banana-2.1, flux-3-image and seedream-5-5, recorded unchanged under `answers[id]`), `openrouter-request-examples-2026-11-17.json` (the example bodies on OpenRouter's image and video docs pages)
- Modify: `crates/agent/src/lib.rs` (`pub mod media;`), `crates/agent/src/cloud.rs` (`media_company`)
- Test: inline `#[cfg(test)] mod tests` in `spec.rs`

**Interfaces:**
- Consumes: `agent::ProviderKind`, `agent::ProviderError`.
- Produces (`agent::media`):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)] #[serde(rename_all = "lowercase")]
pub enum MediaKind { Image, Video }                // Ord: Task A12 keys a BTreeMap by it
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "lowercase")]
pub enum Company { OpenRouter, OpenAi, Local }     // "openrouter" | "openai" | "local" (this computer, Task L14)
// cloud.rs: pub fn media_company(kind: ProviderKind) -> Option<Company>;   mod.rs: pub use crate::cloud::media_company;

// spec.rs (serde camelCase; mirrored by src/components/studio/types.ts)
pub struct ParamSpec { pub key: String, pub canonical: Option<Canonical>, pub label: String, pub help: Option<String>,
    pub layer: Layer, pub kind: ParamKind, pub company_default: Option<Value>, pub initial: Option<Value>,
    pub affects_price: bool, pub requires: Option<Requires> }
pub enum Canonical { Shape, Count, Quality, Resolution, Length, Sound, Seed, References, StartFrame, EndFrame, Size, Format, Background }
pub enum Layer { Simple, Advanced }
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]   // fields too: Extra → "allowedKeys"
pub enum ParamKind { Enum { values: Vec<Choice> }, Range { min: f64, max: f64, step: Option<f64>, unit: Option<String> },
    Boolean, Seed { max: u64 }, Images { min: u32, max: u32, role: RefRole }, Size { rules: SizeRules }, Extra { allowed_keys: Vec<String> } }
pub struct EndpointCompany { pub slug: String, pub tag: String, pub name: String, pub passthrough: Vec<String> }  // one `/endpoints` record
pub struct Choice { pub value: Value, pub label: String }          // value: string or number, sent as is
pub struct Requires { pub key: String, pub values: Vec<Value> }
pub struct SizeRules { pub multiple_of: u32, pub min_ratio: f64, pub max_ratio: f64, pub max_side: u32, pub min_pixels: u64, pub max_pixels: u64 }
pub const OPENAI_SIZE_RULES: SizeRules;                           // 16, 1/3, 3, 3840, 655_360, 8_294_400
pub const SEED_MAX: u64 = 2_147_483_647;

// mod.rs
pub struct MediaModel { pub id: String, pub display_name: String, pub family: String, pub kind: MediaKind,
    pub accepts_images: bool, pub params: Vec<ParamSpec>, pub price: Price, pub notes: Vec<String>,
    pub usable: Option<String>, pub created: Option<i64>,
    pub companies: Vec<EndpointCompany> }                      // from the saved `/endpoints` answer (OpenRouter pictures), else empty
#[serde(rename_all = "camelCase")] pub enum RefRole { Reference, StartFrame, EndFrame }
pub struct RefImage { pub role: RefRole, pub mime: String, pub path: PathBuf, pub sha256: String, pub size_bytes: u64 }
pub struct Routing { pub order: Vec<String>, pub only: bool, pub allow_fallbacks: bool, pub zdr: bool }   // order: provider_tag values
pub struct MediaRequest { pub kind: MediaKind, pub model: String, pub prompt: String,
    pub params: Map<String, Value>, pub extra: Map<String, Value>, pub refs: Vec<RefImage>, pub routing: Option<Routing>,
    pub provider_options: BTreeMap<String, Map<String, Value>> }   // extra settings by provider_slug (OpenRouter pictures, Task A4)
pub struct RawCatalog { pub images: Value, pub videos: Option<Value> }
pub struct Saved { pub path: PathBuf, pub mime: String, pub size_bytes: u64, pub width: Option<u32>, pub height: Option<u32>, pub duration_ms: Option<u64> }
pub struct Cost { pub usd: f64, pub reported: bool }
pub enum Started { Done { outputs: Vec<Saved>, cost: Option<Cost> }, Job { remote_id: String } }
pub struct JobDone { pub outputs: usize, pub signed: Vec<SignedUrl>, pub cost: Option<Cost> }
pub struct SignedUrl { pub url: Url, pub expires_at: Option<i64> }
pub enum JobState { Pending, Running { progress: Option<f32> }, Done(JobDone), Failed(MediaFailure), Gone }
pub enum Fetched { Saved(Saved), Elsewhere(Url) }
pub struct RequestPreview { pub method: String, pub url: String, pub host: String, pub body: Value }
pub enum Stage { Prompt, Output }
pub enum FailureKind { Blocked, Unauthorized, NoCredit, OrgVerification, RateLimited, BadRequest, NotFound, Expired, Server, Other,
    OutOfMemory, EngineMissing, EngineFailed, ModelFileBad, DeviceLost }   // the last five: local runs only, always `charged: Some(false)`
pub struct MediaFailure { pub kind: FailureKind, pub stage: Option<Stage>, pub charged: Option<bool>, pub company_code: Option<String>,
    pub request_id: Option<String>, pub status: Option<u16>, pub message: String, pub retry_after: Option<Duration> }
pub enum LocalIo { DiskFull, TooLarge(u64), Io(String) }          // failures writing bytes on this computer
pub enum MediaError { NotSent(ProviderError), Answered(MediaFailure), Unknown(ProviderError), Local(LocalIo) }
pub struct MediaLimits { pub response_cap: u64, pub picture_cap: u64, pub video_cap: u64, pub picture_timeout: Duration,
    pub check_timeout: Duration, pub download_idle: Duration }
impl Default for MediaLimits  // 192 MiB, 50 MiB, 1 GiB, 300 s, 30 s, 60 s
#[async_trait] pub trait MediaProvider: Send + Sync {
    fn company(&self) -> Company;
    fn host(&self) -> String;
    async fn fetch_catalog(&self) -> Result<RawCatalog, MediaError>;
    async fn fetch_image_price(&self, model: &str) -> Result<Value, MediaError>;   // Value::Null for OpenAI
    fn preview(&self, req: &MediaRequest) -> RequestPreview;
    /// Sent at most once. Pictures come back done; a video comes back as a job.
    async fn start(&self, req: &MediaRequest, scratch: &Path) -> Result<Started, MediaError>;
    async fn check(&self, remote_id: &str) -> Result<JobState, MediaError>;
    async fn download(&self, remote_id: &str, job: &JobDone, index: usize, dest: &Path) -> Result<Fetched, MediaError>;
    /// Stops a started run. Cloud keeps this no-op (closing the connection is all it can do);
    /// the local provider kills the engine (Task L14).
    async fn cancel(&self, _remote_id: &str) -> Result<(), MediaError> { Ok(()) }
}
// spec.rs (`image_prices`: saved `/endpoints` answers by model id; they give picture prices, companies and extra-setting names)
// Company::Local: `raw.images` is the JSON array of `MediaModel`s the local provider built (Task L14); read as is, `[]` when unreadable.
pub fn parse_catalog(company: Company, raw: &RawCatalog, image_prices: &BTreeMap<String, Value>) -> Vec<MediaModel>;
pub fn openrouter_image_models(list: &Value, image_prices: &BTreeMap<String, Value>) -> Vec<MediaModel>;
pub fn endpoint_companies(answer: &Value) -> Vec<EndpointCompany>;   // one per `/endpoints` record, in answer order
pub fn openrouter_video_models(list: &Value) -> Vec<MediaModel>;
pub fn family_of(id: &str) -> String;
pub enum Invalid { NotOffered(String), BadValue(String), Requires { key: String, needs: String }, TooManyPictures { role: RefRole, max: u32 },
    TooFewPictures { role: RefRole, min: u32 }, Reserved(String), EmptyPrompt, PromptTooLong, NotUsable(String) }
pub fn validate(model: &MediaModel, req: &MediaRequest) -> Result<(), Vec<Invalid>>;
// price.rs (types only in this task; Task A2 fills them)
pub enum Price { PerImage(Table), PerSecond(Table), PerToken, Unknown, Free }    // Free: a local run
pub struct Table { pub rows: Vec<PriceRow>, pub per_input_image_usd: Option<f64>, pub minimum_usd: Option<f64> }
pub struct PriceRow { pub mode: Option<Mode>, pub resolution: Option<String>, pub quality: Option<String>, pub audio: Option<bool>, pub usd: f64 }
pub enum Mode { TextToVideo, ImageToVideo }
```
`MediaRequest` holds the structured request; `extra` holds only top-level extras once `place_extra` (Task A4) has moved passthrough keys (those some `companies[].passthrough` lists) into `provider_options`. The run's `params_json` stores `{params, extra, routing}` as the user set them (Reuse places them again). Every public type above derives `Debug, Clone, PartialEq, Serialize, Deserialize` (camelCase), except the trait and `MediaError`; `RawCatalog` is what Task A11 caches.

- [ ] **Step 1: Write the failing tests** (`crates/agent/src/media/spec.rs`)
```rust
const IMAGES: &str = include_str!("../../tests/fixtures/media/openrouter-images-2026-11-17.json");
const VIDEOS: &str = include_str!("../../tests/fixtures/media/openrouter-videos-2026-11-17.json");
const ENDPOINTS: &str = include_str!("../../tests/fixtures/media/openrouter-image-endpoints-2026-11-17.json");

#[test] fn the_picture_catalogue_gives_68_models() {
    let m = openrouter_image_models(&serde_json::from_str(IMAGES).unwrap(), &BTreeMap::new());
    assert_eq!(m.len(), 68);
    assert!(m.iter().all(|m| m.kind == MediaKind::Image && m.usable.is_none()));
}
#[test] fn gpt_image_2_6_flare_has_its_controls() {
    let m = image("openai/gpt-image-2.6-flare");
    assert_eq!(m.display_name, "GPT Image 2.6 Flare");
    assert_eq!(values(&m, "quality"), ["auto", "low", "medium", "high", "xhigh", "max"]);
    assert_eq!(param(&m, "quality").layer, Layer::Simple);
    assert_eq!(param(&m, "n").kind, ParamKind::Range { min: 1.0, max: 10.0, step: Some(1.0), unit: None });
    assert_eq!(param(&m, "input_references").kind, ParamKind::Images { min: 0, max: 16, role: RefRole::Reference });
    assert_eq!(param(&m, "background").layer, Layer::Advanced);
    assert_eq!(param(&m, "output_compression").requires,
        Some(Requires { key: "output_format".into(), values: vec![json!("jpeg"), json!("webp")] }));
    assert!(maybe(&m, "frame_count").is_some() && maybe(&m, "seed").is_none());
}
#[test] fn nano_banana_has_no_count_and_no_seed() {
    let m = image("google/gemini-nano-banana-2.1");
    assert!(maybe(&m, "n").is_none() && maybe(&m, "seed").is_none());
    assert_eq!(values(&m, "resolution"), ["1K", "2K", "4K"]);
    assert!(param(&m, "resolution").affects_price);
}
#[test] fn k3_edit_needs_a_picture() {
    assert_eq!(param(&image("krea/k3-edit"), "input_references").kind,
        ParamKind::Images { min: 1, max: 8, role: RefRole::Reference });
}
#[test] fn no_catalogue_parameter_is_dropped() {
    for (id, entry) in catalogue_entries(IMAGES) {
        let m = image(&id);
        for key in entry["supported_parameters"].as_object().unwrap().keys() {
            let typed = maybe(&m, key).is_some() || (key == "n" && entry["supported_parameters"]["n"]["max"] == 1)
                || (key == "input_references" && entry["supported_parameters"][key]["max"] == 0);
            let hinted = matches!(maybe(&m, "extra").map(|p| &p.kind), Some(ParamKind::Extra { allowed_keys }) if allowed_keys.contains(key));
            assert!(typed || hinted, "{id}: {key}");
        }
    }
}
#[test] fn the_param_kinds_serialise_as_the_webview_reads_them() {
    assert_eq!(serde_json::to_value(ParamKind::Extra { allowed_keys: vec!["moderation".into()] }).unwrap(),
        json!({"type": "extra", "allowedKeys": ["moderation"]}));
    assert_eq!(serde_json::to_value(ParamKind::Images { min: 0, max: 16, role: RefRole::Reference }).unwrap(),
        json!({"type": "images", "min": 0, "max": 16, "role": "reference"}));
}
#[test] fn picture_extra_settings_come_from_the_saved_endpoints_answer() {
    let flare = image_with_endpoints("openai/gpt-image-2.6-flare");
    assert!(matches!(&param(&flare, "extra").kind, ParamKind::Extra { allowed_keys } if allowed_keys == &["moderation", "complexity"]));
    assert!(maybe(&image("openai/gpt-image-2.6-flare"), "extra").is_none());   // no saved answer yet: nothing to offer
}
#[test] fn the_video_catalogue_gives_33_models_and_4_need_a_video() {
    let m = openrouter_video_models(&serde_json::from_str(VIDEOS).unwrap());
    assert_eq!(m.len(), 33);
    let mut unusable: Vec<_> = m.iter().filter(|m| m.usable.is_some()).map(|m| m.id.as_str()).collect();
    unusable.sort();
    assert_eq!(unusable, ["black-forest-labs/flux-video-edit", "black-forest-labs/flux-video-upscale",
                          "heygen/avatar-iv", "runway/aleph-2"]);
}
#[test] fn video_defaults_keep_the_first_try_cheap() {
    let veo = video("google/veo-3.1");
    assert_eq!(values(&veo, "duration"), [json!(4), json!(6), json!(8)]);
    assert_eq!(param(&veo, "duration").initial, Some(json!(4)));
    assert_eq!(param(&veo, "resolution").initial, Some(json!("720p")));
    assert_eq!(param(&veo, "generate_audio").company_default, Some(json!(true)));
    assert_eq!(param(&veo, "first_frame").layer, Layer::Simple);
    assert_eq!(param(&veo, "last_frame").layer, Layer::Advanced);
    assert_eq!(values(&video("bytedance/seedance-2.5-flash"), "duration")[0], json!(4));    // -1 dropped
    assert_eq!(values(&video("google/veo-3.1-lite"), "duration"), [json!(4), json!(6), json!(8)]);  // sorted
    assert_eq!(param(&video("minimax/hailuo-3-max"), "resolution").initial, Some(json!("480p")));  // no 720p
    assert_eq!(param(&video("x-ai/grok-imagine-video-1.5-lite"), "duration").initial, Some(json!(4)));
    assert!(veo.notes.iter().any(|n| n == "OpenRouter video is not covered by zero data retention."));
    assert!(veo.notes.iter().any(|n| n == "Google adds an invisible SynthID watermark."));
}
#[test] fn video_extra_settings_are_named_but_not_sent() {
    let veo = video("google/veo-3.1");
    assert!(maybe(&veo, "extra").is_none());
    assert!(veo.notes.iter().any(|n| n.contains("negativePrompt") && n.contains("Studio cannot send them for video yet")));
}
#[test] fn this_computer_is_a_company_of_its_own() {
    assert_eq!(serde_json::to_value(Company::Local).unwrap(), json!("local"));
    assert_eq!(serde_json::to_value(FailureKind::OutOfMemory).unwrap(), json!("outOfMemory"));
}
#[test] fn local_models_pass_through_parse_catalog() {
    let m = image("openai/gpt-image-2.6-flare");
    let raw = RawCatalog { images: serde_json::to_value(vec![m.clone()]).unwrap(), videos: None };
    assert_eq!(parse_catalog(Company::Local, &raw, &BTreeMap::new()), vec![m]);
    assert!(parse_catalog(Company::Local, &RawCatalog { images: json!({"x": 1}), videos: None }, &BTreeMap::new()).is_empty());
}
#[tokio::test] async fn cancel_does_nothing_unless_a_provider_says_otherwise() {
    struct Bare;                                     // implements every required method with unreachable!()
    assert!(Bare.cancel("vid_1").await.is_ok());
}
#[test] fn families_group_versions() {
    assert_eq!(family_of("google/veo-3.1-fast"), "google/veo");
    assert_eq!(family_of("openai/gpt-image-2.6-flare"), "openai/gpt-image");
    assert_eq!(family_of("kwaivgi/kling-v3.0-pro"), "kwaivgi/kling");
}
#[test] fn validate_refuses_a_field_the_model_does_not_list() {
    let req = request("google/gemini-nano-banana-2.1", json!({"seed": 7}));
    assert_eq!(validate(&image("google/gemini-nano-banana-2.1"), &req), Err(vec![Invalid::NotOffered("seed".into())]));
}
#[test] fn validate_checks_values_requires_extra_and_pictures() {
    let flare = image("openai/gpt-image-2.6-flare");
    assert_eq!(validate(&flare, &request_p(json!({"quality": "ultra"}))), Err(vec![Invalid::BadValue("quality".into())]));
    assert!(matches!(validate(&flare, &request_p(json!({"output_compression": 80}))).unwrap_err()[0], Invalid::Requires { .. }));
    assert!(validate(&flare, &request_p(json!({"output_format": "webp", "output_compression": 80}))).is_ok());
    assert_eq!(validate(&image("krea/k3-edit"), &request_p(json!({}))).unwrap_err(),
        vec![Invalid::TooFewPictures { role: RefRole::Reference, min: 1 }]);
    let flare = image_with_endpoints("openai/gpt-image-2.6-flare");
    assert!(validate(&flare, &request_x(json!({"moderation": "low"}))).is_ok());
    assert_eq!(validate(&flare, &request_x(json!({"foo": 1}))), Err(vec![Invalid::NotOffered("foo".into())]));
    assert_eq!(validate(&flare, &request_x(json!({"quality": "low"}))), Err(vec![Invalid::Reserved("quality".into())]));
    assert_eq!(validate(&video("google/veo-3.1"), &request_x(json!({"negativePrompt": "rain"}))),
        Err(vec![Invalid::NotOffered("negativePrompt".into())]));
}
```
(`image`, `image_with_endpoints` (with `ENDPOINTS` answers as `image_prices`), `video`, `catalogue_entries`, `param`, `maybe`, `values`, `request*` are small helpers in the test module.)

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p agent --lib media::spec`
Expected: FAIL to compile, `error[E0433]: failed to resolve: could not find 'media'`.

- [ ] **Step 3: Implement** the types above and the parsers in `spec.rs`. Rules the tests do not fully pin:
  - Picture `supported_parameters`: `aspect_ratio` → Shape, simple, enum; `n` → Count, simple, range step 1, **omitted when max is 1**; `resolution` → Resolution, simple, affects price; `quality` → Quality, simple, affects price; `input_references` → References, simple, `Images{min, max}`, omitted when max is 0; `output_format` → Format, advanced; `background` → Background, advanced; `output_compression` → advanced range, requires `output_format` in `["jpeg","webp"]`; `seed: boolean` → Seed, advanced, `Seed{max: SEED_MAX}`; `size: boolean` → Size with `OPENAI_SIZE_RULES` for `openai/` ids, else an Extra key; any other key → an Extra key (sent at the top level, as the catalogue lists it). For `openai/gpt-image-2.6-*` with `gif` in `output_format`: `frame_count` range 2–16 and `frame_duration_ms` range 20–2000 (unit `ms`), both requiring `output_format = "gif"`.
  - Picture extra settings: when `image_prices` holds the model's `/endpoints` answer, `companies = endpoint_companies(answer)` and each record's `allowed_passthrough_parameters` joins the Extra field's `allowed_keys` (in record order, no duplicates); these are sent under `provider.options[<that record's provider_slug>]` (Task A4). No saved answer and no unknown top-level key → no Extra field (the drawer says "Pick the model to see its extra settings.").
  - Video fields: `supported_aspect_ratios` → `aspect_ratio` (Shape); `supported_durations` → `duration` (Length, numbers, `-1` dropped, ascending, labels "4 s", initial = shortest ≥ 4 else the longest); `supported_resolutions` → `resolution` (initial `720p`, else the lowest by its number); `generate_audio == true` → `generate_audio` (Sound, boolean, company default `true`, affects price); `supported_frame_images` → `first_frame` (StartFrame, simple, `Images{0,1}`) and `last_frame` (EndFrame, advanced); `seed == true` → Seed; `supported_sizes` → `size` (Size, advanced enum; when set, `aspect_ratio` and `resolution` are not sent); a non-empty `allowed_passthrough_parameters` → the note "OpenRouter lists extra settings for this model (<names>), but Studio cannot send them for video yet." (OpenRouter keys them by the company the job is routed to, which the video catalogue does not name). `usable = Some("Needs a video to start from, which Studio does not offer yet.")` when `supported_durations` is null or empty.
  - Display name: catalogue `name` without its `"<Maker>: "` prefix. Family: `<maker>/` + the name cut before the first `-` followed by a digit, or by `v` and a digit.
  - `validate`: param keys must be offered non-picture fields; enum values from the list; ranges within bounds and on the step; seeds integers in `0..=max`; sizes pass `SizeRules` or are in the list; `requires` met; extra keys only from `allowed_keys` (a passthrough key also needs a company the request's routing allows that lists it) and never a typed key, `model`, `prompt` or `provider` (`Reserved`); pictures per role within `min..=max`; prompt 1–32,000 characters; an unusable model is refused.
  - `cloud::media_company`: `OpenRouter → Some(Company::OpenRouter)`, `OpenAi → Some(Company::OpenAi)`, everything else `None`.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p agent --lib media::spec && cargo clippy -p agent --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 5: Commit**
```bash
git add crates/agent/src/media crates/agent/src/lib.rs crates/agent/src/cloud.rs crates/agent/tests/fixtures/media
git commit -m "Studio: media types and controls built from OpenRouter's catalogues"
```

### Task A2: Prices and estimates

**Files:**
- Modify: `crates/agent/src/media/price.rs`, `crates/agent/src/media/spec.rs` (fill `MediaModel.price`)
- Test: inline in `price.rs` (fixtures from Task A1)

**Interfaces:**
- Consumes: Task A1 types.
- Produces:
```rust
pub struct PriceChoice { pub count: u32, pub seconds: Option<f64>, pub resolution: Option<String>, pub quality: Option<String>,
    pub audio: Option<bool>, pub start_frame: bool, pub input_images: u32 }
pub fn parse_pricing_skus(skus: &Map<String, Value>) -> Price;
/// One saved `/endpoints` answer. With several companies the dearest is used, unless `routing` pins one
/// (`only`, or `allow_fallbacks == false`): then that company's price.
pub fn parse_image_endpoints(answer: &Value, routing: Option<&Routing>) -> Price;
pub fn estimate(price: &Price, choice: &PriceChoice) -> Option<f64>;          // None = price unknown
pub fn choice_for(model: &MediaModel, req: &MediaRequest) -> PriceChoice;
impl Price { pub fn range(&self) -> Option<(f64, f64)>; }                     // for the picker's price hint
```
The recorded `/endpoints` shape (2026-11-17): `{id, endpoints: [{provider_name, provider_slug, provider_tag, supported_parameters, allowed_passthrough_parameters, pricing: [{billable: "output_image"|"input_image"|"input_text"|"input_reference", unit: "image"|"megapixel"|"token", cost_usd, variant?}]}]}`. On that day GPT Image 2.6 Flare and Nano Banana 2.1 were priced per token only, FLUX 3 Image per picture by resolution (`variant` "768", "1k", "1.5k", "2k", "4k"), Seedream 5.5 per picture plus a `high_resolution` variant and `input_image` per picture.

- [ ] **Step 1: Write the failing tests**
```rust
fn sku(id: &str) -> Price { /* parse_pricing_skus of that model in the video fixture */ }
fn secs(price: &Price, s: f64, res: &str, audio: Option<bool>, start: bool, images: u32) -> Option<f64>;
fn usd(a: Option<f64>, b: f64) { assert!((a.unwrap() - b).abs() < 1e-9, "{a:?} != {b}"); }
// also: endpoints_price(id, routing) from the recorded answers, pics(count, resolution, input_images) -> PriceChoice,
// two_companies, only(tag), order(tag, allow_fallbacks), row(resolution, usd), catalogue_videos()

#[test] fn veo_3_1_prices() {
    let p = sku("google/veo-3.1");
    usd(secs(&p, 8.0, "720p", Some(true), false, 0), 3.20);
    usd(secs(&p, 8.0, "4K", Some(true), false, 0), 4.80);
    usd(secs(&p, 8.0, "1080p", Some(false), false, 0), 1.60);
}
#[test] fn the_resolution_row_comes_before_the_plain_one() {
    let p = sku("google/veo-3.1-fast");
    usd(secs(&p, 8.0, "720p", Some(true), false, 0), 0.80);
    usd(secs(&p, 8.0, "1080p", Some(true), false, 0), 0.96);
}
#[test] fn cents_are_cents() {
    usd(secs(&sku("x-ai/grok-imagine-video-1.5-lite"), 4.0, "480p", Some(true), true, 1), 0.09);
    usd(secs(&sku("tbc/mischief-1"), 5.0, "720p", None, false, 0), 0.05);
}
#[test] fn mode_keys_follow_the_start_frame() {
    let p = sku("alibaba/wan-2.6");
    usd(secs(&p, 5.0, "720p", Some(true), false, 0), 0.40);
    usd(secs(&p, 5.0, "720p", Some(true), true, 0), 0.50);
    assert_eq!(secs(&p, 5.0, "480p", Some(true), true, 0), None);
}
#[test] fn sound_keys() {
    let k = sku("kwaivgi/kling-v3.0-pro");
    usd(secs(&k, 5.0, "720p", Some(true), false, 0), 0.84);
    usd(secs(&k, 5.0, "720p", Some(false), false, 0), 0.56);
    assert_eq!(secs(&sku("google/gemini-omni-1.2-flash"), 8.0, "720p", Some(false), false, 0), None);
}
#[test] fn a_minimum_per_run_applies() {
    usd(secs(&sku("runway/aleph-2"), 1.0, "", None, false, 0), 0.56);
    usd(secs(&sku("runway/aleph-2"), 3.0, "", None, false, 0), 0.84);
}
#[test] fn video_tokens_have_no_estimate() {
    assert_eq!(sku("bytedance/seedance-2.5"), Price::PerToken);
    assert_eq!(secs(&sku("bytedance/seedance-2.5"), 5.0, "720p", Some(true), false, 0), None);
}
#[test] fn keys_it_does_not_know_give_unknown() {
    for id in ["heygen/heygen-video-1", "black-forest-labs/flux-video-upscale", "minimax/hailuo-3"] {
        assert_eq!(sku(id), Price::Unknown, "{id}");
    }
}
#[test] fn features_studio_does_not_offer_are_ignored() {
    usd(secs(&sku("black-forest-labs/flux-3-video"), 5.0, "720p", Some(true), false, 0), 0.85);
}
#[test] fn every_catalogue_video_gets_a_price_or_says_why() {
    let prices: Vec<(String, Price)> = catalogue_videos().map(|(id, skus)| (id, parse_pricing_skus(&skus))).collect();
    assert_eq!(prices.len(), 33);
    let unknown: Vec<_> = prices.iter().filter(|(_, p)| *p == Price::Unknown).map(|(id, _)| id.as_str()).collect();
    assert_eq!(unknown.len(), 3, "{unknown:?}");                         // the three ids above
    assert_eq!(prices.iter().filter(|(_, p)| *p == Price::PerToken).count(), 5);   // the seedance models
    assert_eq!(prices.iter().filter(|(_, p)| matches!(p, Price::PerSecond(_))).count(), 25);
}
#[test] fn picture_prices_from_the_recorded_endpoints() {
    assert_eq!(endpoints_price("openai/gpt-image-2.6-flare", None), Price::PerToken);
    assert_eq!(endpoints_price("google/gemini-nano-banana-2.1", None), Price::PerToken);   // both companies per token
    let flux = endpoints_price("black-forest-labs/flux-3-image", None);
    usd(estimate(&flux, &pics(1, "1K", 0)), 0.048);
    usd(estimate(&flux, &pics(1, "1.5K", 0)), 0.07);
    usd(estimate(&flux, &pics(1, "4K", 0)), 0.607);
    let seedream = endpoints_price("bytedance-seed/seedream-5-5", None);       // "high_resolution" matches no value: the dearer row
    usd(estimate(&seedream, &pics(1, "1K", 0)), 0.035);
    usd(estimate(&seedream, &pics(1, "2K", 1)), 0.043);                       // + 0.008 per added picture
}
#[test] fn a_pinned_company_is_used_only_when_routing_cannot_fall_back() {
    let answer = two_companies(("a", 0.04), ("b", 0.06));                     // synthetic, same shape as the recording
    usd(estimate(&parse_image_endpoints(&answer, None), &pics(1, "1K", 0)), 0.06);
    usd(estimate(&parse_image_endpoints(&answer, Some(&only("a"))), &pics(1, "1K", 0)), 0.04);
    usd(estimate(&parse_image_endpoints(&answer, Some(&order("a", true))), &pics(1, "1K", 0)), 0.06);
    usd(estimate(&parse_image_endpoints(&answer, Some(&order("a", false))), &pics(1, "1K", 0)), 0.04);
}
#[test] fn count_multiplies_and_pictures_in_add() {
    let p = Price::PerImage(Table { rows: vec![row(None, 0.04)], per_input_image_usd: Some(0.01), minimum_usd: None });
    usd(estimate(&p, &pics(4, "1K", 2)), 0.18);
}
#[test] fn a_local_run_costs_nothing_whatever_its_size() {
    assert_eq!(estimate(&Price::Free, &pics(4, "1K", 2)), Some(0.0));
    assert_eq!(Price::Free.range(), Some((0.0, 0.0)));
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p agent --lib media::price`
Expected: FAIL to compile, `cannot find function 'parse_pricing_skus'`.

- [ ] **Step 3: Implement.** The lookup order for a per-second price (`res` lower-cased: `4K` → `4k`; dollars unless the key starts with `cents_`):
  1. sound on: `duration_seconds_with_audio_{res}`, `duration_seconds_with_audio`; sound off: `duration_seconds_without_audio_{res}`, `duration_seconds_without_audio`;
  2. then, whatever the sound: `{text_to_video|image_to_video}_duration_seconds_{res}` (image-to-video when a start frame is sent), `duration_seconds_{res}`, `duration_seconds`, `cents_per_video_output_second_{res}`, `cents_per_second_output_{res}`, `cents_per_second_output`;
  3. none found → `None` (unknown). `cents_per_image_input` adds per added picture; `minimum_cents_per_generation` is a floor. Keys containing `video_continuation` or `with_video_input` are ignored (features not in 0.1.0); any `video_tokens*` key makes the price `PerToken`; any other key makes the whole price `Unknown`, never a guess.
  - `choice_for`: count from `n` (1 when absent; always 1 for video), seconds from `duration`, resolution from `resolution` or from `size`'s shorter side (`1280x720` → `720p`), quality from `quality`, sound from `generate_audio` or its company default, `start_frame` when a StartFrame picture is sent, `input_images` = pictures sent.
  - `parse_image_endpoints`: per company, `output_image` rows with unit `image` become `PerImage` rows keyed by `variant` (no variant = every resolution), matched case-insensitively to the resolution value (`1k` ↔ `1K`, `1.5k` ↔ `1.5K`); when any variant matches no offered resolution (Seedream's `high_resolution`), every choice uses that company's dearest `output_image` row, so the estimate is never too low; `input_image`/`input_reference` rows with unit `image` fill `per_input_image_usd`; any row with unit `token` makes the company `PerToken`, unit `megapixel` makes it `Unknown`. Then the dearest company, or the pinned one as the doc comment says (`PerToken`/`Unknown` for any allowed company wins over a number).
  - `parse_catalog` now sets `price` from `pricing_skus` (video) and from `image_prices[id]` when cached (pictures), else `Price::Unknown`; local models keep the `Price::Free` the local provider gave them.
  - `estimate(&Price::Free, _)` is `Some(0.0)` for any choice.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p agent --lib media`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/agent/src/media crates/agent/tests/fixtures/media
git commit -m "Studio: read OpenRouter prices and estimate a run, unknown when unsure"
```

### Task A3: Spending guard

**Files:**
- Create: `src-tauri/src/studio/mod.rs` (`pub mod spend;`), `src-tauri/src/studio/spend.rs`
- Modify: `src-tauri/src/lib.rs` (`#[allow(dead_code, reason = "wired up in Task A16")] mod studio;`; the attribute goes in Task A16)
- Test: inline in `spend.rs`

**Interfaces:**
- Consumes: `agent::media::{MediaKind, Price}`, `chrono`.
- Produces:
```rust
pub const CONFIRM_AT_USD: f64 = 1.0;
pub const DEFAULT_DAILY_LIMIT_USD: f64 = 10.0;
pub const UNKNOWN_PRICE_RESERVE_USD: f64 = 5.0;
pub struct ConfirmRules { pub every_video: bool, pub at_usd: f64 }          // Default: true, CONFIRM_AT_USD
#[serde(rename_all = "camelCase")]
pub enum ConfirmReason { Video, AtLeast, UnknownPrice }                    // unit variants: "video" | "atLeast" | "unknownPrice";
                                                                           // the threshold travels as EstimateDto.confirm_at_usd
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Billing { Free, Paid(Option<f64>) }                               // Paid(None): the price is unknown
pub fn billing_of(price: &Price, estimate: Option<f64>) -> Billing;        // Price::Free → Free, whatever the estimate
pub fn confirm_reasons(kind: MediaKind, billing: Billing, rules: &ConfirmRules) -> Vec<ConfirmReason>;   // Free → always empty
pub enum Charge { Pending, Final, None, Maybe }
pub enum RunState { Live, Done, Ended { charged: Option<bool> } }          // Ended: failed, blocked, cancelled, interrupted
pub fn charge_of(state: RunState) -> Charge;  // Live → Pending; Done → Final; Ended: Some(true) → Final, Some(false) → None, None → Maybe
pub struct SpendRow { pub created_at: i64, pub estimate_usd: Option<f64>, pub cost_usd: Option<f64>, pub charge: Charge }
pub fn counted_usd(row: &SpendRow) -> f64;
pub struct Spend { pub settled_usd: f64, pub pending_usd: f64 }
pub fn spent_since(rows: &[SpendRow], day_start_ms: i64) -> Spend;
pub enum LimitRefusal { Over { spent: f64, limit: f64 } }
pub fn check_limit(limit: Option<f64>, spend: &Spend, billing: Billing) -> Result<(), LimitRefusal>;   // Free → always Ok
pub fn day_start_in<Tz: chrono::TimeZone>(tz: &Tz, at_ms: i64) -> i64;
```

- [ ] **Step 1: Write the failing tests** (`use Billing::Paid;`)
```rust
#[test] fn every_video_asks() {
    assert_eq!(confirm_reasons(MediaKind::Video, Paid(Some(0.09)), &ConfirmRules::default()), [ConfirmReason::Video]);
}
#[test] fn a_dollar_or_more_asks() {
    assert!(confirm_reasons(MediaKind::Image, Paid(Some(0.99)), &ConfirmRules::default()).is_empty());
    assert_eq!(confirm_reasons(MediaKind::Image, Paid(Some(1.00)), &ConfirmRules::default()), [ConfirmReason::AtLeast]);
}
#[test] fn a_local_run_never_asks_even_for_a_video() {
    assert_eq!(billing_of(&Price::Free, Some(0.0)), Billing::Free);
    assert_eq!(billing_of(&Price::Unknown, None), Paid(None));
    assert!(confirm_reasons(MediaKind::Video, Billing::Free, &ConfirmRules::default()).is_empty());
}
#[test] fn a_local_run_passes_any_limit() {
    assert!(check_limit(Some(10.0), &Spend { settled_usd: 10.0, pending_usd: 3.0 }, Billing::Free).is_ok());
    assert_eq!(counted_usd(&row(Some(0.0), Some(0.0), Charge::None)), 0.0);   // how a local run is recorded
}
#[test] fn reasons_serialise_as_plain_names() {
    assert_eq!(serde_json::to_value([ConfirmReason::Video, ConfirmReason::AtLeast, ConfirmReason::UnknownPrice]).unwrap(),
        json!(["video", "atLeast", "unknownPrice"]));
}
#[test] fn what_a_run_is_charged_follows_what_the_company_said() {
    assert_eq!(charge_of(RunState::Live), Charge::Pending);
    assert_eq!(charge_of(RunState::Done), Charge::Final);
    assert_eq!(charge_of(RunState::Ended { charged: Some(true) }), Charge::Final);
    assert_eq!(charge_of(RunState::Ended { charged: Some(false) }), Charge::None);
    assert_eq!(charge_of(RunState::Ended { charged: None }), Charge::Maybe);
}
#[test] fn an_unknown_price_always_asks() {
    let lax = ConfirmRules { every_video: false, at_usd: 100.0 };
    assert_eq!(confirm_reasons(MediaKind::Image, Paid(None), &lax), [ConfirmReason::UnknownPrice]);
}
#[test] fn dont_ask_again_for_videos_keeps_the_dollar_rule() {
    let r = ConfirmRules { every_video: false, at_usd: 1.0 };
    assert!(confirm_reasons(MediaKind::Video, Paid(Some(0.96)), &r).is_empty());
    assert_eq!(confirm_reasons(MediaKind::Video, Paid(Some(1.20)), &r), [ConfirmReason::AtLeast]);
}
#[test] fn reaching_the_limit_exactly_is_allowed() {
    let s = Spend { settled_usd: 9.04, pending_usd: 0.0 };
    assert!(check_limit(Some(10.0), &s, Paid(Some(0.96))).is_ok());
    assert_eq!(check_limit(Some(10.0), &s, Paid(Some(0.97))), Err(LimitRefusal::Over { spent: 9.04, limit: 10.0 }));
}
#[test] fn an_unknown_price_needs_five_dollars_left() {
    assert!(check_limit(Some(10.0), &Spend { settled_usd: 4.0, pending_usd: 1.0 }, Paid(None)).is_ok());
    assert!(check_limit(Some(10.0), &Spend { settled_usd: 5.01, pending_usd: 0.0 }, Paid(None)).is_err());
}
#[test] fn no_limit_refuses_nothing() {
    assert!(check_limit(None, &Spend { settled_usd: 1e6, pending_usd: 0.0 }, Paid(None)).is_ok());
}
#[test] fn what_each_run_counts() {
    assert_eq!(counted_usd(&row(Some(0.5), Some(0.42), Charge::Final)), 0.42);
    assert_eq!(counted_usd(&row(Some(3.0), None, Charge::None)), 0.0);
    assert_eq!(counted_usd(&row(Some(0.96), None, Charge::Pending)), 0.96);
    assert_eq!(counted_usd(&row(Some(0.96), None, Charge::Maybe)), 0.96);
    assert_eq!(counted_usd(&row(None, None, Charge::Pending)), UNKNOWN_PRICE_RESERVE_USD);
}
#[test] fn a_run_counts_on_the_day_it_was_created() {
    let tz = FixedOffset::east_opt(3600).unwrap();
    let late = ms(&tz, 2026, 11, 17, 23, 59, 30);
    let next_day = day_start_in(&tz, ms(&tz, 2026, 11, 18, 0, 0, 10));
    assert_eq!(spent_since(&[row_at(late, Some(0.96), Charge::Pending)], day_start_in(&tz, late)).pending_usd, 0.96);
    assert_eq!(spent_since(&[row_at(late, Some(0.96), Charge::Pending)], next_day).pending_usd, 0.0);
}
#[test] fn the_day_starts_at_local_midnight() {
    for secs in [14 * 3600, -12 * 3600] {
        let tz = FixedOffset::east_opt(secs).unwrap();
        assert_eq!(day_start_in(&tz, ms(&tz, 2026, 11, 17, 13, 5, 0)), ms(&tz, 2026, 11, 17, 0, 0, 0));
    }
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::spend`
Expected: FAIL to compile, `file not found for module 'studio'` / unresolved names.

- [ ] **Step 3: Implement** with these rules: `counted_usd` is the reported cost when there is one (any charge but `None`), 0 for `Charge::None`, else the estimate, else `UNKNOWN_PRICE_RESERVE_USD`. `spent_since` sums rows with `created_at >= day_start_ms`, `Pending` into `pending_usd`, the rest into `settled_usd`. `check_limit` returns `Ok` for `Billing::Free`; for `Paid(Some(e))` it refuses when `settled + pending + e > limit + 1e-9`, and for `Paid(None)` when `limit - settled - pending < UNKNOWN_PRICE_RESERVE_USD`. `confirm_reasons` is empty for `Billing::Free`. A local run is stored with `estimate_usd = Some(0.0)` and, when done, `cost_usd = Some(0.0)`, so it adds $0 to every sum. `day_start_in` takes the local date of `at_ms`, then midnight's `earliest()` instant, or the first valid instant after it when midnight does not exist in that zone. The app uses `day_start_in(&chrono::Local, now)`.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio::spend`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio src-tauri/src/lib.rs
git commit -m "Studio: confirmation rules and the daily limit, counted per local day"
```

### Task A4: A create request sent at most once, and OpenRouter pictures

**Files:**
- Modify: `crates/agent/src/client.rs` (make `same_origin_redirects`, `authorize`, `read_capped`, `network_error` `pub(crate)`; add `post_once` and `stream_body_to`), `crates/agent/src/cloud.rs` (`pub(crate) fn scrub_text(text: &str, key: Option<&str>) -> String` over the private `without_key`, for failure messages that are plain text)
- Create: `crates/agent/src/media/openrouter.rs` (catalogues, prices, pictures, preview, failure mapping)
- Modify: `crates/agent/src/media/mod.rs` (`media_provider`)
- Test: `crates/agent/tests/media_endpoints.rs` (new, on `tests/common` `MockServer`)

**Interfaces:**
- Consumes: Task A1 trait and types, Task A2 `parse_image_endpoints`.
- Produces:
```rust
// client.rs
pub(crate) enum OnceBody { Json(Value), Multipart { boundary: String, bytes: Vec<u8> } }
pub(crate) enum OnceReply { Success { path: PathBuf, len: u64, headers: HeaderMap }, Error { status: u16, body: Vec<u8>, headers: HeaderMap } }
impl OpenAiClient {
    /// Sends `body` once; one more try only on 429 with Retry-After <= 30 s, or a connect failure.
    /// A success body is streamed to a new file in `scratch` (at most `cap` bytes).
    pub(crate) async fn post_once(&self, url: Url, body: OnceBody, limit: Duration, cap: u64, scratch: &Path) -> Result<OnceReply, MediaError>;
}
/// The one writer for response bodies: chunks to `file`, at most `cap` bytes, `idle` at most between chunks.
/// Over the cap → `TooLarge(cap)`; `ErrorKind::StorageFull` → `DiskFull`; other write errors → `Io`; a body that stops → Err(network).
pub(crate) async fn stream_body_to(resp: reqwest::Response, file: &mut tokio::fs::File, cap: u64, idle: Option<Duration>)
    -> Result<u64, BodyError>;                                  // BodyError { Local(LocalIo), Network(reqwest::Error) }
// URLs are built with the existing `api_url()` (`.../api/v1` or `.../v1`) and `join`.
// openrouter.rs
pub struct OpenRouterMedia { /* client, limits */ }
impl OpenRouterMedia { pub fn new(client: OpenAiClient) -> Self; pub fn with_limits(self, limits: MediaLimits) -> Self; }
impl MediaProvider for OpenRouterMedia { /* pictures in this task; check/download in Task A5 */ }
pub(crate) fn openrouter_failure(status: u16, headers: &HeaderMap, body: &[u8], key: Option<&str>) -> MediaFailure;
// mod.rs
pub fn media_provider(client: OpenAiClient) -> Option<Arc<dyn MediaProvider>>;   // OpenRouter here; OpenAI in Task A7
// openrouter.rs: splits the user's extra settings into top-level keys and `provider.options` per company
pub fn place_extra(model: &MediaModel, extra: Map<String, Value>, routing: Option<&Routing>)
    -> (Map<String, Value>, BTreeMap<String, Map<String, Value>>);
```

- [ ] **Step 1: Write the failing tests** (`crates/agent/tests/media_endpoints.rs`)
```rust
const KEY: &str = "sk-or-v1-studio-test-key-0123456789";
fn openrouter(server: &MockServer) -> OpenRouterMedia;          // ClientConfig::new(server.url() + "/api/v1", OpenRouter), key KEY
fn quick() -> MediaLimits { MediaLimits { picture_timeout: Duration::from_secs(1), response_cap: 64 * 1024, ..Default::default() } }

#[tokio::test] async fn the_catalogues_come_from_the_endpoints_own_client() {
    // GET /api/v1/images/models and /api/v1/videos/models → RawCatalog with both; Authorization: Bearer KEY sent to this host only
}
#[tokio::test] async fn a_picture_run_decodes_every_picture() {
    // reply 200 {"data":[{"b64_json":PNG_A,"media_type":"image/png"},{"b64_json":PNG_B}],"usage":{"cost":0.08}}
    let Started::Done { outputs, cost } = media.start(&req_with_ref(), dir.path()).await.unwrap() else { panic!() };
    assert_eq!(outputs.len(), 2);
    assert_eq!(std::fs::read(&outputs[0].path).unwrap(), PNG_A_BYTES);
    assert_eq!(cost, Some(Cost { usd: 0.08, reported: true }));
    let sent = server.requests()[0].json();
    assert_eq!(sent["model"], "openai/gpt-image-2.6-flare");
    assert_eq!(sent["n"], 2);
    assert!(sent["input_references"][0]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
    assert_eq!(server.requests()[0].path(), "/api/v1/images");
}
#[tokio::test] async fn a_500_is_never_sent_again() {
    assert!(matches!(media.start(&req(), dir).await, Err(MediaError::Unknown(_))));
    assert_eq!(server.requests().len(), 1);
}
#[tokio::test] async fn a_timeout_is_never_sent_again()     { /* Reply::Silence(3 s), limit 1 s → Unknown, 1 request */ }
#[tokio::test] async fn a_cut_connection_is_never_sent_again() { /* Reply::Stream{end: Abort} → Unknown, 1 request */ }
#[tokio::test] async fn a_429_with_a_short_wait_is_sent_once_more() {
    // WithHeaders 429 retry-after "1", then 200 → Ok; server.requests().len() == 2
}
#[tokio::test] async fn a_429_with_a_long_wait_is_not_sent_again() {
    // retry-after "31" → Err(Answered(f)) with f.kind == RateLimited and f.retry_after == Some(31 s); 1 request
}
#[tokio::test] async fn a_refused_connection_was_not_sent() {
    // base URL on a port with nothing listening → Err(MediaError::NotSent(_))
}
#[tokio::test] async fn a_502_was_not_charged() {
    // 502 → Answered(MediaFailure { kind: Server, charged: Some(false), .. })
}
#[tokio::test] async fn a_moderation_error_is_blocked() {
    // 403 {"error":{"code":403,"message":"Your prompt was flagged by moderation","metadata":{"reasons":["violence"]}}}
    // → Answered(f): f.kind == Blocked, f.charged == Some(false), f.status == Some(403)
}
#[tokio::test] async fn the_key_never_appears_in_an_error() {
    // 400 body echoing KEY → f.message contains "[API key]" and not KEY
}
#[tokio::test] async fn a_body_over_the_cap_is_refused_and_its_file_removed() {
    // 200 with 100 KiB body, cap 64 KiB
    assert!(matches!(media.start(&req(), dir.path()).await, Err(MediaError::Local(LocalIo::TooLarge(65_536)))));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
#[tokio::test] async fn the_picture_body_matches_openrouters_documented_shape() {
    // fixture openrouter-request-examples-2026-11-17.json: "image_references", "image_routing", "image_provider_options"
    media.start(&req_with_ref_routing_and_extra(), dir.path()).await.unwrap();
    let sent = server.requests()[0].json();
    assert_same_shape(&sent["input_references"][0], &example("image_references")["input_references"][0]);
    assert_eq!(sent["provider"]["only"], json!(["openai"]));            // `only: true` → a list of provider_tag values
    assert_eq!(sent["provider"]["allow_fallbacks"], false);
}
#[tokio::test] async fn extra_settings_go_under_provider_options_for_each_company_that_lists_them() {
    // flare with its recorded /endpoints answer (record "openai" lists moderation, complexity); extra {"moderation":"low"}
    let sent = server.requests()[0].json();
    assert_eq!(sent["provider"]["options"]["openai"], json!({"moderation": "low"}));
    assert!(sent.get("moderation").is_none());
}
#[tokio::test] async fn the_preview_hides_pictures_and_the_key() {
    let p = media.preview(&req_with_ref());
    assert_eq!(p.host, "127.0.0.1");
    assert!(p.body["input_references"][0].as_str().unwrap().starts_with("picture 1 (sha "));
    assert!(!p.body.to_string().contains(KEY));
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p agent --test media_endpoints`
Expected: FAIL to compile, `cannot find type 'OpenRouterMedia' in module 'agent::media'`.

- [ ] **Step 3: Implement.**
  - `post_once`: build the request with `authorize`, `limit` as the whole-request timeout, no `retry_delay`. Error mapping: `reqwest::Error::is_connect()` → retry once after 1 s, then `NotSent`; timeout, a body read that fails, or any other send error → `Unknown`; a 429 with `cloud::parse_retry_after` ≤ 30 s → wait and send once more; any other non-2xx → `OnceReply::Error` (body read with `read_capped(.., MAX_ERROR_BODY, true)`); 2xx → `stream_body_to` into `scratch/<uuid>.part` (no idle limit; `limit` covers it), removing the file on any error and returning `MediaError::Local(..)` for local failures and `Unknown` for a body that stops. Error texts go through `cloud::scrub_key` (a `ProviderError`) or `cloud::scrub_text` (a message).
  - Picture body: `model`, `prompt`, every `params` entry, extra keys that are catalogue fields at the top level, `input_references: [{"type":"image_url","image_url":{"url":"data:<mime>;base64,..."}}]`, and `provider`: `only: [tag]` when `routing.only`, else `order: [tags]`; `allow_fallbacks`; `zdr` when true; `options` = `req.provider_options` when not empty. `place_extra` fills those: a passthrough key goes to every company in `model.companies` whose record lists it (only the pinned one when `only`); any other key stays top-level (`validate` has already refused keys the model does not offer). `assert_same_shape` compares key paths and JSON types, not values. Response read with a typed struct (`data: Vec<{b64_json: String, media_type: Option<String>}>`, `usage.cost`), one picture decoded and written at a time to `scratch/<uuid>-<n>`; MIME from `media_type`, else from the bytes.
  - `fetch_catalog`: `GET {api}/images/models` and `{api}/videos/models` through the endpoint's client. `fetch_image_price`: `GET {api}/images/models/{id}/endpoints`.
  - `openrouter_failure`: 401 `Unauthorized`; 402 `NoCredit`; 403 with "moderation"/"flagged" or a `moderation` metadata object, or 400 with the same → `Blocked`, `charged: Some(false)`; 429 `RateLimited` with `retry_after`; 404 `NotFound`; 502 `Server` with `charged: Some(false)` ("failed, not charged"); other 5xx `Server`; `request_id` from the body's `id` or the `x-request-id` header; `message` cut by `error::bounded` and scrubbed.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p agent --test media_endpoints && cargo test -p agent --lib`
Expected: PASS (the existing client tests still pass).

- [ ] **Step 5: Commit**
```bash
git add crates/agent/src/client.rs crates/agent/src/media crates/agent/tests/media_endpoints.rs
git commit -m "Studio: OpenRouter pictures, with the create request sent at most once"
```

### Task A5: OpenRouter video jobs and streamed downloads

**Files:**
- Modify: `crates/agent/src/client.rs` (`download_to_file`, `get_json_within`), `crates/agent/src/media/openrouter.rs`
- Test: `crates/agent/tests/media_endpoints.rs`; Create: `crates/agent/tests/media_redirects.rs` (one test, sets `NO_PROXY=*` before any thread, like `tests/redirects.rs`)

**Interfaces:**
- Consumes: Task A4.
- Produces:
```rust
impl OpenAiClient {
    pub(crate) async fn get_json_within(&self, url: Url, limit: Duration) -> Result<Value, ProviderError>;
    /// Streams to `dest` through `stream_body_to` (at most `cap` bytes, at most `idle` between chunks; local
    /// failures come back as `MediaError::Local`). A 3xx whose Location is another origin comes back
    /// as `Fetched::Elsewhere` and is never followed with the key.
    pub(crate) async fn download_to_file(&self, url: Url, dest: &Path, cap: u64, idle: Duration) -> Result<Fetched, MediaError>;
}
// OpenRouterMedia: start (video) → Started::Job; check; download
```

- [ ] **Step 1: Write the failing tests**
```rust
#[tokio::test] async fn a_video_job_starts_checks_and_downloads() {
    // POST /api/v1/videos → 202 {"id":"vid_1","polling_url":"...","status":"pending"}
    assert_eq!(media.start(&video_req(), dir).await.unwrap(), Started::Job { remote_id: "vid_1".into() });
    // GET /api/v1/videos/vid_1: pending → Pending; in_progress {"progress":0.4} → Running{progress: Some(0.4)};
    // completed {"usage":{"cost":0.36},"unsigned_urls":["..."]} → Done(JobDone { outputs: 1, cost: Some(Cost{usd:0.36,reported:true}), .. })
    // download(…, 0, dest) → GET /api/v1/videos/vid_1/content?index=0 (video/mp4) → Fetched::Saved { mime: "video/mp4", .. }
    let sent = server.requests()[0].json();
    assert_eq!(sent["duration"], 4);
    assert_eq!(sent["frame_images"][0]["frame_type"], "first_frame");
}
#[tokio::test] async fn a_failed_job_says_whether_it_was_charged() { /* status failed + error → Failed(f), f.charged == Some(false) */ }
#[tokio::test] async fn a_job_the_company_no_longer_has_is_gone() { /* 404 on check → JobState::Gone */ }
#[tokio::test] async fn a_check_that_times_out_is_an_error_not_a_state() { /* Silence, check_timeout 1 s → Err(Unknown) */ }
#[tokio::test] async fn content_gone_after_done_is_expired() {
    // 404 and 410 on /content → Err(Answered(f)) with f.kind == FailureKind::Expired
}
#[tokio::test] async fn a_download_over_the_cap_removes_the_partial_file() {
    // video_cap 1 KiB, content 4 KiB
    assert!(matches!(media.download("vid_1", &done, 0, &dest).await, Err(MediaError::Local(LocalIo::TooLarge(1024)))));
    assert!(!dest.exists());
}
#[cfg(target_os = "linux")]
#[tokio::test] async fn a_full_disk_while_downloading_is_named() {
    // /dev/full accepts the open and refuses every write with ENOSPC
    assert!(matches!(media.download("vid_1", &done, 0, Path::new("/dev/full")).await, Err(MediaError::Local(LocalIo::DiskFull))));
}
#[tokio::test] async fn a_stalled_download_stops_after_the_idle_limit() {
    // download_idle 200 ms; Reply::Stream sends 1 KiB then waits 2 s → Err(MediaError::Unknown(_)) within 1 s
}
#[tokio::test] async fn the_video_body_matches_openrouters_documented_shape() {
    // fixture "video_frame_images": sent["frame_images"][0] has the same shape; no "provider" key; extras are never sent for video
    assert_same_shape(&sent["frame_images"][0], &example("video_frame_images")["frame_images"][0]);
    assert!(sent.get("provider").is_none());
}
#[tokio::test] async fn a_link_to_another_host_is_handed_back_not_followed() {
    // /content → 302 Location: http://127.0.0.1:<other>/v.mp4 → Ok(Fetched::Elsewhere(url)); `other` saw no request
}
#[tokio::test] async fn signed_urls_are_kept_for_the_guarded_fetcher() {
    // completed with "urls":[{"url":"https://cdn.example/v.mp4","expires_at":1794900000}] → JobDone.signed[0]
}
// media_redirects.rs (loopback = false):
#[test] fn a_redirect_to_another_origin_carries_no_key() {
    // content → 307 to another port → Fetched::Elsewhere; the other server recorded 0 requests
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p agent --test media_endpoints && cargo test -p agent --test media_redirects`
Expected: FAIL (`start` for video returns an error; `download_to_file` does not exist).

- [ ] **Step 3: Implement.** The shapes come from the examples recorded from https://openrouter.ai/docs/guides/overview/multimodal/video-generation on 2026-11-17 (`frame_images` items `{"type":"image_url","image_url":{"url":"data:…"},"frame_type":"first_frame"|"last_frame"}`; passthrough settings would go under `provider.options[<slug>]`, nested in `parameters` for Veo only, which is why 0.1.0 sends none for video). Video body: `model`, `prompt`, `duration`, `resolution`, `aspect_ratio` or `size`, `generate_audio`, `seed`, `frame_images`; no `provider`, no extra keys. Status mapping: `pending` → `Pending`; `in_progress` → `Running{progress}` (a 0–1 number when present, a 0–100 number divided by 100); `completed` → `Done` (outputs = `max(len(unsigned_urls), len(urls), 1)`); `failed`, `cancelled`, `expired` → `Failed` (`expired` with kind `Expired`); 404 → `Gone`. Each check uses `get_json_within(limits.check_timeout)`. `download` prefers `GET {api}/videos/{id}/content?index=N` with the key (`download_to_file(.., limits.video_cap, limits.download_idle)`; the docs say `unsigned_urls` keep working with the key for as long as the output is available); when only `signed` URLs exist it returns `Fetched::Elsewhere(signed[N].url)`; 404/410 on content → `Expired`.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p agent --test media_endpoints && cargo test -p agent --test media_redirects`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/agent/src/client.rs crates/agent/src/media/openrouter.rs crates/agent/tests/media_endpoints.rs crates/agent/tests/media_redirects.rs
git commit -m "Studio: OpenRouter video jobs, checked and downloaded without following foreign links"
```

### Task A6: Guarded download of a result file (no key)

**Files:**
- Modify: `crates/search/src/fetch.rs`, `crates/search/Cargo.toml` (`tempfile` dev-dependency, already in the lock)
- Test: `crates/search/tests/fetch.rs`

**Interfaces:**
- Produces:
```rust
pub const MEDIA_TYPES: &[&str] = &["image/png","image/jpeg","image/webp","image/gif","image/svg+xml","video/mp4","video/webm"];
pub struct FetchedFile { pub url: Url, pub content_type: String, pub len: u64, pub redirects: usize }
impl Fetcher {
    /// The same address rules as `get`, binary, streamed to `dest`, at most `cap` bytes; never sends a key.
    pub async fn fetch_to_file(&self, url: &Url, cap: u64, dest: &Path) -> Result<FetchedFile, FetchError>;
}
// FetchError gains: NotMedia(String), TooLarge(u64), DiskFull, Disk(String)   (DiskFull = ErrorKind::StorageFull)
```

- [ ] **Step 1: Write the failing tests** (`crates/search/tests/fetch.rs`, with `with_test_loopback_ports`)
```rust
#[tokio::test] async fn fetch_to_file_saves_a_video() { /* 200 video/mp4 → file bytes equal, len matches */ }
#[tokio::test] async fn fetch_to_file_never_sends_authorization() { /* recorded request has no Authorization header */ }
#[tokio::test] async fn fetch_to_file_refuses_a_private_address() {
    let err = Fetcher::new("t").fetch_to_file(&url("http://127.0.0.1:9/v.mp4"), 1 << 20, &dest).await.unwrap_err();
    assert!(err.is_refusal()); assert!(!dest.exists());
}
#[tokio::test] async fn fetch_to_file_refuses_a_page() { /* text/html → FetchError::NotMedia("text/html"), no file */ }
#[tokio::test] async fn fetch_to_file_stops_at_the_cap() { /* 4 KiB body, cap 1 KiB → TooLarge(1024), no file */ }
#[tokio::test] async fn fetch_to_file_follows_redirects_by_hand() { /* allowed port → allowed port: saved; → 10.0.0.1: Blocked */ }
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p search --test fetch fetch_to_file`
Expected: FAIL to compile, `no method named 'fetch_to_file'`.

- [ ] **Step 3: Implement** by reusing `check`, `client_for` and `redirect_target` per hop, with `Accept: <MEDIA_TYPES joined>` and `Accept-Encoding: identity`; the media type must be in `MEDIA_TYPES`; chunks go to `dest.with_extension("part")`, renamed to `dest` at the end, removed on any error; `std::io::ErrorKind::StorageFull` → `DiskFull`, other write errors → `Disk`. The caller sets the time limit with `with_timeout` (Studio uses 10 minutes).

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p search --test fetch`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/search/src/fetch.rs crates/search/Cargo.toml crates/search/tests/fetch.rs Cargo.lock
git commit -m "Studio: download a result from a company's file host through the guarded fetcher"
```

### Task A7: OpenAI pictures

**Files:**
- Create: `crates/agent/src/media/openai.rs`, `crates/agent/src/media/multipart.rs`, `crates/agent/src/media/specs/openai_images.json`
- Create: `crates/agent/tests/fixtures/media/openai-models.json`, `openai-generation.json`, `openai-moderation-blocked.json`, `openai-org-verification.json` (shapes from OpenAI's API reference; replaced by recorded answers in the owner's real run)
- Modify: `crates/agent/src/cloud.rs` (`openai_media_kind`), `crates/agent/src/media/mod.rs` (`media_provider` covers OpenAI), `crates/agent/src/media/spec.rs` (`parse_catalog` for OpenAI), `crates/agent/src/media/price.rs` (`openai_cost`)
- Test: inline (`openai.rs`, `multipart.rs`, `cloud.rs`) and `crates/agent/tests/media_endpoints.rs`

**Interfaces:**
- Produces:
```rust
// cloud.rs: pub(crate) fn openai_media_kind(id: &str) -> Option<MediaKind>;
pub struct OpenAiImages; impl OpenAiImages { pub fn new(client: OpenAiClient) -> Self; pub fn with_limits(self, l: MediaLimits) -> Self; }
pub struct OpenAiRates { pub text_input: Option<f64>, pub image_input: f64, pub image_output: f64 }   // dollars per 1M tokens, per model entry
pub fn openai_cost(usage: &Value, rates: &OpenAiRates) -> Option<f64>;   // None when usage is missing, or text was sent and no text rate is stated
pub(crate) fn openai_failure(status: u16, headers: &HeaderMap, body: &[u8], key: Option<&str>) -> MediaFailure;
pub fn check_size(size: &str, rules: &SizeRules) -> Result<(), SizeProblem>;   // SizeProblem: NotWxH, MultipleOf, Ratio, LongestSide, TooFewPixels, TooManyPixels
// multipart.rs: pub(crate) fn form(fields: &[(&str, String)], files: &[FilePart]) -> (String, Vec<u8>);   // (boundary, body)
```
`openai_images.json` shape: `{"checked":"2026-11-17","sources":[…],"models":[{"prefix","label","note","retires","maxN","maxRefs","qualities","formats","svg","gif","rates":{"textInput","imageInput","imageOutput"},"perImage":{"<quality>|<size>":usd}}]}`, led by `gpt-image-2.6-sunburst` and `gpt-image-2.6-flare`, then `gpt-image-2` (retires `2027-02-10`), `gpt-image-1.5`, `gpt-image-1-mini`, `chatgpt-image-latest` (retire `2026-12-01`). Rates as https://developers.openai.com/api/docs/pricing states them on 2026-11-17: both 2.6 models text input $5.00, image input $8.00, image output $30.00 per 1M tokens (standard); `textInput` is `null` where the page states none. `perImage` holds only per-picture figures OpenAI states (the image guide gives gpt-image-2 at 1024²: low $0.006, medium $0.053, high $0.211; none for 2.6), so a 2.6 run has no estimate, says "price unknown" and asks first, and its actual cost is worked out from `usage` afterwards.

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn openai_media_kind_is_the_reverse_of_chat() {
    for id in ["gpt-image-2.6-flare", "gpt-image-2.6-sunburst", "gpt-image-1.5", "chatgpt-image-latest"] {
        assert_eq!(openai_media_kind(id), Some(MediaKind::Image)); assert!(!openai_can_chat(id));
    }
    for id in ["gpt-5.5", "dall-e-3", "sora-2"] { assert_eq!(openai_media_kind(id), None); }
}
#[test] fn the_bundled_spec_leads_with_2_6() {
    let s = bundled(); assert_eq!(s.checked, "2026-11-17");
    assert_eq!([s.models[0].prefix.as_str(), s.models[1].prefix.as_str()], ["gpt-image-2.6-sunburst", "gpt-image-2.6-flare"]);
}
#[test] fn the_longest_prefix_wins_and_a_new_model_gets_basic_fields() {
    assert_eq!(match_spec("gpt-image-2.6-flare-2026-11-04").unwrap().prefix, "gpt-image-2.6-flare");
    let m = model_for("gpt-image-3");
    assert_eq!(keys(&m), ["size", "quality", "n"]);
    assert!(m.notes.contains(&"Settings may not match this new model.".to_string()));
}
#[test] fn custom_sizes_are_checked() {
    assert!(check_size("1536x1024", &OPENAI_SIZE_RULES).is_ok());
    assert!(check_size("3840x2160", &OPENAI_SIZE_RULES).is_ok());
    assert_eq!(check_size("1000x1000", &OPENAI_SIZE_RULES), Err(SizeProblem::MultipleOf));
    assert_eq!(check_size("3840x1024", &OPENAI_SIZE_RULES), Err(SizeProblem::Ratio));
    assert_eq!(check_size("4096x2048", &OPENAI_SIZE_RULES), Err(SizeProblem::LongestSide));
    assert_eq!(check_size("512x512", &OPENAI_SIZE_RULES), Err(SizeProblem::TooFewPixels));
}
#[test] fn svg_is_one_picture_and_never_an_edit() { /* validate n=2 + svg → Err; svg + a reference → Err */ }
#[test] fn a_multipart_body_has_each_part() { /* boundary not in any part; "Content-Disposition: form-data; name=\"image[]\"; filename=\"picture-1.png\"" */ }
#[test] fn cost_from_usage() {
    let flare = match_spec("gpt-image-2.6-flare").unwrap().rates.clone();
    assert_eq!(flare, OpenAiRates { text_input: Some(5.0), image_input: 8.0, image_output: 30.0 });
    let u = json!({"input_tokens": 1050, "input_tokens_details": {"text_tokens": 50, "image_tokens": 1000}, "output_tokens": 2000});
    assert!((openai_cost(&u, &flare).unwrap() - 0.06825).abs() < 1e-9);       // 50×5 + 1000×8 + 2000×30, per 1e6
    let no_text_rate = OpenAiRates { text_input: None, ..flare };
    assert_eq!(openai_cost(&u, &no_text_rate), None);
    assert_eq!(openai_cost(&json!({}), &flare), None);
}
// media_endpoints.rs
#[tokio::test] async fn an_openai_generation_is_one_json_post() { /* POST /v1/images/generations; body model/prompt/n/size/quality; data[0].b64_json decoded */ }
#[tokio::test] async fn an_openai_edit_is_one_multipart_post() { /* with a reference: POST /v1/images/edits, content-type multipart/form-data; boundary=…, 1 request */ }
#[tokio::test] async fn moderation_blocked_is_explained() {
    // 400 {"error":{"code":"moderation_blocked","message":"…","moderation_details":{…}}} + x-request-id req_8f2
    // → Answered(f): f.kind == Blocked, f.company_code == Some("moderation_blocked"), f.request_id == Some("req_8f2")
}
#[tokio::test] async fn organization_verification_is_named() { /* 403 "…organization must be verified…" → kind OrgVerification */ }
#[tokio::test] async fn the_openai_list_keeps_picture_models_only() { /* /v1/models fixture → ids gpt-image-2.6-flare, gpt-image-2.6-sunburst */ }
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p agent --lib media && cargo test -p agent --lib cloud::tests::openai_media_kind && cargo test -p agent --test media_endpoints openai`
Expected: FAIL to compile, `cannot find type 'OpenAiImages'`.

- [ ] **Step 3: Implement.** `fetch_catalog` = `GET {api}/models` (key needed) as `RawCatalog{images: list, videos: None}`; `parse_catalog(OpenAi, …)` keeps ids with `openai_media_kind == Some(Image)` and builds params from the longest matching prefix: `size` (enum `auto`, `1024x1024`, `1536x1024`, `1024x1536`, plus Size rules in Advanced), `quality`, `n` (1–`maxN`, chips stop at 4), `output_format` (adds `svg`/`gif` when the entry says so), `output_compression` (requires jpeg/webp), `background`, `moderation` (Auto or Low; help "Low makes OpenAI's filter less strict; it still refuses what its rules forbid."), `frame_count`/`frame_duration_ms` (gif), `image[]` references up to `maxRefs`. A retiring model gets the note "OpenAI retires this model on <date>." Price: `PerImage` rows from `perImage`, else `Unknown`. With no references the run is `POST {api}/images/generations` (JSON); with references, `POST {api}/images/edits` (multipart, files as `image[]`). Actual cost: `openai_cost(usage, entry.rates)`, `reported: false` (`None` leaves the run counted at its estimate, or $5 when it has none). `openai_failure`: `moderation_blocked` → `Blocked` (stage from `moderation_details` when it says input or output, else none; `charged: None`); 403 whose message mentions organization verification → `OrgVerification`; 401/402/429/5xx as in Task A4; `request_id` from `x-request-id`.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p agent`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/agent/src crates/agent/tests
git commit -m "Studio: OpenAI pictures and edits from a dated settings file"
```

### Task A8: Storage (schema V3)

**Files:**
- Modify: `crates/store/src/schema.rs` (V3 as in the design §5, appended, amended here because nothing has shipped: two columns on `creation_runs`, `upstream TEXT` (the company OpenRouter ran it at, when known) and `deleted_at INTEGER` (a spending stub, see below); the local addendum's columns and status (L4: `runs_on`, `gen_model_id`, `engine_build`, `device`, `elapsed_ms`, and `'running'` in the status CHECK and in `creation_runs_live`); and its two tables (L5), below), `crates/store/src/ids.rs` (`update_columns_by_text`), `crates/store/src/lib.rs`, `crates/store/src/error.rs` (`TooManyCreations`)

The V3 additions for local runs, exactly:
```sql
-- on creation_runs
runs_on      TEXT NOT NULL DEFAULT 'cloud' CHECK (runs_on IN ('cloud','local')),
gen_model_id TEXT,  engine_build INTEGER,  device TEXT,  elapsed_ms INTEGER,
-- status CHECK gains 'running'; creation_runs_live: WHERE status IN ('queued','sending','waiting','running','downloading')
CREATE TABLE gen_models (
  id          TEXT PRIMARY KEY,                    -- the catalog id, or "hf:<owner>/<repo>:<file>" for a searched model
  catalog_id  TEXT,
  name        TEXT NOT NULL,
  kind        TEXT NOT NULL CHECK (kind IN ('image','image-edit','video')),
  family      TEXT NOT NULL,
  parts_json  TEXT NOT NULL CHECK (json_valid(parts_json)),   -- [{role, repo, revision, path, file, size, sha256, license}]
  size_bytes  INTEGER NOT NULL CHECK (size_bytes >= 0),
  source      TEXT NOT NULL CHECK (source IN ('catalog','search')),
  added_at    INTEGER NOT NULL
);
CREATE TABLE license_acceptances (
  sha256      TEXT NOT NULL,  license_id TEXT NOT NULL,  catalog_id TEXT,
  accepted_at INTEGER NOT NULL,
  PRIMARY KEY (sha256, catalog_id)
);
```
A local run has `provider_id` NULL, `provider_name` "This computer" and `provider_host` ''; `runs_on` keeps "endpoint deleted" and "local" apart. The `gen_models` accessors come in Task L9.
- Create: `crates/store/src/creations.rs`
- Test: inline in `schema.rs` and `creations.rs`

**Interfaces:**
- Produces (`store`):
```rust
sql_enum!(
    /// What a Studio run makes.
    CreationKind, "creation kind" {
        /// A picture.
        Image => "image",
        /// A video.
        Video => "video",
    }
);
sql_enum!(
    /// Where a Studio run is.
    CreationStatus, "creation status" {   // each variant with its own `///` line, as above
        Queued => "queued", Sending => "sending", Waiting => "waiting", Running => "running", Downloading => "downloading",
        Done => "done", Failed => "failed", Blocked => "blocked", Cancelled => "cancelled", Interrupted => "interrupted",
    }
);   // Running: a local run the engine is working on (cloud runs never use it)
sql_enum!(
    /// Where a Studio run ran.
    RunsOn, "runs on" {
        /// A company's computers, through an endpoint.
        Cloud => "cloud",
        /// This computer, with the Studio engine.
        Local => "local",
    }
);
// ids.rs: the same as update_columns, keyed by a TEXT id (creation ids are uuids)
pub(crate) fn update_columns_by_text(conn: &rusqlite::Connection, table: &'static str, key: &'static str, id: &str,
    sets: Vec<(&'static str, rusqlite::types::Value)>) -> Result<usize, StoreError>;
pub const MAX_CREATION_RUNS: usize = 20_000;                    // counts runs that are not stubs
pub struct NewCreationRun { pub id: String, pub kind: CreationKind, pub runs_on: RunsOn, pub provider_id: Option<ProviderId>, pub provider_name: String,
    pub provider_host: String, pub model: String, pub model_label: String, pub prompt: String, pub params: Value, pub refs: Value, pub estimate_usd: Option<f64>,
    pub gen_model_id: Option<String>, pub engine_build: Option<u32>, pub device: Option<String> }
pub struct CreationPatch { pub status: Option<CreationStatus>, pub remote_id: Option<Option<String>>, pub progress: Option<Option<f64>>,
    pub cost_usd: Option<Option<f64>>, pub error: Option<Option<Value>>, pub audit_id: Option<Option<AuditId>>, pub upstream: Option<Option<String>>,
    pub started_at: Option<Option<i64>>, pub finished_at: Option<Option<i64>>, pub elapsed_ms: Option<Option<u64>>, pub device: Option<Option<String>> }
pub struct CreationOutput { pub id: String, pub run_id: String, pub idx: u32, pub path: String, pub thumb_path: Option<String>, pub mime: String,
    pub size_bytes: u64, pub width: Option<u32>, pub height: Option<u32>, pub duration_ms: Option<u64>, pub seed: Option<u64>, pub starred: bool, pub created_at: i64 }
pub struct NewCreationOutput { /* CreationOutput without starred and created_at */ }
pub struct CreationRun { /* every creation_runs column, params/refs/error as Value */ pub outputs: Vec<CreationOutput> }
pub struct CreationQuery { pub kind: Option<CreationKind>, pub starred: bool, pub text: Option<String>, pub model: Option<String>,
    pub since: Option<i64>, pub until: Option<i64>, pub before: Option<i64>, pub limit: u32 }
pub struct CreationUsage { pub pictures: u64, pub videos: u64, pub bytes: u64 }
impl Store {
    pub async fn insert_creation_run(&self, run: NewCreationRun) -> Result<(), StoreError>;     // TooManyCreations at the cap
    pub async fn update_creation_run(&self, id: &str, patch: CreationPatch) -> Result<(), StoreError>;
    pub async fn get_creation_run(&self, id: &str) -> Result<Option<CreationRun>, StoreError>;
    pub async fn list_creation_runs(&self, q: CreationQuery) -> Result<Vec<CreationRun>, StoreError>;  // newest first
    pub async fn live_creation_runs(&self) -> Result<Vec<CreationRun>, StoreError>;           // queued, sending, waiting, running, downloading
    pub async fn creation_runs_since(&self, since_ms: i64) -> Result<Vec<CreationRun>, StoreError>;   // stubs included: spending reads this
    pub async fn insert_creation_output(&self, out: NewCreationOutput) -> Result<(), StoreError>;
    pub async fn get_creation_output(&self, id: &str) -> Result<Option<CreationOutput>, StoreError>;
    pub async fn set_creation_thumb(&self, id: &str, thumb_path: &str) -> Result<(), StoreError>;
    pub async fn set_creation_starred(&self, id: &str, starred: bool) -> Result<(), StoreError>;
    /// Deletes outputs; a finished run left with none is deleted too. A deleted run created at or after
    /// `stub_since_ms` (the start of today) stays as a stub, everything else goes.
    pub async fn delete_creation_outputs(&self, ids: Vec<String>, stub_since_ms: i64) -> Result<Deleted, StoreError>;
    pub async fn delete_creation_runs(&self, ids: Vec<String>, stub_since_ms: i64) -> Result<Deleted, StoreError>;
    pub async fn purge_creation_stubs(&self, before_ms: i64) -> Result<usize, StoreError>;      // at start-up: yesterday's stubs go
    pub async fn unfinished_creation_run_ids(&self) -> Result<Vec<String>, StoreError>;      // failed, blocked, cancelled, interrupted
    pub async fn creation_ref_hashes(&self) -> Result<BTreeSet<String>, StoreError>;          // sha256 of refs used by runs that are not stubs
    pub async fn creation_usage(&self) -> Result<CreationUsage, StoreError>;
}
pub struct Deleted { pub outputs: Vec<CreationOutput>, pub runs: Vec<CreationRun> }  // for removing files, sidecars and refs
```
A **stub** keeps `id`, `kind`, `provider_*`, `model`, `model_label`, `status`, `remote_id`, `estimate_usd`, `cost_usd`, `error_json` (so `charged` survives), `audit_id`, `created_at`, `finished_at`, and clears `prompt` (`''`), `params_json` (`'{}'`), `refs_json` (`'[]'`), `upstream`; `deleted_at` is set. Every read except `creation_runs_since` and `purge_creation_stubs` leaves stubs out.

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn version_3_adds_the_studio_tables_to_a_version_2_database() {
    // Migrations(&[V1, V2]).to_latest; insert a provider and a chat; MIGRATIONS.to_latest;
    // user_version == 3; both tables exist; the provider and chat rows are unchanged
}
#[tokio::test] async fn deleting_an_endpoint_keeps_its_runs_with_their_names() {
    // run with provider_id 1; delete_provider(1); run.provider_id == None; provider_name and provider_host unchanged
}
#[tokio::test] async fn deleting_a_run_deletes_its_outputs() { /* delete_creation_runs → outputs gone, returned for file removal */ }
#[tokio::test] async fn a_status_outside_the_list_is_refused() { /* raw UPDATE status = 'generating' fails the CHECK */ }
#[tokio::test] async fn runs_list_newest_first_with_filters() {
    // kind, starred (a run with a starred output), text (LIKE with % and _ escaped), model, before cursor, limit
}
#[tokio::test] async fn live_runs_are_the_five_working_states() {
    // one run per status → live_creation_runs() statuses == {queued, sending, waiting, running, downloading}
}
#[test] fn version_3_has_the_local_columns_and_tables() {
    // after to_latest: an INSERT without runs_on reads back 'cloud'; runs_on = 'gpu' fails the CHECK;
    // tables gen_models and license_acceptances exist; gen_models.kind = 'audio' fails the CHECK
}
#[tokio::test] async fn a_local_run_has_no_endpoint_and_keeps_its_engine_facts() {
    // NewCreationRun { runs_on: Local, provider_id: None, provider_name: "This computer", provider_host: "",
    //   gen_model_id: Some("flux2-klein-4b-q4"), engine_build: Some(1106), device: Some("vulkan0"), .. }
    // update elapsed_ms Some(228_000) → get_creation_run: runs_on Local, engine_build Some(1106), elapsed_ms Some(228_000)
}
#[tokio::test] async fn the_last_output_takes_its_finished_run_along() {
    // done run with 2 outputs: deleting 1 keeps the run; deleting the other → get_creation_run(id) == None
}
#[tokio::test] async fn deleting_todays_run_keeps_a_spending_stub_without_the_prompt() {
    let d = store.delete_creation_runs(vec!["today".into(), "yesterday".into()], day_start).await.unwrap();
    assert_eq!(d.runs.len(), 2);
    let since = store.creation_runs_since(day_start).await.unwrap();
    assert_eq!(since.len(), 1);
    assert_eq!((since[0].prompt.as_str(), since[0].cost_usd), ("", Some(9.5)));
    assert!(store.get_creation_run("today").await.unwrap().is_none());
    assert!(store.list_creation_runs(CreationQuery::default()).await.unwrap().is_empty());
    assert_eq!(store.purge_creation_stubs(day_start + DAY_MS).await.unwrap(), 1);
}
#[tokio::test] async fn the_cap_is_twenty_thousand_runs() {
    assert_eq!(MAX_CREATION_RUNS, 20_000);
    // 20,000 rows through one INSERT … SELECT over a recursive CTE, then insert_creation_run → Err(StoreError::TooManyCreations)
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p store creations && cargo test -p store schema`
Expected: FAIL to compile, `no method named 'insert_creation_run'`.

- [ ] **Step 3: Implement** with the existing helpers (`sql_enum!`, `JsonColumn`, `json_text`, `now_ms`, `like_contains`, `sql_limit`) and the new `update_columns_by_text` (the existing `update_columns` binds an `i64` key). The insert checks the count inside the same transaction. JSON columns are written with `json_text` so `json_valid` holds. A stub's outputs are deleted explicitly (the row stays, so the cascade does not fire).

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p store`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/store/src
git commit -m "Studio: schema V3 with runs and their outputs"
```

### Task A9: The gallery folder and added pictures

**Files:**
- Create: `src-tauri/src/studio/gallery.rs`, `src-tauri/src/studio/test_support.rs` (`#[cfg(test)]`: `Scratch` (temp dir + uuid, removed on Drop, as in `downloads.rs`; the app crate has no `tempfile`), `png(w, h)`, `mp4()`)
- Modify: `src-tauri/src/studio/mod.rs`, `src-tauri/src/attachments.rs` (`sniff`, `checked_picture`, `PictureWorker`, `ImageInput` become `pub(crate)`)
- Test: inline in `gallery.rs`

**Interfaces:**
- Consumes: `attachments::{checked_picture, PictureWorker, ImageInput, MAX_IMAGE_BYTES}`, `store::CreationRun`.
- Produces:
```rust
pub struct Gallery { root: PathBuf }
impl Gallery {
    pub fn new(root: PathBuf) -> Self;
    pub fn default_root(home: &Path, app_name: &str) -> PathBuf;                  // <home>/<app_name>/studio
    pub fn partial_dir(&self) -> PathBuf;                                         // <root>/.partial (created)
    pub fn output_path(&self, created_ms: i64, run_id: &str, idx: u32, ext: &str) -> PathBuf;   // <root>/YYYY-MM/<run>-<idx>.<ext>
    pub fn sidecar_path(&self, created_ms: i64, run_id: &str) -> PathBuf;          // <root>/YYYY-MM/<run>.json
    pub fn thumb_path(&self, output_id: &str) -> PathBuf;                         // <root>/.thumbs/<id>.jpg
    pub fn finalize(&self, partial: &Path, claimed: Option<&str>, target_no_ext: &Path, cap: u64) -> Result<(PathBuf, String), GalleryError>;
    pub fn write_sidecar(&self, run: &store::CreationRun) -> Result<(), GalleryError>;
    pub async fn add_ref(&self, worker: &PictureWorker, img: &ImageInput) -> AppResult<RefStored>;
    pub fn ref_file(&self, sha256: &str) -> Result<(PathBuf, String), GalleryError>;   // refuses a name that is not 64 hex characters
    pub fn remove_files(&self, deleted: &store::Deleted);                         // outputs, thumbs, and sidecars of deleted runs
    pub fn remove_unused_refs(&self, candidates: &BTreeSet<String>, in_use: &BTreeSet<String>);  // refs of deleted runs no run still uses
    pub fn refs_bytes(&self) -> u64;                                              // counted in Settings → Studio's usage line
}
pub struct RefStored { pub sha256: String, pub mime: String, pub size_bytes: u64 }
pub fn sniff_media(bytes: &[u8]) -> Option<&'static str>;
pub enum GalleryError { DiskFull, TooLarge(u64), NotMedia, Mismatch { claimed: String, found: String }, BadName, Io(String) }
pub fn io_error(e: std::io::Error) -> GalleryError;                               // StorageFull → DiskFull
```

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn files_go_under_their_month() {
    let g = Gallery::new("/g".into());
    assert_eq!(g.output_path(ms(2026, 11, 17), "r1", 0, "png"), Path::new("/g/2026-11/r1-0.png"));
    assert_eq!(g.sidecar_path(ms(2026, 11, 17), "r1"), Path::new("/g/2026-11/r1.json"));
}
#[test] fn the_default_folder_is_next_to_the_models() {
    assert_eq!(Gallery::default_root(Path::new("/home/u"), "harnessUI"), Path::new("/home/u/harnessUI/studio"));
}
#[test] fn the_type_is_read_from_the_bytes() {
    assert_eq!(sniff_media(PNG), Some("image/png"));   // also JPEG, WebP, GIF, SVG, MP4 (ftyp at 4), WebM (1A 45 DF A3)
    assert_eq!(sniff_media(b"<!doctype html><p>"), None);
}
#[test] fn a_file_that_is_not_what_it_claims_is_refused_and_removed() {
    // partial holds HTML, claimed "video/mp4" → Err(NotMedia); partial no longer exists
    // partial holds PNG, claimed "video/mp4" → Err(Mismatch{..})
}
#[test] fn a_full_disk_is_named() {
    assert!(matches!(io_error(std::io::Error::from(std::io::ErrorKind::StorageFull)), GalleryError::DiskFull));
}
#[test] fn the_sidecar_is_the_run_record() { /* JSON has id, prompt, params, outputs; no "Authorization", no "api_key" */ }
#[test] fn a_reference_name_that_is_not_a_hash_is_refused() {
    assert!(matches!(Gallery::new(Scratch::new().path().into()).ref_file("../../etc/passwd"), Err(GalleryError::BadName)));
}
#[tokio::test] async fn references_are_kept_once_by_hash() { /* add_ref twice → same refs/<sha>.png, one file */ }
#[test] fn a_reference_no_run_uses_any_more_is_removed() {
    // refs a and b on disk; candidates {a, b}, in_use {b} → a removed, b kept; refs_bytes() == size of b
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::gallery`
Expected: FAIL to compile, `cannot find type 'Gallery'`.

- [ ] **Step 3: Implement.** `finalize` sniffs the first bytes, refuses a claimed type that differs (a WebP called `image/png` is a mismatch too), refuses anything outside the seven types, renames into `<target>.<ext>` (ext from the sniffed type) and removes the partial on every refusal. `add_ref` runs `checked_picture` (20 MiB, sniff, WebP → JPEG/PNG) and writes `refs/<sha>.<ext>` through a `.tmp` and rename, like `save_images`.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio::gallery`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio src-tauri/src/attachments.rs
git commit -m "Studio: gallery folder layout, type checks, sidecars and added pictures"
```

### Task A10: Thumbnails in the picture worker

**Files:**
- Modify: `src-tauri/src/attachments.rs` (thumbnail mode), `src-tauri/src/main.rs` (pass the mode argument)
- Test: inline in `attachments.rs` (its existing `test_worker` helper)

**Interfaces:**
- Produces:
```rust
pub const THUMB_MODE: &str = "thumb";
pub const THUMB_SIDE: u32 = 512;
pub fn picture_worker_main(mode: Option<&str>) -> !;             // None: today's WebP conversion; Some("thumb"): a thumbnail
pub(crate) fn thumbnail(bytes: &[u8]) -> Result<Vec<u8>, NotConverted>;   // PNG, JPEG or WebP in, JPEG out, longest side 512
impl PictureWorker { pub(crate) fn thumbs() -> Self; pub(crate) async fn thumbnail(&self, bytes: &[u8]) -> Result<Vec<u8>, NotConverted>; }
```

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn a_thumbnail_is_a_jpeg_at_most_512_px() {
    let jpg = thumbnail(&png(1536, 1024)).unwrap();
    assert!(jpg.starts_with(&[0xFF, 0xD8, 0xFF]));
    let (w, h) = image::load_from_memory(&jpg).unwrap().dimensions();
    assert_eq!((w, h), (512, 341));
}
#[test] fn a_small_picture_is_not_enlarged() { /* 300x200 → 300x200 */ }
#[tokio::test] async fn the_worker_makes_thumbnails() {
    let jpg = test_worker("thumb").thumbnail(&png(1536, 1024)).await.unwrap();
    assert!(jpg.starts_with(&[0xFF, 0xD8, 0xFF]));
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib attachments::tests::a_thumbnail`
Expected: FAIL to compile, `cannot find function 'thumbnail'`.

- [ ] **Step 3: Implement.** The worker keeps its 1 GiB memory cap and 60 s limit; in thumbnail mode it reads at most 50 MiB, decodes with the existing size checks (at most 8,192 px a side), resizes with `image::imageops::thumbnail`, and writes JPEG at quality 80 after `PICTURE_MARK`. `main.rs` passes `args.next().as_deref()`. GIF and SVG get no thumbnail (the grid shows the original). The test binary takes its mode from `HARNESS_TEST_PICTURE_WORKER`, not argv (libtest owns argv), so `picture_worker_process` gains `Ok("thumb") => picture_worker_main(Some(THUMB_MODE))` and its other arms become `picture_worker_main(None)`; `PictureWorker::thumbs()` adds `thumb` to argv only for the real binary.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib attachments && bash scripts/check-spawn.sh`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/attachments.rs src-tauri/src/main.rs
git commit -m "Studio: 512 px thumbnails made in the picture worker"
```

### Task A11: Saved model lists (Test, Update, delete)

**Files:**
- Create: `src-tauri/src/studio/catalog.rs`
- Modify: `src-tauri/src/providers.rs` (`test_provider` also refreshes media lists, failures only logged; `delete_provider` calls `catalog::forget`; `may_hold_key` `pub(crate)`; the client-building half of `client_for` becomes `pub(crate) fn client_from(provider: &store::Provider, key: Option<String>) -> AppResult<OpenAiClient>` so Task A13's `StoreProviders` builds clients the same way)
- Test: inline in `catalog.rs`

**Interfaces:**
- Consumes: `agent::media::{MediaProvider, RawCatalog, parse_catalog, media_company}` (`media_company` re-exported from `agent::cloud` in Task A1), `store::Store` settings.
- Produces:
```rust
pub fn cache_key(id: store::ProviderId) -> String;                        // "providerMediaModels:<id>"
#[derive(Serialize, Deserialize)] pub struct CachedCatalog { pub fetched_at: i64, pub raw: RawCatalog, pub image_prices: BTreeMap<String, CachedPrice> }
pub struct CachedPrice { pub fetched_at: i64, pub raw: Value }
pub async fn refresh(store: &store::Store, id: store::ProviderId, media: &dyn MediaProvider, now: i64) -> Result<(), MediaError>;
pub async fn refresh_price(store: &store::Store, id: store::ProviderId, media: &dyn MediaProvider, model: &str, now: i64) -> Result<(), MediaError>;
pub async fn forget(store: &store::Store, id: store::ProviderId);
pub struct EndpointModels { pub provider: store::Provider, pub company: Company, pub host: String, pub cloud: bool,
    pub has_key: bool, pub fetched_at: Option<i64>, pub models: Vec<PricedModel> }
pub struct PricedModel { pub model: MediaModel, pub price_fetched_at: Option<i64>, pub endpoints: Option<Value> }  // the saved `/endpoints` answer
/// From the saved lists only: never builds a client or touches the network.
pub async fn models_for(store: &store::Store, providers: &[store::Provider], has_key: impl Fn(store::ProviderId) -> bool) -> Vec<EndpointModels>;
pub async fn model(store: &store::Store, id: store::ProviderId, model: &str) -> Option<(EndpointModels, PricedModel)>;
```

- [ ] **Step 1: Write the failing tests**
```rust
#[tokio::test] async fn models_come_from_the_saved_lists_only() {
    // save CachedCatalog from the two fixtures for an "openrouter" provider; models_for → 101 models, fetched_at Some
}
#[tokio::test] async fn an_endpoint_without_a_list_offers_to_get_it() { /* fetched_at None, models empty */ }
#[tokio::test] async fn only_openrouter_and_openai_endpoints_are_listed() { /* anthropic, generic, ollama left out */ }
#[tokio::test] async fn a_saved_picture_price_is_used() { /* flux-3-image's recorded /endpoints answer saved → model.price is PerImage */ }
#[tokio::test] async fn a_failed_refresh_keeps_the_old_list() { /* fake provider fails → Err, cache unchanged */ }
#[tokio::test] async fn forget_removes_the_list() { /* refresh, forget → get_setting(cache_key(id)) is None; models_for → fetched_at None */ }
#[tokio::test] async fn a_picture_price_says_when_it_was_fetched() {
    // refresh_price at 1000 → that model's price_fetched_at Some(1000); another picture model None; a video model = the list's fetched_at
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::catalog`
Expected: FAIL to compile.

- [ ] **Step 3: Implement.** `models_for` keeps providers whose `kind_of(kind)` has a `media_company`; `host` from `agent::endpoint_host`, `cloud` from `endpoint_place == Internet`. In `test_provider`, after the chat list is saved: `if let Some(media) = agent::media::media_provider(client.clone()) { if let Err(e) = catalog::refresh(..).await { tracing::warn!(..) } }`, so a media list failure never fails Test.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio::catalog && cargo test -p app --lib providers`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio/catalog.rs src-tauri/src/studio/mod.rs src-tauri/src/providers.rs
git commit -m "Studio: keep each endpoint's picture and video lists, fetched only on request"
```

### Task A12: Studio preferences

**Files:**
- Create: `src-tauri/src/studio/prefs.rs`
- Test: inline

**Interfaces:**
- Consumes: `spend::{CONFIRM_AT_USD, DEFAULT_DAILY_LIMIT_USD, ConfirmRules}`.
- Produces:
```rust
pub const PREFS_KEY: &str = "studioPrefs";
pub const MAX_SETUPS: usize = 50;
pub const DAILY_LIMIT_RANGE: RangeInclusive<f64> = 1.0..=1000.0;
pub const CONFIRM_AT_RANGE: RangeInclusive<f64> = 0.25..=100.0;
/// Where a run goes: an endpoint, or this computer. Serialised `{"endpoint": 3}` or `"thisComputer"`. Named
/// `RunTarget`, not `Target`, because `models::fit::Target` (the hardware) is used beside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub enum RunTarget { Endpoint(i64), ThisComputer }
/// What happens when a local job does not fit beside the loaded chat model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub enum GpuPolicy { #[default] Unload, Ask, Processor }
#[serde(rename_all = "camelCase", default)]
pub struct StudioPrefs { pub last_model: BTreeMap<MediaKind, ModelChoice>, pub advanced_open: bool, pub confirm_every_video: bool,
    pub confirm_at_usd: f64, pub daily_limit_usd: Option<f64>, pub setups: Vec<Setup>, pub gallery_dir: Option<String>,
    pub gpu_policy: GpuPolicy, pub reload_chat_after: bool, pub suggestion_dismissed: bool,
    pub local_notice_accepted_at: Option<i64> }       // set only by `studio_accept_local_notice` (Task L16), never by a patch
pub struct ModelChoice { pub target: RunTarget, pub model: String }
pub struct Setup { pub name: String, pub kind: MediaKind, pub target: RunTarget, pub model: String,
    pub params: Map<String, Value>, pub extra: Map<String, Value>, pub routing: Option<Routing> }
impl StudioPrefs { pub fn confirm_rules(&self) -> ConfirmRules; pub fn validate(&self) -> Result<(), String>;
    pub fn apply(&self, patch: StudioPrefsPatch) -> Result<StudioPrefs, String>; }   // only the fields sent change; then validate
#[serde(rename_all = "camelCase")]
pub struct StudioPrefsPatch { pub last_model: Option<BTreeMap<MediaKind, ModelChoice>>, pub advanced_open: Option<bool>,
    pub confirm_every_video: Option<bool>, pub confirm_at_usd: Option<f64>, pub daily_limit_usd: Option<Option<f64>>,
    pub setups: Option<Vec<Setup>>, pub gallery_dir: Option<Option<String>>,
    pub gpu_policy: Option<GpuPolicy>, pub reload_chat_after: Option<bool>, pub suggestion_dismissed: Option<bool> }   // Option<Option<_>>: absent = keep, null = clear
pub async fn load(store: &store::Store) -> StudioPrefs;                  // a missing or unreadable value gives the defaults
pub async fn save(store: &store::Store, prefs: &StudioPrefs) -> AppResult<()>;   // validates first
```

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn the_defaults_are_the_owners() {
    let p = StudioPrefs::default();
    assert_eq!(p.daily_limit_usd, Some(10.0));
    assert!(p.confirm_every_video);
    assert_eq!(p.confirm_at_usd, 1.0);
    assert_eq!(p.gpu_policy, GpuPolicy::Unload);
    assert!(!p.reload_chat_after && !p.suggestion_dismissed && p.local_notice_accepted_at.is_none());
}
#[test] fn targets_serialise_for_the_webview() {
    assert_eq!(serde_json::to_value(RunTarget::Endpoint(3)).unwrap(), json!({"endpoint": 3}));
    assert_eq!(serde_json::to_value(RunTarget::ThisComputer).unwrap(), json!("thisComputer"));
}
#[test] fn a_patch_cannot_accept_the_local_notice() {
    assert_eq!(StudioPrefs::default().apply(patch(json!({"localNoticeAcceptedAt": 1}))).unwrap().local_notice_accepted_at, None);
    assert_eq!(StudioPrefs::default().apply(patch(json!({"gpuPolicy": "ask"}))).unwrap().gpu_policy, GpuPolicy::Ask);
}
#[test] fn the_limit_is_between_1_and_1000_dollars_or_off() {
    for bad in [0.5, 1000.01, f64::NAN] { assert!(with_limit(Some(bad)).validate().is_err()); }
    for good in [Some(1.0), Some(1000.0), None] { assert!(with_limit(good).validate().is_ok()); }
}
#[test] fn setups_stop_at_50_and_need_a_name() {
    assert_eq!(with_setups(51).validate(), Err("Keep at most 50 setups.".into()));
    assert_eq!(with_setup_named("  ").validate(), Err("Give the setup a name.".into()));
}
#[test] fn a_patch_changes_only_the_fields_it_sends() {
    let p = StudioPrefs { daily_limit_usd: Some(25.0), ..Default::default() };
    let q = p.apply(patch(json!({"confirmEveryVideo": false}))).unwrap();
    assert_eq!((q.confirm_every_video, q.daily_limit_usd), (false, Some(25.0)));
    assert_eq!(p.apply(patch(json!({"dailyLimitUsd": null}))).unwrap().daily_limit_usd, None);
}
#[tokio::test] async fn prefs_saved_without_new_fields_read_with_defaults() {
    // store {"advancedOpen":true} → load: advanced_open true, daily_limit_usd Some(10.0)
}
#[tokio::test] async fn a_gallery_folder_must_exist_and_take_files() {
    // missing folder → Err "Pick an existing folder."; an existing folder named "shots %03d" → Err "Pick a folder whose name has no %d in it."
    // (sd-cli reads `-o` as a numbering template when it holds %d, %3d or %03d)
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::prefs`
Expected: FAIL to compile.

- [ ] **Step 3: Implement.** The folder check copies `hub::set_models_dir` (exists, `.write-test` probe) and refuses a path in which `%` is followed by up to three digits and `d` (the same pattern `runtime::sd::args` refuses in `-o`, Task L4). Changing the folder affects new results only; existing results keep their absolute paths and stay where they are (said on the settings page).

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio::prefs`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio
git commit -m "Studio: preferences with a \$10 daily limit, confirmation rules and the chat-model unload on by default"
```

### Task A13: StudioManager: starting runs, pictures, queue, cancel, audit

**Files:**
- Create: `src-tauri/src/studio/deps.rs`, `src-tauri/src/studio/manager.rs`, `src-tauri/src/studio/manager_tests.rs`, `src-tauri/src/studio/errors.rs`
- Modify: `src-tauri/src/dto.rs` (the run DTOs below)
- Test: `manager_tests.rs` (fakes: `FakeProvider` counting `start` calls with scripted answers, `FakeSink`, `FakeFetch`, `FixedClock`; `store::Store::open_in_memory()`; a `Scratch` gallery). The harness saves an OpenRouter catalogue from the fixtures plus flux-3-image's recorded `/endpoints` answer, so an OpenRouter picture run has a price and needs no confirmation under $1; `req_estimated(usd, token)` uses a test picture model whose saved `/endpoints` answer (same shape) prices one picture at `usd`; `openai_req()` uses an OpenAI catalogue whose 2.6 entry has no per-picture price.

**Interfaces:**
- Consumes: Tasks A1–A12, `store::{AuditEntry, AuditDecision}`.
- Produces:
```rust
// deps.rs
#[async_trait] pub trait ProviderSource: Send + Sync {
    async fn media(&self, id: store::ProviderId) -> AppResult<(Arc<dyn MediaProvider>, Endpoint)>;   // studio_no_endpoint / studio_no_key
}
pub struct Endpoint { pub id: Option<store::ProviderId>, pub name: String, pub host: String, pub company: Company }   // id None: this computer
pub trait StudioSink: Send + Sync { fn progress(&self, run: &RunDto); }
#[async_trait] pub trait ForeignFetch: Send + Sync { async fn fetch_to_file(&self, url: &Url, cap: u64, dest: &Path) -> Result<String, GalleryError>; }  // returns the content type
pub trait Clock: Send + Sync { fn now_ms(&self) -> i64; fn day_start_ms(&self, at_ms: i64) -> i64; }
pub struct StoreProviders { pub store: store::Store, pub secrets: Arc<dyn store::secrets::SecretStore> }   // reads the endpoint and its key like providers::client_for
pub struct AppSink(pub AppHandle); pub struct GuardedFetch(pub search::Fetcher); pub struct SystemClock;
// manager.rs
pub const PICTURES_AT_ONCE: usize = 2;
pub const VIDEOS_AT_ONCE: usize = 2;
pub struct StudioDeps { pub store: store::Store, pub providers: Arc<dyn ProviderSource>, pub sink: Arc<dyn StudioSink>,
    pub fetch: Arc<dyn ForeignFetch>, pub clock: Arc<dyn Clock>, pub default_gallery: PathBuf,
    pub thumbs: Option<PictureWorker>, pub limits: MediaLimits }          // Task A14 adds `poll: PollPlan`; Task L15 adds `local: Option<LocalLane>`
pub struct StudioManager { /* Arc<Inner>: prefs: RwLock<StudioPrefs>, running, queues, starting: tokio::sync::Mutex<()>, recent_tokens, live */ }
impl StudioManager {
    pub fn new(deps: StudioDeps, prefs: StudioPrefs) -> Self;
    pub fn gallery(&self) -> Gallery;                                     // prefs.gallery_dir, else default_gallery; read per run
    pub async fn estimate(&self, req: &StartRequest) -> AppResult<EstimateDto>;
    pub async fn preview(&self, req: &StartRequest) -> AppResult<RequestPreview>;
    pub async fn start(&self, req: StartRequest, confirmed: bool) -> AppResult<RunDto>;
    pub async fn cancel(&self, run_id: &str) -> AppResult<RunDto>;
    pub async fn live(&self) -> AppResult<Vec<RunDto>>;
    pub async fn run_dto(&self, run: &store::CreationRun) -> RunDto;
    pub async fn usage(&self) -> AppResult<UsageDto>;
}
pub fn spend_row(run: &store::CreationRun) -> SpendRow;   // status + error_json.charged through spend::charge_of
// errors.rs (each a four-part AppError; copy in plain English)
pub fn needs_confirmation(reasons: &[ConfirmReason]) -> AppError;             // "studio_needs_confirmation"
pub fn daily_limit(spent: f64, limit: f64) -> AppError;                       // "studio_daily_limit": "Daily limit reached: $9.50 of $10 spent today"
pub fn blocked(company: Company, stage: Option<Stage>, charged: Option<bool>, request_id: Option<&str>) -> AppError;  // "studio_blocked"
pub fn org_verification() -> AppError; pub fn no_endpoint() -> AppError; pub fn no_key(name: &str) -> AppError;
pub fn job_lost(company: Company) -> AppError; pub fn interrupted(company: Company) -> AppError; pub fn result_expired(company: Company) -> AppError;
pub fn disk_full() -> AppError; pub fn too_large(cap: u64) -> AppError; pub fn gallery_full() -> AppError;
pub fn not_sent(company: Company) -> AppError;                                // "studio_not_sent": "Not sent. Nothing was charged."
pub fn not_answered(company: Company, charged: Option<bool>) -> AppError;      // "studio_not_answered": timeouts and cut connections
pub fn stopped(company: Company, kind: MediaKind, charged: Option<bool>) -> AppError;   // "studio_stopped": the Cancel / Stop watching copy
pub fn download_failed(company: Company) -> AppError;                         // "studio_download_failed" (Task A14)
pub fn from_failure(company: Company, f: &MediaFailure) -> AppError;          // also the local kinds, in L2's words:
    // OutOfMemory "Ran out of graphics memory." / "Try 480p or a shorter clip."; DeviceLost "The graphics driver stopped responding.";
    // ModelFileBad "A model file is damaged." / "Download it again in Get local models."; EngineMissing "The Studio engine is missing or
    // was blocked." / "Download it again, or allow it in your antivirus."; EngineFailed "The Studio engine stopped." (codes studio_out_of_memory,
    // studio_device_lost, studio_model_file_bad, studio_engine_missing, studio_engine_failed)
pub fn from_local(e: &LocalIo, cap: u64) -> AppError;                         // DiskFull → disk_full, TooLarge → too_large
pub fn no_local_engine() -> AppError;                                         // "studio_no_local_engine": "Local models are not set up on this computer."
```
The run DTOs, in `src-tauri/src/dto.rs` (serde camelCase; Task B1 mirrors them in TS with the same names):
```rust
pub struct StartRequest { pub client_token: String, pub target: RunTarget, pub kind: MediaKind, pub model: String, pub prompt: String,
    pub params: Map<String, Value>, pub extra: Map<String, Value>, pub refs: Vec<RefInput>, pub routing: Option<Routing> }   // RunTarget: Task A12
pub struct RefInput { pub sha256: String, pub role: RefRole }
pub struct RefDto { pub sha256: String, pub mime: String, pub size_bytes: u64, pub role: Option<RefRole> }
pub struct RunErrorDto { pub code: String, pub message: String, pub next_step: String, pub details: String,
    pub charged: Option<bool>, pub request_id: Option<String>, pub company: String, pub stage: Option<String> }
pub struct OutputDto { pub id: String, pub run_id: String, pub idx: u32, pub mime: String, pub size_bytes: u64, pub width: Option<u32>,
    pub height: Option<u32>, pub duration_ms: Option<u64>, pub seed: Option<u64>, pub starred: bool, pub has_thumb: bool, pub path: String }
pub struct RunDto { pub id: String, pub kind: MediaKind, pub runs_on: String /* "cloud" | "local" */, pub provider_id: Option<i64>, pub provider_name: String,
    pub provider_host: String, pub upstream: Option<String>, pub model: String, pub model_label: String, pub prompt: String, pub params: Map<String, Value>,
    pub extra: Map<String, Value>, pub routing: Option<Routing>, pub refs: Vec<RefDto>, pub status: String, pub watching: bool, pub stale: bool,
    pub remote_id: Option<String>, pub progress: Option<f64>, pub estimate_usd: Option<f64>, pub cost_usd: Option<f64>, pub error: Option<RunErrorDto>,
    pub created_at: i64, pub started_at: Option<i64>, pub finished_at: Option<i64>, pub outputs: Vec<OutputDto>, pub queue_position: Option<u32>,
    pub eta: Option<EtaDto>, pub local: Option<LocalRunDto> }                  // both None for cloud runs; Task L15 fills them
pub struct EtaDto { pub low_ms: u64, pub high_ms: u64, pub measured: bool, pub left_ms: Option<u64> }   // left_ms: during a run
pub struct LocalRunDto { pub stage: Option<String> /* loading|readingPrompt|drawing|decoding|saving */, pub step: Option<u32>, pub steps: Option<u32>,
    pub picture: Option<u32>, pub pictures: Option<u32>, pub pass: Option<u32>, pub passes: Option<u32>,
    pub waiting_for: Option<String> /* "reply" | "studioJob" | "answer" */, pub unloaded_chat: Option<String> /* the chat model's path */,
    pub retrying: bool, pub stalled: bool, pub device: Option<String>, pub engine_build: Option<u32>, pub elapsed_ms: Option<u64>, pub labelled: bool,
    pub alongside: bool /* runs beside the loaded chat model */, pub on_processor: bool }
pub struct LimitDto { pub limit_usd: Option<f64>, pub spent_today_usd: f64, pub pending_usd: f64, pub would_pass: bool }
pub struct EstimateDto { pub usd: Option<f64>, pub reasons: Vec<ConfirmReason>, pub confirm_at_usd: f64, pub limit: LimitDto, pub invalid: Vec<String>,
    pub eta: Option<EtaDto>, pub local: Option<LocalEstimateDto> }            // None for cloud runs; Task L15 fills them
pub struct LocalEstimateDto { pub fit: String /* fits|slow|afterUnload|notHere */, pub unload_chat: bool, pub very_slow: bool, pub over_15_min: bool }
pub struct UsageDto { pub spent_today_usd: f64, pub pending_usd: f64, pub limit_usd: Option<f64>, pub folder: String,
    pub bytes: u64, pub pictures: u64, pub videos: u64 }
```

- [ ] **Step 1: Write the failing tests** (`manager_tests.rs`)
```rust
#[tokio::test] async fn a_picture_run_goes_from_queued_to_done_with_its_files() {
    let h = harness().await;                                     // fake answers Done with 2 PNGs, cost 0.04
    let run = h.manager.start(h.picture_req("lighthouse at dusk"), false).await.unwrap();
    let done = h.wait_status(&run.id, "done").await;
    assert_eq!(done.outputs.len(), 2);
    assert!(Path::new(&done.outputs[0].path).starts_with(h.gallery_root.join("2026-11")));
    assert_eq!(done.cost_usd, Some(0.04));
    assert_eq!(h.statuses(&run.id), ["queued", "sending", "done"]);
}
#[tokio::test] async fn a_video_without_confirmation_is_refused() {
    let err = h.manager.start(h.video_req(), false).await.unwrap_err();
    assert_eq!(err.code, "studio_needs_confirmation");
    assert_eq!(h.provider.starts(), 0); assert!(h.store_runs().await.is_empty());
}
#[tokio::test] async fn a_video_model_sent_as_a_picture_is_refused() {
    for confirmed in [false, true] {
        let err = h.manager.start(StartRequest { kind: MediaKind::Image, ..h.video_req() }, confirmed).await.unwrap_err();
        assert_eq!(err.code, "bad_request");
    }
    assert_eq!(h.provider.starts(), 0);
}
#[tokio::test] async fn a_run_over_the_daily_limit_is_refused_whatever_the_webview_says() {
    h.seed_done_run_costing(9.50).await;
    let err = h.manager.start(h.video_req_estimated(0.96), true).await.unwrap_err();
    assert_eq!(err.code, "studio_daily_limit");
    assert_eq!(err.message, "Daily limit reached: $9.50 of $10 spent today");
    assert_eq!(h.provider.starts(), 0);
}
#[tokio::test] async fn queued_and_running_estimates_count_against_the_limit() {
    // FakeProvider holds each start; two pictures at 4.50 each (confirmed) sit in `sending`
    let err = h.manager.start(h.req_estimated(1.50, "t3"), true).await.unwrap_err();
    assert_eq!(err.code, "studio_daily_limit"); assert_eq!(h.provider.starts(), 2);
}
#[tokio::test] async fn two_starts_at_once_cannot_both_pass_the_limit() {
    h.seed_done_run_costing(8.50).await;
    let (a, b) = tokio::join!(h.manager.start(h.req_estimated(1.00, "t1"), true), h.manager.start(h.req_estimated(1.00, "t2"), true));
    assert_eq!([a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count(), 1);
}
#[tokio::test] async fn the_same_create_press_twice_makes_one_run() {
    let a = h.manager.start(h.req_token("press-1"), false).await.unwrap();
    let b = h.manager.start(h.req_token("press-1"), false).await.unwrap();
    assert_eq!(a.id, b.id); h.wait_status(&a.id, "done").await; assert_eq!(h.provider.starts(), 1);
}
#[tokio::test] async fn a_refused_unconfirmed_press_then_the_confirmed_one_makes_one_run() {
    let req = || StartRequest { client_token: "press-2".into(), ..h.video_req() };
    assert_eq!(h.manager.start(req(), false).await.unwrap_err().code, "studio_needs_confirmation");
    let a = h.manager.start(req(), true).await.unwrap();
    let b = h.manager.start(req(), true).await.unwrap();
    assert_eq!(a.id, b.id); assert_eq!(h.store_runs().await.len(), 1);
}
#[tokio::test] async fn studio_picks_a_seed_when_the_model_takes_one() {
    // seedream-5-5 request without seed → stored params.params.seed in 0..=2_147_483_647; nano banana → no seed key
}
#[tokio::test] async fn a_field_the_model_does_not_list_is_refused_before_anything_is_sent() {
    // {"seed": 1} for nano banana → bad_request naming the setting; starts() == 0
}
#[tokio::test] async fn two_pictures_run_at_once_and_the_third_waits() {
    // provider holds each start until released; 3 starts on one endpoint → [sending, sending, queued], queue_position Some(1)
}
#[tokio::test] async fn an_openai_picture_cancelled_while_sending_may_be_charged_and_counts() {
    let run = h.manager.start(h.openai_req(), true).await.unwrap();      // held in `sending`, no estimate
    let r = h.manager.cancel(&run.id).await.unwrap();
    assert_eq!(r.status, "cancelled"); assert_eq!(r.error.as_ref().unwrap().charged, None);
    assert_eq!(r.error.unwrap().message, "Stopped waiting. OpenAI may still charge for it.");
    assert_eq!(h.manager.usage().await.unwrap().spent_today_usd, UNKNOWN_PRICE_RESERVE_USD);
}
#[tokio::test] async fn an_openrouter_picture_cancelled_while_sending_is_not_charged() {
    // charged Some(false); message "Stopped. OpenRouter does not charge for a picture you did not receive."; spent_today 0
}
#[tokio::test] async fn a_run_that_was_not_sent_counts_nothing() {
    // fake → Err(NotSent) → status failed, code "studio_not_sent", charged Some(false), message "Not sent. Nothing was charged."
    assert_eq!(h.manager.usage().await.unwrap().spent_today_usd, 0.0);
}
#[tokio::test] async fn three_openai_runs_in_a_row_stay_within_the_limit() {
    // unknown estimate, confirmed; fake answers Done with Cost { usd: 0.07, reported: false } each time
    for t in ["o1", "o2", "o3"] { let r = h.manager.start(h.openai_req_token(t), true).await.unwrap(); h.wait_status(&r.id, "done").await; }
    assert!(h.manager.start(h.openai_req_token("o4"), true).await.is_ok());   // held in `sending`
    let u = h.manager.usage().await.unwrap();
    assert!((u.spent_today_usd - 0.21).abs() < 1e-9); assert_eq!(u.pending_usd, UNKNOWN_PRICE_RESERVE_USD);
}
#[tokio::test] async fn a_timeout_fails_the_run_and_it_is_never_sent_again() {
    // fake → Err(Unknown) for an OpenAI picture; status failed, code "studio_not_answered", charged None; after 2 s starts() == 1
}
#[tokio::test] async fn a_blocked_run_is_never_sent_again() {
    // fake → Answered(Blocked, charged Some(false)); status blocked; error.message "OpenAI's safety filter refused the prompt."; starts() == 1
}
#[tokio::test] async fn the_audit_row_has_the_prompt_length_not_the_prompt() {
    let row = h.audit_rows().await.pop().unwrap();
    assert_eq!((row.server.as_str(), row.tool.as_str()), ("studio", "picture"));
    assert_eq!(row.chat_id, None);
    assert_eq!(row.decision, AuditDecision::AllowedWithoutPrompt); assert_eq!(row.decided_by, "user");
    assert_eq!(row.arguments["prompt_chars"], 18);
    assert!(!row.arguments.to_string().contains("lighthouse"));
    assert_eq!(row.result_status.as_deref(), Some("done"));
    assert!(row.result_summary.unwrap().contains("$0.04"));
}
#[tokio::test] async fn extra_field_values_are_not_audited() {
    // flare with its saved answer, extra {"moderation":"low"} → arguments["extra_fields"] == ["moderation"]; no "low" value
}
#[tokio::test] async fn the_gallery_cap_is_said_plainly() { /* store at MAX_CREATION_RUNS → "studio_gallery_full" */ }
#[tokio::test] async fn a_run_on_this_computer_without_a_local_lane_is_refused() {
    let err = h.manager.start(StartRequest { target: RunTarget::ThisComputer, ..h.picture_req("x") }, false).await.unwrap_err();
    assert_eq!(err.code, "studio_no_local_engine"); assert!(h.store_runs().await.is_empty());
}
#[tokio::test] async fn cloud_runs_say_they_ran_in_the_cloud() {
    let run = h.manager.start(h.picture_req("x"), false).await.unwrap();
    assert_eq!(run.runs_on, "cloud"); assert!(run.eta.is_none() && run.local.is_none());
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::manager`
Expected: FAIL to compile, `cannot find type 'StudioManager'`.

- [ ] **Step 3: Implement.** `start`, `estimate` and `preview` first match `req.target`: `RunTarget::ThisComputer` goes to `start_local` / `estimate_local` / `preview_local`, which in this task return `errors::no_local_engine()` (Task L15 gives them the local lane); `RunTarget::Endpoint(id)` continues as below. `start`, under the `starting` lock: a known `client_token` (last 64 kept in memory, recorded only after the run row is inserted, so a refused press leaves no trace) returns its run; load the model from `catalog::model` (else `studio_no_endpoint`); the kind is the model's, and `req.kind != model.kind` is `bad_request`; add a random seed when the model has a Seed field and none is set; `spec::validate`; `openrouter::place_extra`; `price::estimate(choice_for(..))` on the model's price, re-read for an OpenRouter picture as `parse_image_endpoints(saved answer, req.routing)` so a pinned company's price is used; `billing = spend::billing_of(&model.price, estimate)`; `spend::check_limit` with `spent_since(creation_runs_since(day_start).map(spend_row))` and `billing` (refuse `studio_daily_limit`); `spend::confirm_reasons(kind, billing, &prefs.confirm_rules())` (refuse `studio_needs_confirmation` when not empty and `!confirmed`); insert the run (`queued`, `runs_on: RunsOn::Cloud`, `params = {params, extra, routing}`, `refs = [{sha256, role, mime, sizeBytes}]`, `upstream` when the routing pins one company or the saved answer lists only one); append the audit row (arguments: `endpoint`, `host`, `model`, `changed` = typed settings with their values, `extra_fields` = names only, `pictures_sent`, `estimate_usd`, `confirmed`, `prompt_chars`; preview "Made a picture with <model> through <endpoint>" / "Started a video …"; risk `normal`) and store its id in `audit_id`; enqueue. A run task: status `sending` and `started_at` are written **before** `provider.start`; `Started::Done` → each output through `gallery.finalize` (cap 50 MiB) and `insert_creation_output`, a thumbnail when a worker is set, sidecar, `done` with cost; errors into `error` JSON that always carries `charged`, plus `request_id`, `company`, `stage`:
  - `NotSent` → `not_sent`, `Some(false)`; `Answered(f)` → `from_failure`, `f.charged`; `Local(e)` → `from_local`, `None`;
  - `Unknown` (timeout, cut connection) and Cancel while `sending`: a picture on OpenRouter → `Some(false)` (OpenRouter bills only a picture it delivered: https://openrouter.ai/docs/guides/overview/multimodal/image-generation, "Billing and Cancellation"), anything else `None`;
  - Cancel while `queued` → `Some(false)`.
  A slot frees on every exit through a `TaskEnd` drop guard as in `DownloadManager`. Cancel cancels the token; the run's future is dropped (the connection closes). Every exit completes the audit row (once) with the status and `cost $X.XX` or `cost not reported`.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio::manager`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio src-tauri/src/dto.rs
git commit -m "Studio: start runs under the spending rules, queue them and make pictures"
```

### Task A14: StudioManager: video jobs and restarts

**Files:**
- Modify: `src-tauri/src/studio/manager.rs` (`StudioDeps` gains `pub poll: PollPlan`), `src-tauri/src/studio/manager_tests.rs`

**Interfaces:**
- Produces:
```rust
pub struct PollPlan { pub first: Duration, pub second: Duration, pub third: Duration, pub second_from: Duration, pub third_from: Duration, pub watch_limit: Duration }
impl Default for PollPlan   // 5 s; 10 s from 60 s; 20 s from 300 s; watch_limit 30 min
pub fn poll_delay(plan: &PollPlan, watched_for: Duration) -> Duration;
pub const PROGRESS_EVERY: Duration = Duration::from_millis(250);
impl StudioManager {
    pub async fn resume_interrupted(&self);                // also purges yesterday's spending stubs
    /// "Get it anyway" on a stopped video, and "Check again" on one still not ready: checks and downloads; never creates.
    pub async fn fetch_anyway(&self, run_id: &str) -> AppResult<RunDto>;
    /// Stops every run task and waits for them, writing no status: what was `waiting` stays `waiting` for the
    /// next start. Called when the app exits, and by the restart tests in place of `drop`.
    pub async fn shutdown(&self);
}
```

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn checks_come_every_5_then_10_then_20_seconds() {
    let p = PollPlan::default();
    assert_eq!(poll_delay(&p, secs(0)), secs(5));   assert_eq!(poll_delay(&p, secs(59)), secs(5));
    assert_eq!(poll_delay(&p, secs(60)), secs(10)); assert_eq!(poll_delay(&p, secs(299)), secs(10));
    assert_eq!(poll_delay(&p, secs(300)), secs(20)); assert_eq!(p.watch_limit, secs(30 * 60));
}
#[tokio::test] async fn the_remote_id_is_saved_before_the_first_check() { /* fake.check reads the row: remote_id == Some("vid_1") */ }
#[tokio::test] async fn a_network_error_while_checking_keeps_waiting() { /* check Err(Unknown) twice then Done → done */ }
#[tokio::test] async fn resume_after_restart_finishes_the_video_without_a_second_create() {
    let first = h.manager(); let run = first.start(h.video_req(), true).await.unwrap(); h.wait_status(&run.id, "waiting").await;
    first.shutdown().await;                                    // its tasks are gone, not merely dropped
    assert_eq!(h.store_run(&run.id).await.status, CreationStatus::Waiting);
    let second = h.manager_with(fake_done_on_first_check()); second.resume_interrupted().await;
    let done = h.wait_status(&run.id, "done").await;
    assert_eq!(h.provider.starts() + h.provider2.starts(), 1);
    assert_eq!(done.outputs.len(), 1);
    assert_eq!(h.provider.downloads() + h.provider2.downloads(), 1);
}
#[tokio::test] async fn a_run_left_sending_becomes_interrupted_and_is_never_sent_again() {
    // seeded row: status sending, no remote id → resume → interrupted, code "studio_interrupted",
    // message contains "may or may not have started"; starts() == 0
}
#[tokio::test] async fn a_queued_run_is_not_sent_after_a_restart() {
    // seeded row: queued → resume → cancelled, message "Not sent: the app closed before its turn. Nothing was charged."; starts() == 0
}
#[tokio::test] async fn a_download_left_half_done_starts_over_from_the_result() { /* downloading row → check → download; starts() == 0 */ }
#[tokio::test] async fn stop_watching_keeps_the_remote_id_and_get_it_anyway_downloads() {
    // cancel while waiting → cancelled, remote_id kept, error.charged None,
    // error.message "OpenRouter may still finish and charge for this."; fetch_anyway → done; starts() == 1
}
#[tokio::test] async fn stop_watching_then_get_it_anyway_records_done_with_the_cost() {
    // cancel while waiting, then fetch_anyway → done (job cost 0.36): no store error
    let rows = h.audit_rows_for(&run.id).await;                // oldest first
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].result_status.as_deref(), Some("cancelled"));
    assert_eq!(rows[1].parent_audit_id, Some(rows[0].id));
    assert_eq!(rows[1].result_status.as_deref(), Some("done"));
    assert!(rows[1].result_summary.as_deref().unwrap().contains("$0.36"));
}
#[tokio::test] async fn after_30_minutes_it_says_still_not_ready() {
    // watch_limit 50 ms → status waiting, watching false, stale true; fetch_anyway resumes checking
}
#[tokio::test] async fn a_job_the_company_no_longer_has_says_so() { /* Gone → failed "studio_job_lost", remote_id kept */ }
#[tokio::test] async fn an_expired_result_says_so_and_keeps_the_remote_id() {
    // download → Answered(Expired) → failed "studio_result_expired"; remote_id kept; counted at the job's cost (0.36); starts() == 1
}
#[tokio::test] async fn a_full_disk_keeps_the_remote_id_for_get_it_anyway() {
    // provider.download → Err(Local(DiskFull)) once → failed "studio_disk_full", remote_id kept, cost_usd Some(0.36), charged Some(true);
    // .partial empty; fetch_anyway → done; starts() == 1; the audit has a follow-up row "done"
}
#[tokio::test] async fn a_video_over_its_cap_is_refused() { /* provider.download → Err(Local(TooLarge(1024))) → failed "studio_too_large", partial removed */ }
#[tokio::test] async fn a_download_that_stops_can_be_fetched_again() { /* download → Err(Unknown) → failed "studio_download_failed", charged Some(true); fetch_anyway → done */ }
#[tokio::test] async fn a_foreign_result_link_goes_through_the_guarded_fetcher() {
    // provider.download → Elsewhere(url) → FakeFetch saw exactly that url; output saved (ForeignFetch has no key parameter)
}
#[tokio::test] async fn progress_is_sent_at_most_four_times_a_second() {
    // 100 Running{progress} answers within 1 s → progress-only events <= 4; every status change still sent
}
#[tokio::test] async fn the_audit_row_is_completed_after_a_restart() { /* audit row of the resumed run gets "done" and the cost */ }
#[tokio::test] async fn the_third_video_waits_for_a_slot() { /* VIDEOS_AT_ONCE = 2 per endpoint; a stale job frees its slot */ }
#[tokio::test] async fn yesterdays_spending_stubs_go_at_start() { /* stub created yesterday, stub created today → resume → only today's stays */ }
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::manager`
Expected: FAIL (`resume_interrupted`, `fetch_anyway`, `poll_delay` missing).

- [ ] **Step 3: Implement.** Every run task selects on its own token and on a root token that only `shutdown` cancels (a root stop writes nothing); `shutdown` awaits the tasks (a `JoinSet` or the `ended` notifications, as in `DownloadManager`). Video task: `sending` → `provider.start` → write `remote_id` and `waiting` → loop `check` with `poll_delay(watched_for)` until `Done`/`Failed`/`Gone` or `watch_limit` (then `watching = false`, row stays `waiting`, its audit row stays open); a check error keeps waiting. `Done` → write the job's cost first, then `downloading` → `provider.download` into `.partial/<output id>`; `Elsewhere(url)` → `deps.fetch.fetch_to_file(url, video_cap, partial)`; `gallery.finalize`; output row; `done`. A failure after `Done` keeps the remote id and the cost and records `charged: Some(true)`: `Local(e)` → `from_local`, `Expired` → `result_expired`, anything else → `download_failed`. `resume_interrupted` at start: `purge_creation_stubs(today's start)`; `sending` → `interrupted` (`charged: None`); `queued` → `cancelled` (`charged: Some(false)`); `waiting` with a remote id → watch again (counting `watched_for` from now); `downloading` → check and download again. Stop watching (cancel while `waiting`) → `cancelled`, `charged: None`, remote id kept. `fetch_anyway` on a run whose audit row is already completed (cancelled or failed with a remote id) appends a follow-up audit row (`parent_audit_id` = the old `audit_id`, tool and arguments as the original's, preview "Got the stopped video …") and stores its id in `audit_id`; on a stale `waiting` run it only starts watching again, and the original row is completed when the run ends.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio::manager`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio
git commit -m "Studio: follow video jobs across restarts without paying twice"
```

### Task A15: Read commands and model DTOs

**Files:**
- Create: `src-tauri/src/studio/commands.rs`
- Modify: `src-tauri/src/dto.rs` (model DTOs)
- Test: inline in `commands.rs`

**Interfaces:**
- Produces the read commands (each `async`, first checks `only_main_window(window.label())`, then calls a plain function the tests call directly; none touches the network except `studio_refresh_models` and `studio_model_price`):
```text
studio_models() -> StudioModelsDto                          // saved lists only
studio_refresh_models(provider_id: i64) -> StudioModelsDto
studio_model_price(provider_id: i64, model: String) -> MediaModelDto
studio_estimate(req: StartRequest) -> EstimateDto            // no network
studio_preview(req: StartRequest) -> RequestPreviewDto
studio_runs(filter: RunsFilter, before: Option<i64>, limit: u32) -> RunsPage
studio_live() -> Vec<RunDto>
studio_output_bytes(output_id: String, thumb: bool) -> tauri::ipc::Response   // up to SHOW_CAP = 256 MiB
studio_ref_bytes(sha256: String) -> tauri::ipc::Response
studio_usage() -> UsageDto
studio_get_prefs() -> StudioPrefs
```
Event `studio-progress` → `RunDto`. DTOs (serde camelCase; the run DTOs are Task A13's):
```rust
pub const SHOW_CAP: u64 = 256 * 1024 * 1024;
pub struct PriceHintDto { pub unit: String /* image|second|token|unknown|free */, pub low: Option<f64>, pub high: Option<f64> }
pub struct MediaModelDto { pub id: String, pub display_name: String, pub family: String, pub kind: MediaKind, pub accepts_images: bool,
    pub params: Vec<ParamSpec>, pub price: PriceHintDto, pub price_known: bool, pub price_fetched_at: Option<i64>,
    pub companies: Vec<EndpointCompany>, pub notes: Vec<String>, pub usable: Option<String>, pub created: Option<i64> }
    // price_fetched_at None on an OpenRouter picture = "price shown when picked"; Some with price_known false = "price unknown"
pub struct StudioEndpointDto { pub provider_id: i64, pub name: String, pub company: Company, pub host: String, pub cloud: bool,
    pub has_key: bool, pub fetched_at: Option<i64>, pub models: Vec<MediaModelDto> }
pub struct StudioModelsDto { pub endpoints: Vec<StudioEndpointDto> }
pub struct RequestPreviewDto { pub method: String, pub url: String, pub host: String, pub body: Value }
pub struct RunsFilter { pub kind: Option<MediaKind>, pub starred: bool, pub text: String, pub model: Option<String>,
    pub since: Option<i64>, pub until: Option<i64> }
pub struct RunsPage { pub runs: Vec<RunDto>, pub next: Option<i64> }
```

- [ ] **Step 1: Write the failing tests**
```rust
#[tokio::test] async fn studio_models_reads_only_the_saved_lists() {
    // saved catalogue for one OpenRouter endpoint; a ProviderSource that counts media() calls
    assert_eq!(models(&h).await.unwrap().endpoints[0].models.len(), 101);
    assert_eq!(h.providers.calls(), 0);
}
#[tokio::test] async fn output_bytes_gives_the_thumbnail_when_asked_and_refuses_a_huge_video() {
    // thumb=true → the .thumbs/<id>.jpg bytes; a recorded size over SHOW_CAP → code "studio_too_big_to_show"
}
#[tokio::test] async fn ref_bytes_refuses_a_name_that_is_not_a_hash() { /* "../x" → bad_request; a stored ref → its bytes */ }
#[tokio::test] async fn runs_filter_by_date_text_and_kind() { /* since/until bound created_at; text and kind as in Task A8 */ }
#[test] fn studio_errors_have_four_parts() { /* every errors.rs constructor: non-empty code, message, next_step */ }
#[test] fn the_safety_message_names_the_company_and_the_charge() {
    let e = errors::blocked(Company::OpenAi, Some(Stage::Prompt), Some(false), Some("req_8f2"));
    assert_eq!(e.message, "OpenAI's safety filter refused the prompt.");
    assert!(e.next_step.contains("Not charged") && e.details.contains("req_8f2"));
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::commands`
Expected: FAIL to compile, `cannot find function 'models'`.

- [ ] **Step 3: Implement.** Wire each command to `StudioManager`, `catalog`, `prefs` and `Gallery`. `studio_output_bytes` reads the recorded file (or its thumbnail) into `tauri::ipc::Response::new(bytes)`, refusing a file over `SHOW_CAP` with `studio_too_big_to_show` ("This video is too big to play here." / "Use Show in folder to open it with your video player."), since a capped video would otherwise sit whole in Rust, the IPC copy, the ArrayBuffer and the Blob. `studio_model_price` calls `catalog::refresh_price` and returns the model with its price, companies and extra-setting names.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio src-tauri/src/dto.rs
git commit -m "Studio: read commands and the model DTOs"
```

### Task A16: Changing commands, registration and start-up

**Files:**
- Modify: `src-tauri/src/studio/commands.rs`, `src-tauri/src/studio/manager.rs` (`delete_outputs`, `delete_unfinished`, `set_prefs`), `src-tauri/src/studio/manager_tests.rs`, `src-tauri/src/lib.rs` (remove the `dead_code` allow; register handlers; `manage(Arc<StudioManager>)`; spawn `resume_interrupted` next to the downloads resume; `shutdown()` on exit), `src-tauri/commands.list`, `src-tauri/capabilities/main.json`, `src-tauri/src/artifacts.rs` (`export_target(path, ext, forbidden)`, `pub(crate)`), `src-tauri/src/attachments.rs` (`ImageInput` also derives `Serialize`; it already renames to camelCase)
- Test: inline in `commands.rs`, and `manager_tests.rs`

**Interfaces:**
- Produces the changing commands (same pattern as Task A15):
```text
studio_start(req: StartRequest, confirmed: bool) -> RunDto
studio_cancel(run_id: String) -> RunDto
studio_fetch_anyway(run_id: String) -> RunDto
studio_save_thumb(output_id: String, jpeg_base64: String) -> OutputDto     // JPEG only, at most 1 MiB
studio_star(output_id: String, starred: bool) -> OutputDto
studio_delete(output_ids: Vec<String>) -> ()
studio_delete_unfinished() -> u32                            // "Delete stopped and failed runs"
studio_export(output_id: String, path: String, with_settings: bool) -> String
studio_show_in_folder(output_id: String) -> ()
studio_open_folder() -> ()
studio_add_ref(image: ImageInput) -> RefDto
studio_ref_from_output(output_id: String) -> RefDto
studio_output_for_chat(output_id: String) -> ImageInput      // pictures under 20 MB in PNG, JPEG, GIF or WebP
studio_set_prefs(patch: StudioPrefsPatch) -> StudioPrefs
```
```rust
impl StudioManager {
    pub async fn delete_outputs(&self, ids: Vec<String>) -> AppResult<()>;   // stubs for today's runs; files, sidecars, unused refs removed
    pub async fn delete_unfinished(&self) -> AppResult<u32>;
    pub async fn set_prefs(&self, patch: StudioPrefsPatch) -> AppResult<StudioPrefs>;   // apply, save, then hold the new prefs
}
pub(crate) fn for_chat(output: &store::CreationOutput) -> AppResult<()>;
```

- [ ] **Step 1: Write the failing tests**
```rust
const STUDIO_COMMANDS: &[&str] = &["studio_models", /* … the 11 of Task A15 and the 14 above … */ "studio_set_prefs"];
// Tasks L10 (studio::engine_cmd), L15 (studio::commands) and L16 (studio::local_cmd) append their commands to this list.
const STUDIO_COMMAND_MODULES: &[&str] = &["commands", "engine_cmd", "local_cmd"];
#[test] fn every_studio_command_is_listed_granted_and_registered() {
    let list = include_str!("../../commands.list"); let main = include_str!("../../capabilities/main.json");
    let lib = include_str!("../lib.rs");
    for c in STUDIO_COMMANDS {
        assert!(list.lines().any(|l| l.trim() == *c), "{c}");
        assert!(main.contains(&format!("\"allow-{}\"", c.replace('_', "-"))), "{c}");
        // `#[tauri::command]` functions are registered by their own module path (a re-export does not work in generate_handler!)
        assert!(STUDIO_COMMAND_MODULES.iter().any(|m| lib.contains(&format!("studio::{m}::{c},"))), "{c}");
    }
}
// manager_tests.rs
#[tokio::test] async fn deleting_todays_results_does_not_lower_spent_today() {
    let run = h.seed_done_run_costing(9.50).await;
    h.manager.delete_outputs(run.output_ids()).await.unwrap();
    assert!(!Path::new(&run.outputs[0].path).exists());
    let err = h.manager.start(h.req_estimated(0.96, "after-delete"), true).await.unwrap_err();
    assert_eq!(err.message, "Daily limit reached: $9.50 of $10 spent today");
}
#[tokio::test] async fn delete_unfinished_removes_stopped_and_failed_runs_only() {
    // one each of done, waiting, failed, blocked, cancelled, interrupted → returns 4; done and waiting still listed
}
#[tokio::test] async fn a_reference_no_run_uses_any_more_is_removed_and_usage_counts_refs() {
    // two runs share ref a, one uses ref b: delete the b run → refs/b gone, refs/a kept; usage().bytes includes refs/a
}
#[tokio::test] async fn a_new_gallery_folder_applies_to_the_next_run() {
    h.manager.set_prefs(patch(json!({"galleryDir": other.path()}))).await.unwrap();
    let r = h.manager.start(h.picture_req("x"), false).await.unwrap();
    assert!(Path::new(&h.wait_status(&r.id, "done").await.outputs[0].path).starts_with(other.path()));
}
#[tokio::test] async fn set_prefs_changes_only_what_it_is_sent_and_checks_it() {
    // {"confirmEveryVideo": false} keeps dailyLimitUsd; {"dailyLimitUsd": 0.5} → bad_request "Choose an amount from $1 to $1,000."
}
// commands.rs
#[test] fn a_saved_poster_must_be_a_small_jpeg() { /* PNG bytes → bad_request; 1 MiB + 1 → bad_request; a JPEG → thumb written, hasThumb */ }
#[test] fn a_video_or_a_big_picture_cannot_go_to_a_chat() {
    // for_chat: "video/mp4", "image/svg+xml" and a 21 MiB PNG are refused ("This picture is over 20 MB, so it cannot go into a chat."); a 2 MiB PNG passes
}
#[test] fn export_keeps_the_original_extension_and_refuses_app_folders() { /* export_target(path, "mp4", forbidden) */ }
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio && cargo test -p app --lib commands::tests::commands_list_matches_capabilities`
Expected: FAIL (commands missing from the list and lib.rs; `delete_outputs` missing).

- [ ] **Step 3: Implement.** `studio_export` copies the original, and with `with_settings` writes `<name>.json` (the run record: prompt, model, settings, cost; never a key). `studio_show_in_folder` uses `tauri_plugin_opener::OpenerExt::opener().reveal_item_in_dir`. `delete_outputs` / `delete_unfinished` call the store with today's start as `stub_since_ms`, then `gallery.remove_files(&deleted)` and `remove_unused_refs(refs of the deleted runs, store.creation_ref_hashes())`; the audit rows stay. `usage` adds `gallery.refs_bytes()`. In `lib.rs`: `StudioManager::new(StudioDeps{ StoreProviders { store, secrets }, AppSink, GuardedFetch(search::Fetcher::new(user_agent).with_timeout(Duration::from_secs(600))), SystemClock, default_gallery: Gallery::default_root(home, package name), thumbs: Some(PictureWorker::thumbs()), PollPlan::default(), MediaLimits::default() }, prefs::load(&store).await)`, `app.manage(studio.clone())`, then spawn `studio.resume_interrupted()` beside `downloads.resume_interrupted`, and call `studio.shutdown()` from the exit handler.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 5: Commit**
```bash
git add src-tauri
git commit -m "Studio: changing commands, registration, start-up resume and shutdown"
```

---

## Phase L: Local models (the Studio engine, catalog, downloads, runs)

Everything local the webview needs, in Rust. The screens are in Phase B (B2, B3, B5–B10 revised; B13 and B14 new) because they build on Task B1's store and types. No test in this phase downloads a model or runs a real engine; the one exception is `crates/runtime/tests/sd_contract.rs`, which runs an `sd-cli` built in CI from the pinned commit for its help text, its device list and the exact arguments Studio builds (parsed in metadata mode, with no model), and is skipped when `SD_CLI_BIN` is not set. Engine output fixtures are written now in the exact format the pinned build's source prints and replaced by recordings from the benchmark (Task L17), as Task D2 does for the cloud answers.

### Task L1: Pin the Studio engine, bundle it on macOS, and the one-day benchmark

**Files:**
- Create: `sd.lock.json`, `crates/runtime/src/sd/mod.rs`, `crates/runtime/src/sd/lock.rs`, `scripts/fetch_sd.py`, `scripts/fetch_sd_test.py`, `docs/dev/0.1.0/owner-checklist.md` (its "Local benchmark" section; Task D2 adds the rest)
- Modify: `crates/runtime/src/lib.rs` (`pub mod sd;`), `scripts/fetch_llama.py` (`macos_plan(lock, pin_key="llama_cpp")` checks `lock[pin_key]["commit"]`; `check_macos_app` requires the build targets of both `llama.lock.json` and `sd.lock.json`; `verify_packs` HEAD-checks both locks' packs), `llama.lock.json` (`vc_runtime_files` gains `msvcp140_codecvt_ids.dll`, so `fetch_llama.py windows` bundles it into `resources/llama/main` from the same Visual C++ redistributable folder), `scripts/fetch_llama_test.py`, `scripts/intel-mac-smoke.sh` (also runs the disk image's `Contents/MacOS/sd-cli --version` and `--list-devices` on the Intel runner), `src-tauri/tauri.macos.conf.json` (`"externalBin": ["binaries/llama-server", "binaries/llama-fit-params", "binaries/sd-cli"]`), `.github/workflows/ci.yml` (the `checks` job: `python3 scripts/fetch_sd_test.py` beside `fetch_llama_test.py`, and in its contract step `python3 scripts/fetch_sd.py linux` (builds the dev `sd-cli`, cached) with `echo "SD_CLI_BIN=$PWD/.cache/sd-linux/sd-cli" >> "$GITHUB_ENV"`; the `build` job's Windows steps: `python scripts/fetch_sd.py windows-check` right after `fetch_llama.py windows`; its macOS steps: `python3 scripts/fetch_sd.py macos` right after `fetch_llama.py macos`; every cache key that hashes `llama.lock.json` also hashes `sd.lock.json` and `scripts/fetch_sd.py`, and the contract cache also keeps `.cache/sd` and `.cache/sd-linux`), `.github/workflows/release.yml` (the same macOS step and keys)
- Create: `crates/runtime/tests/fixtures/sd_win_pack_listing.txt` (the 23 file names of the pinned Windows zip, one a line, written at pinning)
- Test: inline in `lock.rs`; `scripts/fetch_sd_test.py`, `scripts/fetch_llama_test.py`

**Interfaces:**
- Consumes: `runtime::llama::lock::{PackSpec, PackFile, InstallerEntry, SourceBuild, current_platform}` unchanged.
- Produces:
```rust
// crates/runtime/src/sd/lock.rs (serde; unknown fields ignored, as LlamaLock)
pub struct SdLock { pub schema: u32, pub sd_cpp: SdPin, pub submodules: BTreeMap<String, String>, pub keep_files: Vec<String>,
    pub vc_runtime_files: Vec<String>, pub windows_imports: BTreeMap<String, Vec<String>>, pub packs: Vec<PackSpec>,
    pub installer: HashMap<String, InstallerEntry>, pub dev: HashMap<String, SourceBuild> }
pub struct SdPin { pub tag: String, pub build: u32, pub commit: String, pub repository: String }
impl SdLock { pub fn parse(json: &str) -> Result<Self, serde_json::Error>; pub fn pack_for_current_platform(&self) -> Option<&PackSpec>;
    pub fn keeps(&self, file_name: &str) -> bool; }      // keep_files patterns (`*` only), case-insensitive, as packs::keep does
/// Windows DLLs every PC has (KERNEL32, USER32, ADVAPI32, SHELL32, OLE32, WS2_32, VERSION, api-ms-win-*, …): never shipped, never checked.
pub fn is_windows_system_dll(name: &str) -> bool;
pub fn embedded() -> &'static SdLock;          // include_str!("../../../../sd.lock.json"), parsed once (OnceLock)
```
```python
# scripts/fetch_sd.py (standard library only): commands macos | linux | windows-check | verify-packs; imports fetch_llama's download,
# run, macos_plan, check_macos_binary, lipo_create_command, sidecar_name, keep, find_vc_runtime, MACOS_UNIVERSAL_TRIPLE, MACOS_ARCHS
def macos() -> None    # checkout + the three submodules (SHAs checked against the lock) → sd-cli for both slices → src-tauri/binaries/sd-cli-<triple>,
                       # then the universal file, each checked
def linux() -> None    # the same checkout, built CPU-only with the lock's dev flags → .cache/sd-linux/sd-cli (chmod 755), prints the folder
def windows_check() -> None   # the pinned zip (verified) → keep_files into a temp folder + vc_runtime_files from resources/llama/main →
                              # `sd-cli.exe --version` and `--list-devices` with PATH = that folder and %SystemRoot%\System32 only
```
`sd.lock.json` (from the upstream release page on the day of pinning; each size and SHA-256 from downloading the asset once; the submodule SHAs from `git ls-tree <commit> ggml thirdparty/libwebp thirdparty/libwebm`; the import lists from `objdump -p` on the zip's `sd-cli.exe` and `stable-diffusion.dll`):
```jsonc
{ "schema": 1,
  "comment": "Pinned stable-diffusion.cpp, the Studio engine. Compiled into the app (runtime::sd::lock) and read by scripts/fetch_sd.py. Change only together with a passing contract-test run and re-recorded engine fixtures (ARCHITECTURE.md 7.1, 7.9).",
  "sd_cpp": { "tag": "master-1106-532d2c1", "build": 1106, "repository": "https://github.com/leejet/stable-diffusion.cpp",
              "commit": "<the 40-hex hash `git ls-remote https://github.com/leejet/stable-diffusion.cpp refs/tags/master-1106-532d2c1` prints>" },
  "submodules": { "ggml": "<40-hex>", "thirdparty/libwebp": "<40-hex>", "thirdparty/libwebm": "<40-hex>" },   // never cutlass or the server frontend
  "keep_files": ["sd-cli.exe", "stable-diffusion.dll", "ggml*.dll", "libwebp*.dll", "libsharpyuv.dll", "webm.dll",
                 "ggml.txt", "stable-diffusion.cpp.txt"],                                  // the zip has no LICENSE*; these two are its licence texts
  "vc_runtime_files": ["msvcp140.dll", "msvcp140_codecvt_ids.dll", "vcruntime140.dll", "vcruntime140_1.dll"],
  "windows_imports": { "sd-cli.exe": ["stable-diffusion.dll", "libwebp.dll", "libwebpmux.dll", "webm.dll", "MSVCP140_CODECVT_IDS.dll", "…as recorded"],
                       "stable-diffusion.dll": ["ggml.dll", "ggml-base.dll", "MSVCP140_CODECVT_IDS.dll", "…as recorded"] },
  "packs": [{ "id": "studio-vulkan", "backend": "vulkan", "platform": "windows-x86_64", "build": "master-1106-532d2c1", "dir": "sd-1106",
    "title": "Studio engine (graphics card and processor)", "requirements": "Any graphics card with a Vulkan driver; otherwise the processor",
    "license_name": "stable-diffusion.cpp MIT; ggml MIT; libwebp and libwebm BSD-3-Clause",
    "license_url": "https://github.com/leejet/stable-diffusion.cpp/blob/<commit>/LICENSE",
    "files": [{ "name": "sd-master-532d2c1-bin-win-vulkan-x64.zip",
      "url": "https://github.com/leejet/stable-diffusion.cpp/releases/download/master-1106-532d2c1/sd-master-532d2c1-bin-win-vulkan-x64.zip",
      "size": 0, "sha256": "" }] }],                                   // size and sha256 filled at pinning; the test refuses 0 and ""
  "installer": {
    "macos-aarch64": { "dir": "main", "backends": ["metal", "cpu"], "build_from_source": { "git": "https://github.com/leejet/stable-diffusion.cpp",
      "commit": "<same>", "targets": ["sd-cli"],
      "cmake_flags": ["-DBUILD_SHARED_LIBS=OFF", "-DSD_BUILD_SHARED_LIBS=OFF", "-DGGML_NATIVE=OFF", "-DSD_METAL=ON", "-DGGML_METAL=ON",
                      "-DGGML_METAL_EMBED_LIBRARY=ON", "-DSD_WEBP=ON", "-DSD_WEBM=ON", "-DSD_SERVER_BUILD_FRONTEND=OFF",
                      "-DCMAKE_OSX_DEPLOYMENT_TARGET=13.3", "-DCMAKE_OSX_ARCHITECTURES=arm64", "-DCMAKE_BUILD_TYPE=Release"] } },
    "macos-x86_64":  { "dir": "main", "backends": ["cpu", "metal"], "build_from_source": { /* the same, with -DCMAKE_OSX_ARCHITECTURES=x86_64
      and llama's x86 flags: -DGGML_SSE42=ON -DGGML_AVX=ON -DGGML_AVX2=ON -DGGML_FMA=ON -DGGML_F16C=ON -DGGML_BMI2=ON -DGGML_AVX512=OFF */ } } },
  "dev": { "linux-x86_64": { "git": "https://github.com/leejet/stable-diffusion.cpp", "commit": "<same>", "targets": ["sd-cli"],
    "cmake_flags": ["-DBUILD_SHARED_LIBS=OFF", "-DSD_BUILD_SHARED_LIBS=OFF", "-DGGML_NATIVE=OFF", "-DSD_WEBP=ON", "-DSD_WEBM=ON",
                    "-DSD_SERVER_BUILD_FRONTEND=OFF", "-DCMAKE_BUILD_TYPE=Release"] } } }    // CPU only; the upstream Linux zip needs glibc 2.43
```
The switch names are the pinned `CMakeLists.txt`'s (`SD_METAL`, `SD_WEBP`, `SD_WEBM` (needs `SD_WEBP`), `SD_BUILD_SHARED_LIBS`, all off by default; `SD_SERVER_BUILD_FRONTEND` in `examples/server`); sd.cpp's own Metal kernels are embedded as source, so no `.metallib` ships; `sd-cli --version` exists, so `check_macos_binary` works as for llama. The build fetches only the `ggml`, `thirdparty/libwebp` and `thirdparty/libwebm` submodules (`git submodule update --init --depth 1 <those>`, then `git submodule status` must show the lock's SHAs); without them CMake has no ggml and `SD_WEBP=ON` stops with "WebP support enabled but no source found". `check_macos_binary` fails the build unless both processor parts exist, only system libraries are linked (the vendored libwebp and libwebm built in) and the minimum is 13.3.

- [ ] **Step 1: Write the failing tests**
```rust
// crates/runtime/src/sd/lock.rs
#[test] fn the_embedded_lock_pins_one_build() {
    let l = embedded();
    assert_eq!((l.sd_cpp.tag.as_str(), l.sd_cpp.build), ("master-1106-532d2c1", 1106));
    assert!(l.sd_cpp.commit.len() == 40 && l.sd_cpp.commit.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
}
#[test] fn the_windows_pack_is_the_vulkan_zip_from_github() {
    let p = embedded().packs.iter().find(|p| p.id == "studio-vulkan").unwrap();
    assert_eq!((p.platform.as_str(), p.dir.as_str(), p.backend), ("windows-x86_64", "sd-1106", BackendKind::Vulkan));
    for f in &p.files {
        assert!(f.url.starts_with("https://github.com/leejet/stable-diffusion.cpp/releases/download/master-1106-532d2c1/"));
        assert!(f.size > 0 && f.sha256.len() == 64, "{}: pin its size and SHA-256", f.name);
    }
}
#[test] fn the_engine_keeps_its_own_files_and_none_of_llamas() {
    let l = embedded();
    assert!(l.keeps("sd-cli.exe") && l.keeps("stable-diffusion.cpp.txt") && l.keeps("ggml.txt"));
    assert!(!l.keeps("llama-server.exe") && !l.keeps("sd-server.exe") && !l.keeps("start-server.bat"));
}
#[test] fn the_engine_keeps_every_dll_it_imports() {
    // every import of sd-cli.exe and stable-diffusion.dll is a system DLL, a Visual C++ runtime file, or a file of the pinned zip that is kept
    let l = embedded(); let listing = include_str!("../../tests/fixtures/sd_win_pack_listing.txt");
    assert!(!l.windows_imports.is_empty());
    for (exe, dlls) in &l.windows_imports {
        for dll in dlls {
            let vc = l.vc_runtime_files.iter().any(|f| f.eq_ignore_ascii_case(dll));
            let shipped = listing.lines().any(|f| f.eq_ignore_ascii_case(dll)) && l.keeps(dll);
            assert!(is_windows_system_dll(dll) || vc || shipped, "{exe} imports {dll}, which the pack would not have");
        }
    }
}
#[test] fn the_vc_runtime_covers_every_msvc_import() {
    let l = embedded();
    for dll in l.windows_imports.values().flatten().filter(|d| d.to_ascii_lowercase().starts_with("msvcp") || d.to_ascii_lowercase().starts_with("vcruntime")) {
        assert!(l.vc_runtime_files.iter().any(|f| f.eq_ignore_ascii_case(dll)), "{dll}");
    }
    let llama = crate::llama::lock::embedded();           // packs copy the runtime from llama's installed folder, so it must hold them too
    for f in &l.vc_runtime_files { assert!(llama.vc_runtime_files.contains(f), "{f} is not in llama.lock.json"); }
}
#[test] fn both_macos_slices_build_sd_cli_from_the_pin() {
    for key in ["macos-aarch64", "macos-x86_64"] {
        let b = embedded().installer[key].build_from_source.as_ref().unwrap();
        assert_eq!((b.targets.as_slice(), b.commit.as_str()), (["sd-cli".to_string()].as_slice(), embedded().sd_cpp.commit.as_str()));
    }
}
#[test] fn the_llama_lock_offers_no_studio_pack() {
    assert!(crate::llama::lock::embedded().packs.iter().all(|p| !p.id.starts_with("studio")));   // list_packs never shows it
}
```
```python
# scripts/fetch_sd_test.py
class Plan(unittest.TestCase):
    def test_the_plan_builds_sd_cli_for_both_processors(self):
        plan = fetch_llama.macos_plan(fetch_sd.LOCK, pin_key="sd_cpp")
        self.assertEqual(plan["targets"], ["sd-cli"]); self.assertEqual([s["arch"] for s in plan["slices"]], ["arm64", "x86_64"])
    def test_a_slice_on_another_commit_stops_the_build(self): ...   # SystemExit
    def test_the_flags_keep_the_app_rules(self):                  # every slice: BUILD_SHARED_LIBS=OFF, GGML_NATIVE=OFF,
        ...                                                       # GGML_METAL_EMBED_LIBRARY=ON, CMAKE_OSX_DEPLOYMENT_TARGET=13.3;
                                                                  # the x86_64 slice has llama's GGML_AVX2=ON … GGML_AVX512=OFF
class DryRun(unittest.TestCase):
    def test_macos_joins_both_parts_into_the_universal_sd_cli(self):
        ...  # run/check_macos_binary stubbed as fetch_llama_test.DryRun does → lipo gets sd-cli-aarch64-apple-darwin and
             # sd-cli-x86_64-apple-darwin and writes sd-cli-universal-apple-darwin
    def test_the_mac_build_fetches_its_three_submodules(self):
        ...  # recorded `run` calls: "git submodule update --init --depth 1 ggml thirdparty/libwebp thirdparty/libwebm" after the checkout,
             # never cutlass or examples/server/frontend; a stubbed `git submodule status` with another ggml SHA → SystemExit
    def test_linux_builds_the_dev_engine_from_the_pin(self):
        ...  # recorded cmake configure carries the lock's dev flags; the result is .cache/sd-linux/sd-cli with mode 0o755
    def test_windows_check_runs_the_kept_files_with_a_bare_path(self):
        ...  # stubbed zip with the listing's 23 names: only keep_files land in the folder; the recorded run has PATH = folder;System32
# scripts/fetch_llama_test.py, in DryRun
    def test_the_app_check_needs_sd_cli(self):
        ...  # a universal bundle holding the app, llama-server and llama-fit-params only → SystemExit naming sd-cli
    def test_verify_packs_checks_both_locks(self):
        ...  # urlopen stubbed to answer the pinned sizes → the studio-vulkan zip URL was asked
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p runtime --lib sd::lock; python3 -I scripts/fetch_sd_test.py; python3 -I scripts/fetch_llama_test.py`
Expected: FAIL (`could not find 'sd' in the crate root`; `ModuleNotFoundError: No module named 'fetch_sd'`; `test_the_app_check_needs_sd_cli` passes a bundle without sd-cli).

- [ ] **Step 3: Implement** the lock, its types and `embedded()`, `fetch_sd.py` on `fetch_llama.py`'s helpers (`macos_plan` gains `pin_key`; its existing callers pass nothing), the `externalBin` entry, the Intel smoke lines and the CI and release steps. `check_macos_app`'s "at least 3 programs" becomes "at least 4". Then write the owner checklist's **Local benchmark** section (addendum L7) with a **Results** table that Task L17 applies; it runs before any time is shown in the UI or written in a document:
  1. **Mac** (the owner's MacBook Air M5 and its memory size): Metal from the universal build; FLUX.2 klein 4B and Z-Image Turbo at 1024² (Z-Image with `GGML_METAL_TENSOR_DISABLE=1` and without), Wan 2.2 5B fast at 2 s and 4 s 480p and 5 s 720p, then a second 5 s 720p run straight after (throttling).
  2. **The fast Wan 2.2 version:** the provisional entry (Task L7) is FastVideo's FastWan2.2-TI2V-5B (a DMD distillation, Apache-2.0) as Green-Sky's GGUF; hum-ma's Turbo is not used unless its base (`quanhaol/Wan2.2-TI2V-5B-Turbo`) states a licence. Check that it loads in the pinned build, take its steps (3 or 4), cfg and flow shift from its model card, and compare Q5_K_M with Q6_K. **Decides (Task L17):** it loads → it stays, with those defaults; no fast version loads → the video entry is `wan21-t2v-1.3b` alone; it loads but a 2 s 480p clip on the Air takes more than 10 minutes, or it does not fit a 6 GB card at 480p → `wan21-t2v-1.3b` is **added** as a fourth entry, tiered for Apple silicon and small cards, and the Wan 2.2 entry gets `"runsOnApple": false` only when Wan 2.1 runs acceptably on the Air.
  3. **Windows:** Vulkan speed on the owner's card (and CUDA for comparison if it is NVIDIA), plus one 8 GB and one 12 GB card when available.
  4. **Memory and speed:** peak memory per preset and seconds per step, for each starter entry's `memory[]` and `speed[]` (each speed point with `measuredOn`: date and machine); out-of-memory output of this build, for Task L3's patterns.
  5. **Playback:** WebM (VP8) in WKWebView and WebView2. **Decides:** if WKWebView cannot play it, the video entries get the `video-webp-on-mac` quirk (Task L6).
  6. **Sharing:** `sd-cli` beside a loaded 4B and a loaded 9B chat model (checks Task L12's "fits alongside" margin).
  7. **One shared encoder:** whether one Qwen3-4B GGUF serves both FLUX.2 klein and Z-Image (Comfy-Org ships the identical `qwen_3_4b.safetensors` for both); if it does, Task L17 gives both entries the same `llm` part, so the second starter downloads 2.5 GB less.
  8. **Fixtures:** record over the hand-written ones (raw bytes, so `\r` and `ESC[K` stay; stdout and stderr interleaved as they arrive): `sd_progress_flux2_klein.txt`, `sd_progress_wan22_ti2v.txt`, `sd_stderr_out_of_memory_vulkan.txt`, `sd_list_devices_vulkan.txt`, `sd_list_devices_metal.txt`, `sd_help.txt` in `crates/runtime/tests/fixtures/`, and write the build's tag in `sd_fixtures_build.txt`.
  (Settled from the pinned source, so not measured: `--disable-image-metadata` also clears WebM metadata; `-h` exits 0; `--list-devices` prints `name<TAB>description` a line, the names `--backend` takes; the CLI can write live previews (`--preview`, `--preview-path`), left for 0.1.x.)

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p runtime --lib sd::lock && python3 -I scripts/fetch_sd_test.py && python3 -I scripts/fetch_llama_test.py`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add sd.lock.json llama.lock.json crates/runtime/src/sd crates/runtime/src/lib.rs crates/runtime/tests/fixtures/sd_win_pack_listing.txt scripts/fetch_sd.py scripts/fetch_sd_test.py scripts/fetch_llama.py scripts/fetch_llama_test.py scripts/intel-mac-smoke.sh src-tauri/tauri.macos.conf.json .github/workflows/ci.yml .github/workflows/release.yml docs/dev/0.1.0/owner-checklist.md
git commit -m "Studio engine: pin stable-diffusion.cpp, build sd-cli into the Mac app, and the benchmark checklist"
```

### Task L2: Installing the Windows engine pack, and finding the engine

**Files:**
- Modify: `crates/runtime/src/packs.rs` (`InstallSpec`, `install_files`; `install` builds an `InstallSpec` from `LlamaLock` and calls it; `part_name` becomes `pub(crate)`)
- Create: `crates/runtime/src/sd/engine.rs`
- Test: inline in both

**Interfaces:**
- Consumes: Task L1 `SdLock`; `packs::{PackError, PackState, PackProgress, download_verified, unpack_zip}`; `llama::backend::PACK_COMPLETE_MARKER`.
- Produces:
```rust
// packs.rs
pub struct InstallSpec<'a> { pub id: &'a str, pub build: &'a str, pub dir: &'a str, pub files: &'a [PackFile], pub keep: &'a [String],
    pub keep_all: bool, pub vc_runtime_files: &'a [String], pub vc_runtime_dir: Option<&'a Path> }
/// Downloads (resuming) into `<root>/.downloads`, verifies size and SHA-256, unpacks into `<root>/.tmp-<dir>`, copies the VC++
/// files, writes `.complete`, renames to `<root>/<dir>`. No platform check: the caller makes it.
pub async fn install_files(spec: &InstallSpec<'_>, root: &Path, cancel: &CancellationToken,
    progress: &(dyn Fn(PackProgress) + Send + Sync)) -> Result<PathBuf, PackError>;
// sd/engine.rs
pub const SD_DEV_OVERRIDE_ENV: &str = "APP_DEV_SD_DIR";
pub fn program_file_name() -> &'static str;                         // "sd-cli.exe" on Windows, else "sd-cli"
pub enum EngineSource { Bundled, Pack, Dev }
pub struct Engine { pub program: PathBuf, pub dir: PathBuf, pub build: u32, pub source: EngineSource,
    pub leading_args: Vec<OsString>, pub extra_env: Vec<(String, String)> }   // the last two are empty except for the fake engine in tests
pub struct EngineLayout { pub bundled_dir: Option<PathBuf>, pub engines_dir: PathBuf, pub llama_dir: Option<PathBuf>, pub dev_override: Option<PathBuf> }
impl EngineLayout { pub fn with_env_override(bundled_dir: Option<PathBuf>, engines_dir: PathBuf, llama_dir: Option<PathBuf>) -> Self; }
pub fn discover(layout: &EngineLayout, lock: &SdLock, platform: &str) -> Option<Engine>;   // dev, then bundled, then the installed pack
pub fn pack_state(lock: &SdLock, engines_dir: &Path) -> PackState;                       // Installed needs the marker *and* the program
pub async fn install(lock: &SdLock, layout: &EngineLayout, cancel: &CancellationToken,
    progress: &(dyn Fn(PackProgress) + Send + Sync)) -> Result<Engine, PackError>;     // WrongPlatform off windows-x86_64; VC++ from llama_dir
pub async fn remove(lock: &SdLock, engines_dir: &Path) -> Result<(), PackError>;
```

- [ ] **Step 1: Write the failing tests**
```rust
// packs.rs
#[tokio::test] async fn install_files_keeps_what_the_pinned_zip_needs() {
    // a zip holding one small file for each name in tests/fixtures/sd_win_pack_listing.txt (the pinned zip's 23 names); its PackFile
    // on github.com with the zip's true size and SHA-256; the zip already at <root>/.downloads/<part_name>, so fetch only verifies it;
    // vc_runtime_dir holds the four Visual C++ files
    let dir = install_files(&sd_spec(&f), root, &CancellationToken::new(), &|_| {}).await.unwrap();
    assert_eq!(dir, root.join("sd-1106"));
    for kept in ["sd-cli.exe", "stable-diffusion.dll", "ggml-vulkan.dll", "ggml-cpu-haswell.dll", "libwebp.dll", "libwebpmux.dll",
                 "libsharpyuv.dll", "webm.dll", "ggml.txt", "stable-diffusion.cpp.txt", "msvcp140_codecvt_ids.dll", PACK_COMPLETE_MARKER] {
        assert!(dir.join(kept).is_file(), "{kept}");
    }
    for left in ["sd-server.exe", "start-server.bat", "start-server.sh"] { assert!(!dir.join(left).exists(), "{left}"); }
}
#[tokio::test] async fn a_changed_archive_installs_nothing() {
    // same, the pinned sha256 differs → Err(ChecksumMismatch { .. }); root.join("sd-1106") does not exist
}
// sd/engine.rs
#[test] fn discover_prefers_the_dev_folder_then_the_bundled_program_then_the_pack() {
    // three folders each holding program_file_name(); assert source Dev, then (dev None) Bundled, then (bundled None) Pack on "windows-x86_64"
}
#[test] fn a_pack_whose_program_was_removed_is_not_found() {
    // sd-1106/.complete present, sd-cli.exe deleted (as antivirus quarantine does) → discover(..) None; pack_state == NotInstalled;
    // the same with libwebp.dll deleted instead
}
#[test] fn the_engine_folder_is_never_the_llama_folder() {
    // the installed pack's Engine.dir == engines_dir.join("sd-1106") and != layout.llama_dir
}
#[tokio::test] async fn installing_the_pack_is_for_windows_only() {
    if cfg!(windows) { return; }
    assert!(matches!(install(embedded(), &layout, &CancellationToken::new(), &|_| {}).await, Err(PackError::WrongPlatform)));
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p runtime --lib packs && cargo test -p runtime --lib sd::engine`
Expected: FAIL to compile, `cannot find function 'install_files'`.

- [ ] **Step 3: Implement.** `install_files` is today's `install` body from the download loop on, with the lock's fields replaced by the spec's; `install` keeps its signature and checks (platform, ROCm path length) and passes `InstallSpec { keep: &lock.keep_files, vc_runtime_files: &lock.vc_runtime_files, .. }`. `engine::install` passes the sd lock's `keep_files`, `keep_all: false` and `vc_runtime_dir = layout.llama_dir` (the VC++ files, `msvcp140_codecvt_ids.dll` included, are copied from the installed llama folder, as packs already do), with `root = layout.engines_dir`. `discover` sets `build = lock.sd_cpp.build` for every source and `dir` = the program's folder (the working directory at run time). A pack folder missing any kept DLL the lock's `windows_imports` name counts as `NotInstalled` too, so Studio offers the download again rather than failing every run with "missing library".

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p runtime --lib packs && cargo test -p runtime --lib sd::engine`
Expected: PASS (the existing pack tests too).

- [ ] **Step 5: Commit**
```bash
git add crates/runtime/src/packs.rs crates/runtime/src/sd
git commit -m "Studio engine: install the Windows pack beside, never into, llama's folder, and find the engine"
```

### Task L3: Reading the engine's output: `\r` lines, progress and failures

**Files:**
- Modify: `crates/runtime/src/llama/lines.rs` (`LineReader::splitting_cr`)
- Create: `crates/runtime/src/sd/progress.rs`, `crates/runtime/src/sd/failure.rs`; fixtures in `crates/runtime/tests/fixtures/`: `sd_progress_flux2_klein.txt`, `sd_progress_wan22_ti2v.txt`, `sd_progress_batch_of_two.txt`, `sd_progress_two_passes.txt`, `sd_progress_vae_tiling.txt`, `sd_stderr_out_of_memory_vulkan.txt`, `sd_stderr_out_of_memory_buffers.txt`, `sd_stderr_device_lost.txt`, `sd_stderr_missing_dll.txt`, `sd_stderr_bad_model.txt`, `sd_fixtures_build.txt` (`master-1106-532d2c1`: the build the fixtures' format follows)
- Test: inline in each

**Interfaces:**
- Produces:
```rust
impl<R: AsyncRead + Unpin> LineReader<R> { pub(crate) fn splitting_cr(inner: R) -> Self; }   // `\r` ends a line too; `\r\n` is one end
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)] #[serde(rename_all = "camelCase")]
pub enum LocalStage { Loading, ReadingPrompt, Drawing, Decoding, Saving }
#[derive(Debug, Clone, PartialEq, Serialize)] #[serde(rename_all = "camelCase")]
pub struct LocalProgress { pub stage: LocalStage, pub pass: u32, pub passes: u32, pub step: u32, pub steps: u32,
    pub picture: u32, pub pictures: u32, pub elapsed_ms: u64, pub sec_per_step: Option<f64> }
pub struct ProgressParser { /* pictures, passes, bars seen, last progress */ }
impl ProgressParser { pub fn new(pictures: u32, passes: u32) -> Self; pub fn feed(&mut self, line: &str, elapsed_ms: u64) -> Option<LocalProgress>; }
pub fn overall(p: &LocalProgress) -> f32;     // loading 0–0.10, reading the prompt 0.10–0.15, drawing 0.15–0.90, decoding 0.90–0.99, saving 0.99
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdFailure { OutOfMemory, DeviceLost, MissingLibrary(String), BadModel, EngineMissing, EngineBlocked(String), Crashed { code: Option<i32> }, Cancelled, Io(String) }
/// `tail`: the last 60 lines of stdout and stderr merged in arrival order (this build logs INFO and WARN to stdout, ERROR to stderr).
pub fn classify(tail: &[String], exit_code: Option<i32>) -> SdFailure;
```
The hand-written fixtures follow the pinned build's output exactly as its source prints it (`examples/common/log.cpp`: `[I] `, `[W] `, `[E] ` prefixes, INFO and WARN on stdout, ERROR on stderr; `src/core/util.cpp`: each bar redraw is `\r<bar> <step>/<steps> - <speed right-aligned to 10><unit>[ - ETA mm:ss]\033[K`, with `\n` only after the last step; tensor loading draws `#` bars in MB/s or GB/s; decoding with `--vae-tiling` draws `=` bars that count tiles):
```text
[I] loading diffusion model from '/m/leejet/FLUX.2-klein-4B-GGUF/flux-2-klein-4b-Q4_0.gguf'
⏎|######################                 | 1200/2460 -  410.2MB/s␛[K⏎|#######################################| 2460/2460 -  402.9MB/s␛[K
[I] generate_image 1024x1024
[I] get_learned_condition completed, taking 0.81s
[I] sampling using euler method
[I] generating image: 1/1 - seed 81723
⏎  |============>                                     | 1/4 -      2.31s/it - ETA 00:07␛[K  …  ⏎  |==================================================| 4/4 -      2.18s/it␛[K
[I] sampling completed, taking 8.81s
[I] generating 1 latent images completed, taking 8.90s
[I] decoding 1 latents
[I] decode_first_stage completed, taking 1.02s
[I] save result image 0 to '/g/.partial/r1/r1.png' (success)
```
(⏎ is a `\r`, ␛ an ESC.) The parser strips ESC sequences, the level prefix and an ETA suffix, and moves **forward only**: Loading (and `#` bars as loading progress) → ReadingPrompt on `generate_image` or `generate_video` → Drawing on `get_learned_condition completed` (or `sampling using`) → Decoding on `decoding ` (`decoding N latents`, `decoding N/M latents`), `latent images completed` or `latent video completed` → Saving on `save result`. A marker for an earlier stage is ignored (weights load lazily, so `loading tensors completed` can come after sampling began). Steps come only from `=` bars with an `s/it` or `it/s` unit while the stage is Drawing (tile bars while decoding never count as steps); the picture is read from `generating image: i/n`; a two-pass model's first pass is announced by `sampling(high noise)` and the second starts after `sampling(high noise) completed`.

- [ ] **Step 1: Write the failing tests**
```rust
#[tokio::test] async fn a_carriage_return_ends_a_line() {
    assert_eq!(lines(LineReader::splitting_cr(&b"a\rb\r\nc\n"[..])).await, ["a", "b", "c"]);
}
#[tokio::test] async fn llamas_reader_still_splits_on_newlines_only() {
    assert_eq!(lines(LineReader::new(&b"a\rb\n"[..])).await, ["a\rb"]);
}
#[test] fn steps_come_from_the_bar_in_either_unit() {
    let mut p = drawing_parser(1, 1);                                     // fed "[I] get_learned_condition completed, taking 0.81s"
    let x = p.feed("  |=====>        | 3/4 -      2.10s/it - ETA 00:02\x1b[K", 9_000).unwrap();
    assert_eq!((x.stage, x.step, x.steps, x.sec_per_step), (LocalStage::Drawing, 3, 4, Some(2.10)));
    assert!((p.feed("  |======>       | 4/4 -      1.60it/s\x1b[K", 9_600).unwrap().sec_per_step.unwrap() - 0.625).abs() < 1e-9);
}
#[test] fn the_flux2_fixture_walks_every_stage_in_order() {
    // feed every \r/\n line of sd_progress_flux2_klein.txt → distinct stages, in order: [Loading, ReadingPrompt, Drawing, Decoding, Saving]
}
#[test] fn loading_bars_and_tile_bars_are_not_steps() {
    // "#" loading bars before generate_image leave step 0; sd_progress_vae_tiling.txt: the "=" tile bars after "decoding 1 latents"
    // keep stage Decoding and step == steps of the sampling bar
}
#[test] fn a_late_loading_line_never_moves_the_stage_back() {
    // Drawing, then "[I] loading tensors completed, taking 3.10s (…)" → the next update is still Drawing
}
#[test] fn a_batch_of_two_counts_pictures() { /* sd_progress_batch_of_two.txt, ProgressParser::new(2, 1) → "generating image: 2/2" → picture 2 of 2 */ }
#[test] fn two_pass_models_report_pass_2_of_2() { /* sd_progress_two_passes.txt, ::new(1, 2) → after "sampling(high noise) completed", pass 2 of 2 */ }
#[test] fn the_overall_fraction_never_goes_back() { /* over sd_progress_wan22_ti2v.txt: overall() non-decreasing, ends at 0.99 */ }
#[test] fn failures_are_named() {
    assert_eq!(classify(&fixture("sd_stderr_out_of_memory_vulkan.txt"), Some(1)), SdFailure::OutOfMemory);
    assert_eq!(classify(&fixture("sd_stderr_device_lost.txt"), Some(1)), SdFailure::DeviceLost);
    assert_eq!(classify(&fixture("sd_stderr_missing_dll.txt"), Some(-1073741515)), SdFailure::MissingLibrary("ggml-vulkan.dll".into()));
    assert_eq!(classify(&fixture("sd_stderr_bad_model.txt"), Some(1)), SdFailure::BadModel);
    assert_eq!(classify(&[], Some(139)), SdFailure::Crashed { code: Some(139) });
}
#[test] fn memory_failures_in_this_builds_words_are_named() {
    for line in ["[E] model manager alloc params backend buffer failed", "[E] model manager alloc compute params backend buffer failed",
                 "[E] diffusion graph allocation failed", "[W] execution engine cannot make enough memory available on Vulkan0",
                 "[E] memory allocation of size 2147483648 failed"] {
        assert_eq!(classify(&[line.into()], Some(1)), SdFailure::OutOfMemory, "{line}");
    }
    assert_eq!(classify(&fixture("sd_stderr_out_of_memory_buffers.txt"), Some(1)), SdFailure::OutOfMemory);
}
#[test] fn the_engines_own_tiling_retry_is_not_a_failure() {
    // "[W] VAE decode ran out of memory; retrying with … tiling" followed by a crash exit → Crashed, not OutOfMemory
    assert_eq!(classify(&["[W] VAE decode ran out of memory; retrying with vae tiling".into()], Some(139)), SdFailure::Crashed { code: Some(139) });
}
#[test] fn the_fixtures_were_recorded_from_the_pinned_build() {
    assert_eq!(include_str!("../../tests/fixtures/sd_fixtures_build.txt").trim(), crate::sd::lock::embedded().sd_cpp.tag);
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p runtime --lib sd:: && cargo test -p runtime --lib llama::lines`
Expected: FAIL to compile, `no function or associated item named 'splitting_cr'`.

- [ ] **Step 3: Implement.** `splitting_cr` sets a flag that makes `next_line` end a line at `\r` or `\n` (a `\n` right after a `\r` is skipped); `MAX_LINE_BYTES` still applies. `classify` reads the merged tail, lower-cased, skipping lines about the engine's own `retrying with … tiling`: out of memory on `out of memory`, `erroroutofdevicememory`, `failed to allocate`, `buffer failed`, `graph allocation failed`, `cannot make enough memory available` or `memory allocation of size`; device lost on `errordevicelost` or `device lost`; a missing library on Windows' `0xC0000135` exit code, `error while loading shared libraries: <name>` or `library not loaded: <name>` (the name kept); a bad model on `invalid magic`, `failed to load model`, `unknown model` or `tensor '…' not found`; otherwise a non-zero exit is `Crashed`. (`--auto-fit` is on by default in this build and already spreads weights over the card, memory and disk; Studio never passes `--params-backend`, which turns it off.)

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p runtime --lib sd:: && cargo test -p runtime --lib llama`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/runtime/src/llama/lines.rs crates/runtime/src/sd crates/runtime/tests/fixtures
git commit -m "Studio engine: read its progress bar and stages, and name its failures"
```

### Task L4: Engine arguments from typed fields, devices, and the contract test

**Files:**
- Create: `crates/runtime/src/sd/args.rs`, `crates/runtime/src/sd/devices.rs`, `crates/runtime/tests/sd_contract.rs`; fixtures `sd_help.txt` (the flag lines of `sd-cli -h` as the release README and docs list them), `sd_list_devices_vulkan.txt`, `sd_list_devices_metal.txt`
- Test: inline; `sd_contract.rs`

**Interfaces:**
- Produces:
```rust
pub const PROMPT_MAX_CHARS: usize = 4_000;          // a sanity cap: the prompt goes by file, never in argv
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartRole { Diffusion, HighNoiseDiffusion, Llm, T5xxl, ClipL, ClipVision, Vae, Single }
pub fn flag_for(role: PartRole) -> &'static str;   // --diffusion-model, --high-noise-diffusion-model, --llm, --t5xxl, --clip_l, --clip_vision, --vae, -m
pub enum Sampler { Euler, EulerA, Heun, Dpm2, DpmPp2m, DpmPp2mV2, Lcm }      // whitelist; as_str() gives the CLI word
pub enum Scheduler { Discrete, Karras, Exponential, Simple }                 // whitelist of the pinned CLI's names; None = the engine's own default
pub enum OutputFormat { Png, Webm, Webp }
#[derive(Debug, Clone, PartialEq)]
pub struct SdJob { pub parts: Vec<(PartRole, PathBuf)>, pub prompt: String, pub negative: Option<String>, pub width: u32, pub height: u32,
    pub steps: u32, pub cfg: f32, pub flow_shift: Option<f32>, pub sampler: Option<Sampler>, pub scheduler: Option<Scheduler>, pub seed: u64,
    pub batch: u32, pub frames: Option<u32>, pub fps: Option<u32>, pub init_image: Option<PathBuf>, pub ref_images: Vec<PathBuf>,
    pub backend: Option<String>, pub memory_saving: bool, pub processor_only: bool, pub threads: Option<u32>,
    pub out_dir: PathBuf, pub out_stem: String, pub format: OutputFormat,
    pub env: Vec<(String, String)> }                                       // only from compiled-in quirks
pub enum ArgsError { EmptyPrompt, PromptTooLong, NotUnicode(PathBuf), NoDiffusionPart, BadSize, PercentInOutputPath }
pub fn build_args(job: &SdJob) -> Result<Vec<OsString>, ArgsError>;
/// Where `run_job` writes the prompt (and the negative prompt, when there is one) before the engine starts: inside `out_dir`.
pub fn prompt_files(job: &SdJob) -> (PathBuf, Option<PathBuf>);      // <out_dir>/prompt.txt, <out_dir>/negative.txt
/// Threads for a processor-only run: physical cores minus one, at least 1; physical cores are the logical count halved on x86-64.
pub fn processor_threads(logical_cores: usize, x86_64: bool) -> u32;
pub const FLAGS: &[&str];      // every flag build_args can emit; the contract test checks each against the pinned `-h`
// devices.rs
pub struct SdDevice { pub name: String, pub description: String }
pub fn parse_list_devices(stdout: &str) -> Vec<SdDevice>;     // one `name<TAB>description` a line, as the pinned CLI prints
pub fn match_llama_device(devices: &[SdDevice], llama_description: &str) -> Option<String>;   // by description, case-insensitive
```
Argument order: the parts (one fixed flag each), `-M vid_gen` for video, `--prompt-file <out_dir>/prompt.txt`, `--negative-prompt-file <out_dir>/negative.txt` when there is a negative prompt, `-W`, `-H`, `--steps`, `--cfg-scale`, `--flow-shift` when set, `--sampling-method`, `--scheduler` when set, `--seed`, `--batch-count`, `--video-frames`, `--fps`, `-i <init>`, `-r <ref>` each, `--backend <device>` (or `cpu` when `processor_only`), `--threads <n>` when set, `--vae-tiling --offload-to-cpu` when `memory_saving`, `-o <out_dir>/<out_stem>.<png|webm|webp>`, `--disable-image-metadata`, `--log-level info`. (The pinned parser matches only `arg == long_name`, so `--prompt=<text>` would be "unknown argument"; a string option takes the next argument as its value whatever it starts with, and `--prompt-file` reads the whole file as bytes.)

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn each_part_role_has_one_fixed_flag() {
    let a = strings(build_args(&job_with(vec![(PartRole::Diffusion, p("/m/d.gguf")), (PartRole::Llm, p("/m/q.gguf")), (PartRole::Vae, p("/m/v.safetensors"))])).unwrap());
    assert_eq!(&a[..6], ["--diffusion-model", "/m/d.gguf", "--llm", "/m/q.gguf", "--vae", "/m/v.safetensors"]);
}
#[test] fn the_prompt_goes_by_file_and_never_reaches_argv() {
    let job = SdJob { prompt: "-o /etc/x".into(), negative: Some("--vae evil".into()), ..job() };
    let a = strings(build_args(&job).unwrap());
    let (p, n) = prompt_files(&job);
    assert_eq!(after(&a, "--prompt-file"), p.to_str().unwrap()); assert_eq!(after(&a, "--negative-prompt-file"), n.unwrap().to_str().unwrap());
    assert!(!a.iter().any(|x| x.contains("/etc/x") || x.contains("evil") || x.starts_with("--prompt=")));
    assert_eq!(a.iter().filter(|x| *x == "-o").count(), 1);              // only the output's own
}
#[test] fn the_prompt_is_capped_at_4000_characters() {
    assert!(build_args(&SdJob { prompt: "a".repeat(4_000), ..job() }).is_ok());
    assert_eq!(build_args(&SdJob { prompt: "a".repeat(4_001), ..job() }), Err(ArgsError::PromptTooLong));
}
#[test] fn an_output_folder_that_looks_like_a_numbering_template_is_refused() {
    for dir in ["/g/shots %d", "/g/%03d", "/g/a%12d"] {
        assert_eq!(build_args(&SdJob { out_dir: dir.into(), ..job() }), Err(ArgsError::PercentInOutputPath), "{dir}");
    }
    assert!(build_args(&SdJob { out_dir: "/g/100% real".into(), ..job() }).is_ok());
}
#[test] fn the_metadata_flag_is_always_there() { assert!(strings(build_args(&job()).unwrap()).contains(&"--disable-image-metadata".into())); }
#[test] fn memory_saving_adds_tiling_and_offload_and_processor_only_picks_cpu_and_threads() {
    let a = strings(build_args(&SdJob { memory_saving: true, ..job() }).unwrap());
    assert!(a.contains(&"--vae-tiling".into()) && a.contains(&"--offload-to-cpu".into()));
    let a = strings(build_args(&SdJob { processor_only: true, threads: Some(5), backend: Some("vulkan0".into()), ..job() }).unwrap());
    assert_eq!((after(&a, "--backend"), after(&a, "--threads")), ("cpu", "5"));
    assert_eq!((processor_threads(12, true), processor_threads(10, false), processor_threads(2, true)), (5, 9, 1));
}
#[test] fn no_scheduler_means_the_engines_own() { assert!(!strings(build_args(&SdJob { scheduler: None, ..job() }).unwrap()).contains(&"--scheduler".into())); }
#[test] fn a_video_job_asks_for_frames_fps_shift_and_webm() {
    /* frames 49, fps 24, flow_shift Some(3.0) → "--video-frames" "49" "--fps" "24" "--flow-shift" "3"; output ends ".webm" */
}
#[test] fn every_flag_is_in_the_help_text() {
    let help = include_str!("../../tests/fixtures/sd_help.txt");
    for f in FLAGS { assert!(help.contains(f), "{f}"); }
}
#[test] fn devices_match_llamas_by_description() {
    let d = parse_list_devices(include_str!("../../tests/fixtures/sd_list_devices_vulkan.txt"));
    assert_eq!(match_llama_device(&d, "AMD Radeon RX 7600 (RADV NAVI33)").as_deref(), Some("vulkan0"));
    assert_eq!(match_llama_device(&d, "NVIDIA GeForce RTX 4070"), None);
}
// crates/runtime/tests/sd_contract.rs (each test returns early when SD_CLI_BIN is unset)
#[test] fn the_pinned_engine_knows_every_flag_studio_sends() { /* `$SD_CLI_BIN -h` through runtime::spawn: exit 0; every args::FLAGS entry appears */ }
#[test] fn the_pinned_engine_parses_the_exact_arguments_studio_builds() {
    // for a picture job (memory saving, processor only, threads) and a video job (frames, fps, flow shift, start frame): write the prompt
    // files, a tiny PNG for -i and the metadata image; run build_args(job) followed by `-M metadata --image <tiny.png>` (the last -M wins,
    // and metadata mode parses every option without loading a model) → exit 0, no "unknown argument" or "invalid" in the output
}
#[test] fn the_pinned_engine_lists_its_devices_in_the_format_studio_reads() { /* `--list-devices` → parse_list_devices non-empty (the CPU at least) */ }
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p runtime --lib sd::args && cargo test -p runtime --lib sd::devices && cargo test -p runtime --test sd_contract`
Expected: FAIL to compile, `cannot find function 'build_args'`.

- [ ] **Step 3: Implement.** Sizes must be multiples of 16 and between 64 and 2048 (`BadSize`); every path must be valid Unicode (`NotUnicode`, never a lossy conversion, as launch does); the `-o` path is refused when it matches `%\d{0,3}d` (`PercentInOutputPath`, the pinned CLI's `format_specifier_regex`); the prompt is trimmed only for the empty check, never rewritten, and only `run_job` (Task L5) writes it to its file. "See the exact command" shows the argv with the prompt file's path, and the prompt beside it (Task L14).

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p runtime --lib sd:: && cargo test -p runtime --test sd_contract`
Expected: PASS (the contract tests print "skipped" without `SD_CLI_BIN`; CI's `checks` job runs them).

- [ ] **Step 5: Commit**
```bash
git add crates/runtime/src/sd crates/runtime/tests/sd_contract.rs crates/runtime/tests/fixtures
git commit -m "Studio engine: arguments only from typed fields, the prompt by file, and devices matched to llama's"
```

### Task L5: Running one job: the fake engine, Stop, the out-of-memory retry and the stall

**Files:**
- Create: `crates/runtime/src/sd/run.rs`, `crates/runtime/src/sd/testing.rs` (`#[doc(hidden)] pub mod testing`, as `models::gguf::testing`), `crates/runtime/tests/sd_cli.rs`
- Modify: `crates/runtime/Cargo.toml` (`[[test]] name = "sd_cli" path = "tests/sd_cli.rs" harness = false`), `crates/runtime/src/sd/mod.rs`, `crates/runtime/src/lib.rs` (re-exports: `Engine`, `SdJob`, `LocalProgress`, `SdFailure`, `run_job`, `RunEvent`)
- Test: `crates/runtime/tests/sd_cli.rs` (its `main` is the fake engine when `RUNTIME_FAKE_SD` is set, as `reaper_process.rs` is the reaper with its hidden flag)

**Interfaces:**
- Consumes: Tasks L2–L4; `spawn::{spawn, SpawnSpec, EnvSpec, StdioMode}`, `llama::backend::path_with_vulkan_fallback`, `llama::launch::NO_BACKTRACE_ENV`.
- Produces:
```rust
pub struct RunLimits { pub stall_after: Duration }                 // Default: 600 s
pub enum RunEvent { Progress(LocalProgress), Stalled, Retrying(SdFailure) }
pub struct RunOutput { pub files: Vec<PathBuf>, pub retried: bool, pub elapsed_ms: u64 }
/// One `sd-cli` for `job` (and one more with memory saving after a first out-of-memory failure), after writing the prompt files.
/// Cancel kills the tree and waits for it; Cancel and every final failure remove `job.out_dir` (the prompt files with it).
pub async fn run_job(engine: &Engine, job: &SdJob, llama_dir: Option<&Path>, limits: RunLimits, cancel: CancellationToken,
    on_event: &(dyn Fn(RunEvent) + Send + Sync)) -> Result<RunOutput, SdFailure>;
pub async fn list_devices(engine: &Engine) -> Result<Vec<SdDevice>, SdFailure>;   // `--list-devices`, 30 s limit
// testing.rs
pub const FAKE_MODE_ENV: &str = "RUNTIME_FAKE_SD";        // ok | oom-once | oom | crash | hang | missing-dll | bad-model | slow
                                                           // (`--list-devices` prints the Vulkan devices fixture in every mode)
pub const FAKE_RECORD_ENV: &str = "RUNTIME_FAKE_SD_RECORD"; // a folder: argv-<n>.json and env-<n>.json per start
pub fn fake_main(args: &[String]) -> !;                    // prints a fixture with \r redraws, writes tiny files to -o, exits as the mode says
pub fn tiny_png() -> Vec<u8>; pub fn tiny_webm() -> Vec<u8>;
```

- [ ] **Step 1: Write the failing tests** (`crates/runtime/tests/sd_cli.rs`; each check is a function `main` calls, printing `sd_cli: <name> ... ok`; the engine is `Engine { program: current_exe(), extra_env: [(FAKE_MODE_ENV, mode), (FAKE_RECORD_ENV, dir)], .. }`)
```rust
fn a_picture_run_reports_its_stages_and_writes_its_file() {
    let out = rt().block_on(run_job(&fake("ok"), &picture_job(&dir), None, RunLimits::default(), CancellationToken::new(), &collect)).unwrap();
    assert_eq!(out.files, [dir.join(".partial/r1/r1.png")]);                 // sd-cli's own naming: <stem>.png for one picture
    assert_eq!(stages(&events), [Loading, ReadingPrompt, Drawing, Decoding, Saving]);
}
fn a_batch_comes_back_in_order() { /* batch 2 → files r1_0.png, r1_1.png (the fake names them as sd-cli does); a video → r1.webm */ }
fn the_engine_gets_exactly_the_flags_studio_built_and_the_prompt_by_file() {
    assert_eq!(recorded_argv(&rec, 0), strings(build_args(&job).unwrap()));
    assert_eq!(recorded_prompt(&rec, 0), job.prompt);                       // the fake read --prompt-file's bytes; no trailing newline
}
fn the_environment_is_scrubbed() {
    // recorded env names ⊆ the platform allowlist ∪ {GGML_NO_BACKTRACE, RUNTIME_FAKE_SD, RUNTIME_FAKE_SD_RECORD} ∪ job.env names
}
fn stop_kills_the_engine_and_its_child_within_a_second_and_removes_the_partial_folder() {
    // mode hang (the fake starts one grandchild through runtime::spawn and both sleep); cancel after the first progress line
    assert!(matches!(result, Err(SdFailure::Cancelled))); assert!(started.elapsed() < Duration::from_secs(1));
    assert!(!dir.join(".partial/r1").exists());
    #[cfg(unix)] assert!(!runtime::process_alive(grandchild_pid));
}
fn out_of_memory_retries_once_with_memory_saving() {
    // mode oom-once → Ok, out.retried; argv-1 contains "--vae-tiling" and "--offload-to-cpu", argv-0 neither;
    // events hold exactly one Retrying(OutOfMemory)
}
fn a_second_out_of_memory_is_named() { /* mode oom → Err(OutOfMemory); two starts recorded, no third */ }
fn a_failed_run_removes_its_partial_folder() {
    // modes oom, crash and bad-model, each after the fake wrote a half file → the Err, and !dir.join(".partial/r1").exists()
}
fn a_crash_a_missing_library_and_a_bad_model_are_named() { /* crash → Crashed{..}; missing-dll → MissingLibrary(_); bad-model → BadModel */ }
fn no_progress_for_the_stall_limit_says_so_and_keeps_running() {
    // mode slow (one progress line, then 400 ms of silence, then the rest); stall_after 200 ms → one Stalled event, then Ok
}
fn an_engine_that_cannot_start_is_named() {
    // program path that does not exist → EngineMissing; on Unix a file without the execute bit → EngineBlocked(_)
}
fn the_devices_are_listed() { /* any mode → list_devices == the vulkan fixture's devices (name TAB description) */ }
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p runtime --test sd_cli`
Expected: FAIL to compile, `cannot find function 'run_job'`.

- [ ] **Step 3: Implement** `run_job` on the pattern of `llama::fit_params::run`: create `out_dir`, write the prompt files of `prompt_files(job)` (UTF-8, no trailing newline); `SpawnSpec::new(&engine.program, &engine.dir)`, args `engine.leading_args` then `build_args(job)`, env `EnvSpec::minimal().set(NO_BACKTRACE_ENV, "1")` plus, on Windows, `PATH` from `path_with_vulkan_fallback(llama_dir, PATH, llama_dir/vulkan/vulkan-1.dll exists, ';')` (llama's installed folder, so a PC whose driver lacks the Vulkan runtime still finds it), then `job.env`, then `engine.extra_env`; stdin null, stdout and stderr piped and both read with `LineReader::splitting_cr` into one `ProgressParser`; the last 60 lines of both, merged in arrival order, kept for `classify`. A spawn error of kind `NotFound` is `EngineMissing`, `PermissionDenied` is `EngineBlocked`. Exit 0 → the files in `out_dir` whose names start with `out_stem` (not the prompt files), ordered by their numeric `_<n>` suffix (none for a single picture or a video). `OutOfMemory` on a job without `memory_saving` → `Retrying`, the partial outputs removed (the prompt files stay), the same job once more with `memory_saving = true`. Cancel → `child.start_kill()` (the whole tree), `wait`, remove `out_dir`; every final `Err` also removes `out_dir`. The stall timer resets on every progress change and fires `Stalled` once; it never kills. The fake: modes as listed, its progress from `include_str!` of the Task L3 fixtures (with `\r`, `ESC[K` and the stdout/stderr split of the real engine), its outputs named as `sd-cli`'s `save_results` names them (`<stem>.png` for one picture, `<stem>_0.png`, `<stem>_1.png`, … for a batch, `<stem>.webm` for `-M vid_gen`), a copy of the prompt file's bytes in the record folder; `oom-once` leaves a marker file in the record folder so the second start succeeds.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p runtime --test sd_cli && bash scripts/check-spawn.sh`
Expected: PASS; `spawn check passed`.

- [ ] **Step 5: Commit**
```bash
git add crates/runtime/Cargo.toml crates/runtime/src crates/runtime/tests/sd_cli.rs
git commit -m "Studio engine: run one job, stop it at once, retry once after running out of memory, and say when it stalls"
```

### Task L6: Picture and video entries in the signed catalog

**Files:**
- Create: `crates/models/src/gen_catalog.rs`, `crates/models/tests/fixtures/catalog/generators.json` (one chat model, plus made-up picture and video entries and family records in the real shape, including broken ones), `third_party/licenses/models/apache-2.0.txt` (the canonical Apache License 2.0 text from apache.org, unchanged)
- Modify: `crates/models/src/catalog.rs` (three raw fields on `Catalog`; the v005 compatibility test reads `generators.json`), `crates/models/src/lib.rs` (`pub mod gen_catalog;`)
- Test: inline in `gen_catalog.rs` and `catalog.rs`

**Interfaces:**
- Consumes: `catalog::Catalog`, `hf::{validate_repo_id, validate_repo_path}`.
- Produces:
```rust
// catalog.rs, on Catalog: raw so that one bad entry never fails the chat catalog, and old parsers ignore them
#[serde(default, skip_serializing_if = "BTreeMap::is_empty")] pub generators: BTreeMap<String, serde_json::Value>,
#[serde(default, skip_serializing_if = "serde_json::Value::is_null")] pub generator_tiers: serde_json::Value,
#[serde(default, skip_serializing_if = "BTreeMap::is_empty")] pub generator_families: BTreeMap<String, serde_json::Value>,
// gen_catalog.rs (serde camelCase; enums kebab-case as shown)
pub enum GenKind { Image, ImageEdit, Video }                                   // "image" | "image-edit" | "video"
pub enum GenFamily { Flux2Klein4b, ZImageTurbo, Wan22Ti2v5bFast, Wan21T2v1_3b }   // "flux2-klein-4b" | "z-image-turbo" | "wan2.2-ti2v-5b-fast" | "wan2.1-t2v-1.3b"
                                                                                // (the full Wan 2.2 5B is not a family: about 30–50 steps, cfg 5)
pub enum GenRole { Diffusion, HighNoiseDiffusion, Llm, T5xxl, ClipL, ClipVision, Vae, Model }
pub enum GenInput { Reference, StartFrame, EndFrame }
pub enum Quirk { MetalTensorOff, VideoWebpOnMac }                              // "metal-tensor-off" | "video-webp-on-mac"
pub enum Commercial { Yes, UnderRevenue, Registration, Unclear, No }
pub enum Badge { Green, Amber, Red }
pub enum GateMode { None, Auto, Manual }
pub enum FrameRule { FourNPlusOne, EightNPlusOne }                             // "4n+1" | "8n+1"
pub struct GenLicence { pub id: String, pub name: String, pub url: String, pub sha256: String, pub commercial_outputs: Commercial,
    pub revenue_limit_usd: Option<u64>, pub excluded_regions: Vec<String>, pub obligations: Vec<String>, pub plain_words: String }
impl GenLicence { pub fn badge(&self) -> Badge; }                              // Yes → Green; UnderRevenue, Registration → Amber; Unclear, No → Red
pub struct GenPart { pub role: GenRole, pub repo: String, pub revision: String, pub path: String, pub size: u64, pub sha256: String, pub license: String }
pub struct GenGate { pub mode: GateMode, pub collects: Vec<String> }
pub struct GenDefaults { pub steps: u32, pub cfg: f32, pub flow_shift: Option<f32>, pub sampler: Option<String>, pub scheduler: Option<String>,
    pub width: u32, pub height: u32, pub frames: Option<u32>, pub fps: Option<u32> }
pub struct GenLimits { pub multiple_of: u32, pub max_side: u32, pub frame_rule: Option<FrameRule>, pub max_frames: Option<u32> }
pub struct FitFallback { pub act_base_bytes: u64, pub act_bytes_per_token: u64, pub vae_bytes_per_pixel_frame: u64, pub vae_tile_cap_bytes: u64,
    pub token_div: u32, pub frame_div: Option<u32>,      // token_div 16, or 32 for Wan 2.2 TI2V; frame_div 4 for Wan
    pub vae_chunk_frames: Option<u32> }                  // frames decoded at once: Wan decodes one latent frame (4 frames) at a time; None = all
pub struct MemoryPoint { pub width: u32, pub height: u32, pub frames: u32, pub peak_bytes: u64, pub device: String, pub build: u32 }
pub struct SpeedPoint { pub device_class: String, pub width: u32, pub height: u32, pub frames: u32, pub load_seconds: f64,
    pub seconds_per_step: f64, pub decode_seconds: f64, pub measured_on: String }   // device_class "<backend>-<GB>": "vulkan-8", "metal-16", "cpu"
pub struct GenEntry { pub id: String, pub name: String, pub summary: String, pub publisher: String, pub kind: GenKind, pub family: GenFamily,
    pub inputs: Vec<GenInput>, pub parts: Vec<GenPart>, pub licenses: BTreeMap<String, GenLicence>, pub gate: GenGate, pub defaults: GenDefaults,
    pub limits: GenLimits, pub fallback: FitFallback, pub memory: Vec<MemoryPoint>, pub speed: Vec<SpeedPoint>, pub quirks: Vec<Quirk>,
    pub min_build: u32, pub runs_on_apple: bool, pub tested: Option<String> }  // runs_on_apple: default true; fallback: default the family's
impl GenEntry { pub fn total_bytes(&self) -> u64; pub fn diffusion(&self) -> &GenPart; pub fn effective_licence(&self) -> &GenLicence; }
pub struct FamilyRecord { pub family: GenFamily, pub name: String, pub publisher: String, pub kind: GenKind, pub inputs: Vec<GenInput>,
    pub upstream_license: String, pub companions: Vec<GenPart>, pub licenses: BTreeMap<String, GenLicence>, pub defaults: GenDefaults,
    pub limits: GenLimits, pub fallback: FitFallback, pub memory: Vec<MemoryPoint>, pub speed: Vec<SpeedPoint>, pub quirks: Vec<Quirk>,
    pub min_build: u32, pub runs_on_apple: bool,
    pub parameters: (u64, u64) }                         // the diffusion model's parameter count band (4B klein: 3.5e9..=4.5e9), checked when a listing states it
pub struct GenTiers { pub image: BTreeMap<u64, Vec<String>>, pub video: BTreeMap<u64, Vec<String>> }   // usable memory in GB → ids, best first
pub struct Hidden { pub id: String, pub reason: String }
pub struct GenCatalog { pub entries: BTreeMap<String, GenEntry>, pub families: BTreeMap<GenFamily, FamilyRecord>, pub tiers: GenTiers, pub hidden: Vec<Hidden> }
pub const ALLOWED_PART_EXTENSIONS: [&str; 2] = [".gguf", ".safetensors"];
/// Canonical licence texts bundled with the app (third_party/licenses/models/), by licence id: the catalog pins their SHA-256.
pub const BUNDLED_LICENCE_TEXTS: &[(&str, &str)];        // [("apache-2.0", include_str!("../../../third_party/licenses/models/apache-2.0.txt"))]
pub fn gen_catalog(catalog: &Catalog, engine_build: u32) -> GenCatalog;   // the app passes runtime::sd::lock::embedded().sd_cpp.build
```

- [ ] **Step 1: Write the failing tests**
```rust
const GEN: &str = include_str!("../tests/fixtures/catalog/generators.json");
#[test] fn generators_are_read_beside_the_chat_models() {
    let c = Catalog::parse(GEN).unwrap();
    assert_eq!(c.models.len(), 1);
    let g = gen_catalog(&c, 1106);
    assert_eq!(g.entries.keys().map(String::as_str).collect::<Vec<_>>(), ["sample-image", "sample-video"]);
    assert_eq!((g.entries["sample-video"].kind, g.entries["sample-video"].limits.frame_rule), (GenKind::Video, Some(FrameRule::FourNPlusOne)));
    assert_eq!(g.tiers.image[&8], ["sample-image"]);
}
#[test] fn an_unknown_family_role_or_quirk_hides_only_that_entry() {
    let reasons: BTreeMap<_, _> = gen_catalog(&Catalog::parse(GEN).unwrap(), 1106).hidden.into_iter().map(|h| (h.id, h.reason)).collect();
    for id in ["bad-family", "bad-role", "bad-quirk"] { assert!(reasons.contains_key(id), "{id}"); }
}
#[test] fn an_entry_needing_a_newer_engine_is_hidden() {
    let g = gen_catalog(&Catalog::parse(GEN).unwrap(), 1105);              // sample-image has "minBuild": 1106
    assert!(g.hidden.iter().any(|h| h.id == "sample-image" && h.reason == "needs a newer Studio engine"));
}
#[test] fn pickle_files_are_refused() {
    for ext in [".ckpt", ".pt", ".pth", ".bin"] { assert!(hidden_with_part_path(&format!("model{ext}")), "{ext}"); }
    assert!(!hidden_with_part_path("Model.SAFETENSORS"));
}
#[test] fn every_part_is_pinned() { /* revision of 39 hex, sha256 of 63 hex, size 0, a repo path with ".." → each hides its entry */ }
#[test] fn the_strictest_licence_of_all_parts_decides() {
    let e = entry_with_licences(&["apache-2.0", "kommunity"]);             // kommunity: UnderRevenue, 5_000_000
    assert_eq!((e.effective_licence().id.as_str(), e.effective_licence().badge()), ("kommunity", Badge::Amber));
    assert_eq!(entry_with_licences(&["apache-2.0", "kommunity", "flux-nc"]).effective_licence().badge(), Badge::Red);
}
#[test] fn a_region_limited_licence_is_never_offered() { /* a part licence with excludedRegions ["EU"] → hidden, reason "region-limited" */ }
#[test] fn a_licence_hash_is_the_bundled_texts() {
    // an "apache-2.0" licence whose sha256 is not sha256(BUNDLED_LICENCE_TEXTS["apache-2.0"]) → hidden, reason "licence text does not match";
    // a licence id with no bundled text keeps its own pinned sha256
}
#[test] fn broken_generators_never_fail_the_catalog() {
    let c = Catalog::parse(&with(GEN, r#""generators": {"x": 5}, "generatorTiers": "nope""#)).unwrap();
    let g = gen_catalog(&c, 1106);
    assert!(g.entries.is_empty() && g.tiers.image.is_empty() && g.hidden.iter().any(|h| h.id == "x"));
}
#[test] fn an_entry_without_a_fallback_takes_its_familys() { /* sample-image has none; its fallback == families[&Flux2Klein4b].fallback */ }
// catalog.rs
#[test] fn an_older_parser_reads_a_catalog_with_generators() {
    let old: v005::Catalog = serde_json::from_str(GEN_CATALOG).unwrap();
    assert_eq!(old.models.len(), 1);
}
#[test] fn writing_a_catalog_keeps_its_generators_and_adds_none_to_others() {
    assert!(serde_json::to_value(Catalog::parse(GEN_CATALOG).unwrap()).unwrap()["generators"].as_object().unwrap().len() >= 2);
    assert!(serde_json::to_value(Catalog::parse(TEST_CATALOG).unwrap()).unwrap().get("generators").is_none());
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p models --lib gen_catalog && cargo test -p models --lib catalog`
Expected: FAIL to compile, `could not find 'gen_catalog' in the crate root`.

- [ ] **Step 3: Implement.** Each raw entry is read with `serde_json::from_value::<GenEntryRaw>` (an unknown enum value fails it: hidden "unknown family, role, input, kind or quirk"), then checked: `minBuild` ≤ `engine_build` ("needs a newer Studio engine"); every part's repo and path pass `validate_repo_id`/`validate_repo_path` and end in an allowed extension (case-insensitive; "not a .gguf or .safetensors file"); 40-hex revision, 64-hex SHA-256, size > 0 ("a part is not pinned"); every part's `license` is a key of `licenses` ("a part has no licence"); a licence with `excludedRegions` ("region-limited"); a licence whose id has a bundled text and whose `sha256` is not that text's ("licence text does not match"); exactly one `diffusion` or `model` part ("no model part"). Strictness: `No` > `Unclear` > `Registration` > `UnderRevenue` (a lower revenue limit is stricter) > `Yes`. Family records are read the same way, keyed by family; an entry without `fallback`, `speed` or `runsOnApple` takes its family's. Tiers keys that are not numbers are dropped; ids not among the entries are dropped. `Catalog::validate` is unchanged: generators never fail the chat catalog.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p models`
Expected: PASS (the existing catalog tests too).

- [ ] **Step 5: Commit**
```bash
git add crates/models/src/catalog.rs crates/models/src/gen_catalog.rs crates/models/src/lib.rs crates/models/tests/fixtures/catalog/generators.json third_party/licenses/models
git commit -m "Studio: picture and video entries in the signed catalog, hidden one by one when they cannot be used"
```

### Task L7: The recommended list, the known-good family table, tensor signatures, and Hugging Face search

**Files:**
- Create: `crates/models/src/gen_search.rs`; fixtures in `crates/models/tests/fixtures/hf/`: `gen-search-text-to-image.json` (the search answer for "klein", recorded on the day of pinning), `gen-search-image-to-video.json` (for "wan2.2 5b": holds a Turbo repository tagged `image-to-video`), `gen-search-untagged.json` (holds `Green-Sky/FastWan2.2-TI2V-5B-FullAttn-GGUF`, which has no pipeline tag), `repo-flux2-klein-4b-gguf.json`, `repo-z-image-turbo-gguf.json`, `repo-fastwan22-ti2v-5b-gguf.json`, `repo-wan22-ti2v-5b-full-gguf.json` (QuantStack's full Wan 2.2 5B), one listing per companion repository the starters use (`unsloth/Qwen3-4B-GGUF`, `unsloth/Qwen3-4B-Instruct-2507-GGUF`, `Comfy-Org/Flux-2-Klein-4B`, `Comfy-Org/Z-Image-Turbo`, `city96/umt5-xxl-encoder-gguf`) (listings recorded with `blobs=true` and the `expand[]` fields `HfClient::repo` asks for), `repo-flux2-klein-9b-gguf.json`, `repo-klein-4b-name-9b-parameters.json` (a 4B-named repository whose listing states 9 billion parameters), `repo-unknown-diffusion.json`, `repo-uncensored.json`, `repo-no-hashes.json` (hand-made in the same shape); in `crates/models/tests/fixtures/gguf/`: `klein4b-tensors.txt`, `klein9b-tensors.txt`, `zimage-tensors.txt`, `wan22-ti2v-5b-tensors.txt`, `wan21-t2v-1.3b-tensors.txt`, `qwen3-4b-tensors.txt` (each tensor's name and dimensions, one a line, read at pinning from the first 256 KiB of each file through a Range request; leejet's and Green-Sky's files hold no metadata keys at all, so there is no `general.architecture` to read)
- Modify: `crates/models/src/hf.rs` (`GenSearchKind`, `search_generators`, `merge_generator_searches`; `RepoSummary` and `RepoDetail` gain `not_for_all_audiences`; the item-reading half of `search_url` becomes `fn parse_search_items(arr: &[Value]) -> Vec<RepoSummary>`), `crates/models/src/gguf.rs` (`TensorShape`, `read_tensor_shapes`, `testing::Writer::shaped_tensor`), `crates/models/src/arch.rs` (`is_diffusion`), `crates/models/src/lib.rs`, `catalog/models.json` (below; `sequence` 2 → 3), `src-tauri/src/hub.rs` (`import_model` refuses a picture or video GGUF)
- Test: inline in `gen_search.rs`, `hf.rs`, `gguf.rs`, `arch.rs`, `hub.rs`; `catalog::tests::bundled_catalog_is_valid` stays green

**Interfaces:**
- Consumes: Task L6 types; `hf::{RepoDetail, RepoFile, HubInfo, GgufInfo}`, `files::quant_label`, `gguf`'s tensor-table reader.
- Produces:
```rust
// hf.rs
pub enum GenSearchKind { Pictures, Video }
impl HfClient {
    /// Three queries, each `filter=gguf` with the text, the sort, limit 30 and the `expand[]` of `search`: pipeline_tag text-to-image and
    /// image-to-image (Pictures) or text-to-video and image-to-video (Video), and one without a pipeline tag; merged; NFAA dropped.
    pub async fn search_generators(&self, text: &str, kind: GenSearchKind) -> Result<Vec<RepoSummary>, HfError>;
}
/// One list by repository id, most downloads first; an untagged item stays only when its name matches a family of this kind.
pub fn merge_generator_searches(tagged: Vec<Vec<RepoSummary>>, untagged: Vec<RepoSummary>, kind: GenSearchKind) -> Vec<RepoSummary>;
// gguf.rs
pub struct TensorShape { pub name: String, pub dims: Vec<u64> }
pub fn read_tensor_shapes(path: &Path) -> Result<Vec<TensorShape>, GgufError>;   // names and dimensions as written; the same refusals as read_tensor_table
// testing: Writer::shaped_tensor(self, name: &str, dims: &[u64]) -> Self; pub fn shapes(fixture: &str) -> Vec<TensorShape> (one "name d0 d1 …" a line);
//          pub fn diffusion_gguf(fixture: &str) -> Vec<u8> (that table with 32 bytes of data per tensor (offsets, not dimensions, size a table)
//          and no metadata keys: the tiny model file Tasks L11, L14, D1 and D2 write)
// arch.rs
pub fn is_diffusion(architecture: &str) -> bool;  // a general.architecture llama cannot load that a picture or video model uses (a hint when present)
// gen_search.rs
pub enum NotOffered { RegionLimited, Uncensored, AdultContent }
pub enum Recognition { Supported { family: GenFamily, files: Vec<RepoFile> }, NotOffered(NotOffered), NotSupportedYet(String) }
pub fn recognise(detail: &RepoDetail, families: &BTreeMap<GenFamily, FamilyRecord>) -> Recognition;
pub enum NameMatch { Family(GenFamily), UncheckedWan22, Several, None }
pub fn family_of_name(name: &str) -> NameMatch;                         // FAMILY_TOKENS only: the untagged search and `recognise`
/// The family of a local GGUF diffusion file, from its tensor table (Tasks L11 and L14, and the chat import).
pub fn family_from_tensors(shapes: &[TensorShape]) -> Option<GenFamily>;
pub fn looks_like_diffusion(shapes: &[TensorShape]) -> bool;           // a known signature, or a picture/video model's tensor names
pub struct SearchedLicence { pub licence: GenLicence, pub label_mismatch: Option<String> }   // "The repository says MIT; FLUX.2 klein 4B is Apache 2.0."
pub fn searched_licence(detail: &RepoDetail, record: &FamilyRecord) -> SearchedLicence;
/// The entry a searched file makes: its diffusion part (this repo, this commit, the file's published SHA-256) plus the
/// record's companions, defaults, limits, fallback and speed. Id "hf:<owner>/<repo>:<file>".
pub fn entry_for(detail: &RepoDetail, file: &RepoFile, record: &FamilyRecord) -> Result<GenEntry, String>;
```
The recognition tables, compiled in (family ids, name tokens and tensor signatures decide engine flags and quirks, so they are code; everything else about a family is catalog data):
```rust
const FAMILY_TOKENS: &[(GenFamily, &[&str])] = &[
    (GenFamily::Flux2Klein4b, &["flux.2-klein-4b", "flux2-klein-4b", "flux-2-klein-4b"]),
    (GenFamily::ZImageTurbo, &["z-image-turbo"]),
    (GenFamily::Wan22Ti2v5bFast, &["fastwan2.2-ti2v-5b"]),     // FastVideo's distillation, the family's upstream (Apache-2.0)
    (GenFamily::Wan21T2v1_3b, &["wan2.1-t2v-1.3b"]),
];
/// The full Wan 2.2 5B (about 30–50 steps, cfg 5) and other distillations harnessUI has not checked (a "turbo" whose base states no licence).
const UNCHECKED_WAN22_TOKENS: &[&str] = &["wan2.2-ti2v-5b"];
const REGION_LIMITED_TOKENS: &[&str] = &["hunyuanvideo", "hunyuan-video", "hunyuanimage", "minimax-h3"];
/// From the pinned files' tensor tables (the fixtures above): names that must be there, a width read from one tensor's dimensions,
/// and the block count. FLUX.2 klein 4B: double_blocks.* and single_blocks.*, width 3072, its recorded block counts. Z-Image Turbo:
/// cap_embedder.*, width 3840. Wan 2.2 TI2V-5B: blocks.<n>.*, width 3072, 30 blocks, a patch_embedding of 48 input channels (the fast and
/// full versions share it, so the name decides between them). Wan 2.1 T2V-1.3B: width 1536, 30 blocks, 16 channels. Anything else: None.
const SIGNATURES: &[TensorSignature];
```
Names are compared lower-cased with `_` and spaces read as `-`, against the repository id and each file name. `catalog/models.json` gains, with every repository the **canonical id** its listing's `id` field gives (Comfy-Org renamed `flux2-klein-4B` to `Flux-2-Klein-4B` and `z_image_turbo` to `Z-Image-Turbo`; the old names answer 307 and would put files under a second folder), every revision, path, size and SHA-256 read from that listing on the day of pinning, and each licence's `sha256` that of its bundled canonical text (Task L6), its `url` the upstream model's page at a pinned revision (never a mirror's label):
- `generators` (each with `name`, one-line `summary` and `publisher`: "FLUX.2 klein 4B" / "Quick pictures in 4 steps; can also change a picture you add." / "Black Forest Labs"; "Z-Image Turbo" / "Photo-real pictures in 8 steps." / "Tongyi (Alibaba)"; "Wan 2.2 5B, fast" / "Short clips from words, or from a start picture." / "Wan (Alibaba)"): `flux2-klein-4b-q4` (kind `image`, inputs `["reference"]`: `leejet/FLUX.2-klein-4B-GGUF` `flux-2-klein-4b-Q4_0.gguf`, `unsloth/Qwen3-4B-GGUF` `Qwen3-4B-Q4_K_M.gguf` as `llm`, and the Apache decoder copy `Comfy-Org/Flux-2-Klein-4B` `split_files/vae/flux2-vae.safetensors`; steps 4, cfg 1.0, euler, 1024×1024), `z-image-turbo-q4` (kind `image`: `leejet/Z-Image-Turbo-GGUF` `z_image_turbo-Q4_K.gguf`, `unsloth/Qwen3-4B-Instruct-2507-GGUF` Q4_K_M as `llm`, `Comfy-Org/Z-Image-Turbo` `split_files/vae/ae.safetensors`; steps 8, cfg 1.0; quirk `metal-tensor-off`), and the provisional video entry `wan22-ti2v-5b-fast-q6` (`Green-Sky/FastWan2.2-TI2V-5B-FullAttn-GGUF` `FastWan2.2-TI2V-5B-q6_k.gguf`, `city96/umt5-xxl-encoder-gguf` `umt5-xxl-encoder-Q5_K_M.gguf` as `t5xxl`, `QuantStack/Wan2.2-TI2V-5B-GGUF` `VAE/Wan2.2_VAE.safetensors` as `vae`; inputs `["startFrame"]`; steps 4, cfg 1.0, flow shift 3.0, 832×480, 49 frames, 24 fps; multiple of 32, frame rule `4n+1`, at most 121 frames). Task L17 confirms or corrects the video entry from the benchmark and adds `wan21-t2v-1.3b` (fp16 `.safetensors`, text only, 832×480, 33 frames, 16 fps) when it calls for it. hum-ma's Turbo GGUF is not used (its base declares no licence). Every part Apache-2.0, `gate` none, `memory` and `speed` empty until Task L17 fills them.
- `generatorTiers`: `{"image": {"0": ["flux2-klein-4b-q4"], "8": ["z-image-turbo-q4", "flux2-klein-4b-q4"]}, "video": {"8": ["wan22-ti2v-5b-fast-q6"]}}`.
- `generatorFamilies`: one record per `GenFamily` with the same companions and licences, `upstreamLicense` `apache-2.0`, the parameter bands (FLUX.2 klein 4B 3.5–4.5 billion, Z-Image Turbo 5.5–6.5, Wan 2.2 TI2V-5B 4.5–5.5, Wan 2.1 1.3B 1.1–1.5), and these fallbacks until measured points exist, chosen so the starters' published needs hold with the formulas of Task L8 (klein green on an 8 GB card at 1024²; Z-Image one part at a time on 6–8 GB; Wan 2.2 fast one part at a time on 8 GB at 480p, 49 frames): the two picture families `{actBaseBytes: 5e8, actBytesPerToken: 100000, vaeBytesPerPixelFrame: 300, vaeTileCapBytes: 5e8, tokenDiv: 16}`; `wan2.2-ti2v-5b-fast` `{1e9, 250000, 400, 2.5e9, tokenDiv: 32, frameDiv: 4, vaeChunkFrames: 4}`; `wan2.1-t2v-1.3b` `{5e8, 100000, 300, 1.5e9, tokenDiv: 16, frameDiv: 4, vaeChunkFrames: 4}`.

- [ ] **Step 1: Write the failing tests**
```rust
// gen_search.rs
#[test] fn the_recommended_list_is_free_for_any_use_and_ungated() {
    let g = gen_catalog(&Catalog::bundled(), runtime_build());   // test helper: sd_cpp.build of include_str!("../../../sd.lock.json")
    let ids: BTreeSet<_> = g.entries.keys().map(String::as_str).collect();
    assert!(ids.is_superset(&BTreeSet::from(["flux2-klein-4b-q4", "z-image-turbo-q4"])), "{ids:?}");
    let video: Vec<_> = ids.iter().filter(|id| g.entries[**id].kind == GenKind::Video).collect();
    assert!(matches!(video.len(), 1 | 2) && video.iter().all(|id| id.starts_with("wan22-ti2v-5b-fast") || **id == "wan21-t2v-1.3b"), "{video:?}");
    assert!(matches!(ids.len(), 3 | 4)); assert!(g.hidden.is_empty(), "{:?}", g.hidden);   // Wan 2.1 only as Task L17 records it
    for e in g.entries.values() {
        assert_eq!((e.effective_licence().badge(), e.effective_licence().commercial_outputs), (Badge::Green, Commercial::Yes), "{}", e.id);
        assert_eq!(e.gate.mode, GateMode::None);
        assert!(e.speed.iter().all(|p| !p.measured_on.is_empty()), "{}: a speed point without where it was measured", e.id);
    }
    assert_eq!(g.families.len(), 4);
}
#[test] fn every_part_repo_is_its_listings_canonical_id() {
    // for each part of each bundled entry and family record: the recorded listing of that repo has `id` == part.repo,
    // and holds part.path with part.size and part.sha256 at part.revision
}
#[test] fn a_recognised_repo_lists_its_ggufs_that_have_a_published_hash() {
    let Recognition::Supported { family, files } = recognise(&detail("repo-flux2-klein-4b-gguf.json"), &families()) else { panic!() };
    assert_eq!(family, GenFamily::Flux2Klein4b);
    assert!(!files.is_empty() && files.iter().all(|f| f.path.ends_with(".gguf") && f.sha256.is_some()));
}
#[test] fn a_searched_file_gets_the_familys_companions() {
    let d = detail("repo-z-image-turbo-gguf.json"); let rec = &families()[&GenFamily::ZImageTurbo];
    let e = entry_for(&d, &d.files[0], rec).unwrap();
    assert_eq!(e.id, format!("hf:{}:{}", d.id, d.files[0].path));
    assert_eq!(e.parts.iter().map(|p| p.role).collect::<Vec<_>>(), [GenRole::Diffusion, GenRole::Llm, GenRole::Vae]);
    assert_eq!((e.parts[0].revision.as_str(), e.total_bytes()), (d.commit.as_str(), d.files[0].size + rec.companions.iter().map(|p| p.size).sum::<u64>()));
}
#[test] fn a_9b_klein_is_not_supported_yet() {
    assert!(matches!(recognise(&detail("repo-flux2-klein-9b-gguf.json"), &families()), Recognition::NotSupportedYet(_)));
    assert!(matches!(recognise(&detail("repo-klein-4b-name-9b-parameters.json"), &families()),
        Recognition::NotSupportedYet(r) if r.contains("different size")));      // the listing's gguf.parameters is outside the 4B band
}
#[test] fn the_full_wan_2_2_is_not_the_fast_one() {
    assert!(matches!(recognise(&detail("repo-wan22-ti2v-5b-full-gguf.json"), &families()), Recognition::NotSupportedYet(r) if r.contains("fast version")));
    assert!(matches!(recognise(&detail("repo-fastwan22-ti2v-5b-gguf.json"), &families()),
        Recognition::Supported { family: GenFamily::Wan22Ti2v5bFast, .. }));
}
#[test] fn families_are_told_apart_by_their_tensor_tables() {
    // each fixture table written with Writer::shaped_tensor and no metadata keys, read back with read_tensor_shapes
    assert_eq!(family_from_tensors(&table("klein4b-tensors.txt")), Some(GenFamily::Flux2Klein4b));
    assert_eq!(family_from_tensors(&table("klein9b-tensors.txt")), None);            // a 9B file named like the 4B is never taken for it
    assert_eq!(family_from_tensors(&table("zimage-tensors.txt")), Some(GenFamily::ZImageTurbo));
    assert_eq!(family_from_tensors(&table("wan22-ti2v-5b-tensors.txt")), Some(GenFamily::Wan22Ti2v5bFast));
    assert_eq!(family_from_tensors(&table("wan21-t2v-1.3b-tensors.txt")), Some(GenFamily::Wan21T2v1_3b));
    assert!(looks_like_diffusion(&table("klein9b-tensors.txt")) && !looks_like_diffusion(&table("qwen3-4b-tensors.txt")));
}
#[test] fn an_unknown_family_says_not_supported_yet() {
    assert!(matches!(recognise(&detail("repo-unknown-diffusion.json"), &families()), Recognition::NotSupportedYet(_)));
}
#[test] fn an_uncensored_or_region_limited_repo_is_not_offered() {
    assert_eq!(recognise(&detail("repo-uncensored.json"), &families()), Recognition::NotOffered(NotOffered::Uncensored));
    assert_eq!(recognise(&renamed("repo-unknown-diffusion.json", "someone/HunyuanVideo-1.5-GGUF"), &families()),
        Recognition::NotOffered(NotOffered::RegionLimited));
}
#[test] fn a_repo_without_any_published_hash_offers_nothing() {
    assert!(matches!(recognise(&detail("repo-no-hashes.json"), &families()), Recognition::NotSupportedYet(r) if r.contains("checksum")));
}
#[test] fn the_licence_is_the_stricter_of_the_family_and_the_label() {
    let rec = &families()[&GenFamily::ZImageTurbo];
    let l = searched_licence(&relabelled("repo-z-image-turbo-gguf.json", Some("mit")), rec);
    assert_eq!(l.licence.id, "apache-2.0"); assert!(l.label_mismatch.unwrap().contains("MIT"));
    assert_eq!(searched_licence(&relabelled("repo-z-image-turbo-gguf.json", Some("cc-by-nc-4.0")), rec).licence.badge(), Badge::Red);
    let other = searched_licence(&relabelled("repo-z-image-turbo-gguf.json", Some("other")), rec);
    assert_eq!((other.licence.commercial_outputs, other.licence.plain_words.as_str()), (Commercial::Unclear, "Check the licence before you use what you make."));
    assert_eq!(searched_licence(&relabelled("repo-z-image-turbo-gguf.json", None), rec).licence.id, "apache-2.0");   // states none: the original's
}
// hf.rs
#[test] fn generator_search_drops_repos_not_for_all_audiences() {
    let items = parse_search_items(fixture_array("gen-search-text-to-image.json-with-one-nfaa-item"));
    assert!(items.iter().any(|i| i.not_for_all_audiences));            // read from the raw tags
    assert!(merge_generator_searches(vec![items], vec![], GenSearchKind::Pictures).iter().all(|i| !i.not_for_all_audiences));
}
#[test] fn generator_search_asks_three_ways() {
    // the built URLs for Video: filter=gguf with pipeline_tag=text-to-video, with pipeline_tag=image-to-video, and with no pipeline_tag;
    // none has "conversational"
}
#[test] fn video_search_finds_turbo_and_untagged_repos() {
    let m = merge_generator_searches(vec![items("gen-search-image-to-video.json")], items("gen-search-untagged.json"), GenSearchKind::Video);
    assert!(m.iter().any(|i| i.id.to_lowercase().contains("turbo")));                 // shown, then "Not supported yet" when opened
    assert!(m.iter().any(|i| i.id == "Green-Sky/FastWan2.2-TI2V-5B-FullAttn-GGUF"));
    assert!(m.iter().all(|i| !i.id.contains("Qwen3")));                               // untagged items need a family name of this kind
}
// gguf.rs
#[test] fn tensor_shapes_read_back_as_written() { /* Writer::new().shaped_tensor("img_in.weight", &[128, 3072]) → read_tensor_shapes dims [128, 3072] */ }
// arch.rs
#[test] fn picture_and_video_architectures_are_known() { /* "flux", "wan", "sd3", "hunyuan-video" → true; "llama", "qwen3" → false */ }
// src-tauri/src/hub.rs
#[test] fn importing_a_picture_model_says_add_it_in_studio() {
    let e = diffusion_import_refusal(Some("flux"), &[]).unwrap();
    assert_eq!(e.message, "This is a picture or video model."); assert_eq!(e.next_step, "Add it in Studio → Get local models.");
    assert!(diffusion_import_refusal(None, &table("klein4b-tensors.txt")).is_some());     // leejet's files have no architecture key
    assert!(diffusion_import_refusal(Some("qwen3"), &table("qwen3-4b-tensors.txt")).is_none());
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p models --lib gen_search && cargo test -p models --lib hf && cargo test -p models --lib gguf && cargo test -p models --lib arch && cargo test -p app --lib hub::tests::importing_a_picture_model`
Expected: FAIL to compile, `could not find 'gen_search'`.

- [ ] **Step 3: Implement.** `recognise`, in order: `not_for_all_audiences` → `NotOffered(AdultContent)`; a region-limited token → `NotOffered(RegionLimited)`; `uncensored` or `abliterated` in the id, a file name or `info.tags` → `NotOffered(Uncensored)`; `family_of_name` over the id and file names (none → `NotSupportedYet("harnessUI does not know this kind of model yet.")`; `UncheckedWan22` → `NotSupportedYet("harnessUI runs FastVideo's fast version of Wan 2.2 5B; this one is the full model or a version it has not checked.")`; several → `NotSupportedYet("This repository holds several kinds of model.")`); when the listing states `gguf.parameters`, it must lie in the record's `parameters` band (else `NotSupportedYet("This file is a different size of model than its name says.")`), and a stated `gguf.architecture` must pass `is_diffusion` (else "harnessUI does not know this kind of model yet."); the diffusion files are the repository's `.gguf` files whose names do not mark them as an encoder, decoder or projector (`vae`, `text_encoder`, `umt5`, `qwen3`, `mmproj`) and that carry a SHA-256 (none → `NotSupportedYet("No file here has a published checksum.")`). A name cannot prove a file's family, so Task L11 reads the downloaded file's tensor table before it is added and Task L14 before every run. `family_from_tensors` matches the `SIGNATURES` (width from the recorded tensor and dimension index, blocks by counting distinct `<n>` in the block prefix). `searched_licence`: the repository's label missing → the record's upstream licence; the same id → it; a known stricter id (non-commercial ones such as `cc-by-nc-4.0`, `flux-1-dev-non-commercial-license`) → that licence; a known looser id (`mit`) → the upstream licence with the mismatch sentence; `other` or an unknown id → a licence with `commercial_outputs: Unclear` (red) and the plain words "Check the licence before you use what you make.", with the mismatch sentence. `search_generators` sends the three queries and merges them. `is_diffusion` is true for `flux`, `sd1`, `sdxl`, `sd3`, `wan`, `ltxv`, `hunyuan-video`, `qwen_image`, `lumina2`, `chroma`. `hub::diffusion_import_refusal(arch: Option<&str>, shapes: &[TensorShape]) -> Option<AppError>` (code `picture_model`) runs in `import_model` before `register_installed`, on the file's own tensor table.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p models && cargo test -p app --lib hub`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/models/src catalog/models.json crates/models/tests/fixtures src-tauri/src/hub.rs
git commit -m "Studio: the free recommended list, the known-good family table, tensor signatures, and Hugging Face search that says 'Not supported yet'"
```

### Task L8: Will it fit here, and how long will it take

**Files:**
- Create: `crates/models/src/gen_fit.rs`, `crates/models/src/gen_eta.rs`
- Modify: `crates/models/src/fit.rs` (`Target::room` becomes `pub fn room`), `crates/models/src/lib.rs`
- Test: inline in both

**Interfaces:**
- Consumes: Task L6 `GenEntry`; `fit::{Target, Room, SPARE_BYTES, RAM_RESERVE_BYTES}`.
- Produces:
```rust
// gen_fit.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)] #[serde(rename_all = "camelCase")]
pub struct GenSize { pub width: u32, pub height: u32, pub frames: u32 }                 // frames 1 for a picture
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)] #[serde(rename_all = "camelCase")]
pub enum GenFit { Green, OnePartAtATime, PartlyOnProcessor, Red }
pub enum NotFor { GraphicsMemory { needs: u64 }, MacMemory { needs: u64 }, VideoNeedsGpu, VideoNotOnThisMac }
pub struct GenFitResult { pub fit: GenFit, pub after_unload: Option<GenFit>, pub needed_bytes: u64, pub available_bytes: u64,
    pub memory_saving: bool /* for `fit` */, pub processor_only: bool, pub not_for: Option<NotFor>, pub suggestion: Option<GenSize> }
pub fn needs_memory_saving(fit: GenFit) -> bool;                // one part at a time, or partly on the processor
pub const PICTURE_SIDES: [u32; 3] = [512, 768, 1024];
pub const VIDEO_FRAMES: [u32; 5] = [17, 33, 49, 81, 121];
pub fn tokens(entry: &GenEntry, size: GenSize) -> u64;
pub fn act_vae(entry: &GenEntry, size: GenSize, memory_saving: bool) -> u64;
/// `target` as probed now (with the chat model loaded, if one is); `chat_device_bytes`: what unloading it would give back.
pub fn gen_fit(entry: &GenEntry, size: GenSize, target: &Target, chat_device_bytes: Option<u64>) -> GenFitResult;
// gen_eta.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)] #[serde(rename_all = "camelCase")]
pub struct Eta { pub low_ms: u64, pub high_ms: u64, pub measured: bool }
pub struct DeviceClass { pub backend: String, pub memory_gb: u32 }                         // "vulkan" 8, "metal" 16, "cpu" 0
pub fn device_class(target: &Target, backend: &str) -> DeviceClass;
pub struct MeasuredRun { pub size: GenSize, pub steps: u32, pub pictures: u32, pub elapsed_ms: u64 }
pub const APPLE_VIDEO_PAD: f64 = 1.3;
pub const SLOW_PICTURE_MS: u64 = 120_000;   // "Slow here" for a picture above 2 min
pub const SLOW_CLIP_MS: u64 = 180_000;      // and for the default clip above 3 min
pub const VERY_SLOW_MS: u64 = 900_000;      // 15 min: the amber line, and Studio offers cloud
pub fn estimate(entry: &GenEntry, size: GenSize, steps: u32, pictures: u32, class: &DeviceClass, measured: &[MeasuredRun]) -> Option<Eta>;
pub fn is_slow(entry: &GenEntry, eta: &Eta) -> bool;
pub fn left_ms(step: u32, steps: u32, pictures_left: u32, sec_per_step: f64, tail_ms: u64) -> u64;
pub fn round_for_display(ms: u64) -> u64;   // to 5 s under a minute, 30 s under 10 minutes, whole minutes above
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)] #[serde(rename_all = "camelCase")]
pub enum FitWord { Fits, Slow, AfterUnload, NotHere }        // "fits" | "slow" | "afterUnload" | "notHere": Tasks L15, L16 and the webview
/// The fit a run will have, and whether the chat model must be unloaded for it: `fit` when it is Green or one part at a time (it runs
/// beside the chat model); else `after_unload` when that is not Red; else `fit`.
pub fn running_fit(r: &GenFitResult) -> (GenFit, bool);
/// In this order, so a job that fits once the chat model is unloaded is never called red or slow because of the chat model:
/// 1. `fit` Green or one part at a time → Fits, or Slow for a picture on the processor or when `is_slow(eta)`;
/// 2. `after_unload` Green or one part at a time → AfterUnload;
/// 3. partly on the processor, now or after an unload → Slow;
/// 4. otherwise NotHere.
pub fn fit_word(entry: &GenEntry, r: &GenFitResult, eta: Option<&Eta>) -> FitWord;
pub fn very_slow(word: FitWord, eta: Option<&Eta>) -> bool;      // NotHere, or the range's midpoint above VERY_SLOW_MS
pub fn over_15_min(word: FitWord, eta: Option<&Eta>) -> bool;    // the midpoint above VERY_SLOW_MS; never for NotHere
```
The formulas (addendum L5): D = diffusion part(s) used one after the other, T = text encoders, V = decoder, from part sizes; tokens = ⌈W/d⌉·⌈H/d⌉ with d = `fallback.token_div`, times latent frames (F−1)/`frame_div`+1 for video; act_dit = `act_base_bytes + act_bytes_per_token·tokens`; act_vae = `vae_bytes_per_pixel_frame·W·H·C` with C = `vae_chunk_frames` (at most F; F when unset: Wan decodes one latent frame, 4 frames, at a time), capped at `vae_tile_cap_bytes` with memory saving. **Measured `memory[]` points win**, interpolated linearly in tokens between the two nearest points of the same build (the nearest one alone beyond them). Green: D+T+V+max(act_dit, act_vae)+SPARE ≤ `room.whole`. One part at a time: max(T, D+act_dit, V+act_vae)+SPARE ≤ whole and the rest ≤ the computer's memory less the reserve (sets `memory_saving`). Partly on the processor: D+act_dit ≤ `room.partial`. Red otherwise. `Target::Cpu`: `processor_only`, judged against the computer's memory; a video entry there is Red with `VideoNeedsGpu`; on `AppleUnified` a video entry with `runs_on_apple == false` is Red with `VideoNotOnThisMac`. `after_unload` is computed only when `fit != Green` and `chat_device_bytes` is set, with that many bytes added to `whole` and `partial`; `not_for` (and its `needs`, the one-part-at-a-time need) is judged on that room too, so the reason never blames the chat model's share. The suggestion is the largest preset by pixel-frames (sides at the requested shape, and frames for video, none above the requested size) that is Green or one part at a time, in the room after an unload when a chat model is loaded, so an 8 GB card gets one too. ETA: from the nearest speed point of the same backend (closest `memory_gb`): load + steps·s_per_step·(tokens/tokens_point)^1.3 + decode·(pixel-frames ratio), per picture after the load, as the range [0.75×, 1.5×]; with measured runs of this model on this device, their median scaled the same way, as [0.9×, 1.15×] and `measured: true`; Apple-silicon video ×`APPLE_VIDEO_PAD` (every Apple Mac, because a fanless one cannot be told apart reliably); no point of the same backend and no measured run → `None`.

- [ ] **Step 1: Write the failing tests**
```rust
// gen_fit.rs. Test entries are built in the test module with stated part sizes (GB = 1e9) and the bundled families' fallbacks (Task L7),
// so each expectation follows from the formulas: klein() D 2.46, T 2.50, V 0.34; zimage() D 3.86, T 2.50, V 0.34; wan22() D 4.21, T 4.15,
// V 1.41. gpu(free_gb, ram_gb), apple(ws_gb, ram_gb), cpu(ram_gb). At 1024²: tokens 4096, act_dit 0.91 GB, act_vae 0.31 GB.
#[test] fn klein_at_1024_is_green_on_an_8_gb_card() {          // 5.30 + 0.91 + 1 spare = 7.21 <= 8
    assert_eq!(gen_fit(&klein(), sq(1024), &gpu(8, 32), None).fit, GenFit::Green);
}
#[test] fn on_a_6_gb_card_it_runs_one_part_at_a_time_with_memory_saving() {   // green 8.61 > 6; one part 3.86 + 0.91 + 1 = 5.77 <= 6
    let r = gen_fit(&zimage(), sq(1024), &gpu(6, 32), None); assert_eq!((r.fit, r.memory_saving), (GenFit::OnePartAtATime, true));
}
#[test] fn too_little_everywhere_is_red_with_a_reason() {      // partial room 2 + (4 − 2) = 4 < D + act_dit 4.77
    let r = gen_fit(&zimage(), sq(1024), &gpu(2, 4), None);
    assert_eq!(r.fit, GenFit::Red); assert!(matches!(r.not_for, Some(NotFor::GraphicsMemory { needs }) if needs >= 5_770_000_000));
}
#[test] fn the_chat_models_memory_counts_after_an_unload() {   // 3 GB free: partly on the processor; 8 GB after the unload: one part at a time
    let r = gen_fit(&zimage(), sq(1024), &gpu(3, 32), Some(5 * GB));
    assert_eq!((r.fit, r.after_unload), (GenFit::PartlyOnProcessor, Some(GenFit::OnePartAtATime)));
}
#[test] fn a_video_decoder_works_one_chunk_at_a_time() {      // 400 B × 832 × 480 × 4 frames, not × 49
    assert_eq!(act_vae(&wan22(), clip(832, 480, 49), false), 400 * 832 * 480 * 4);
}
#[test] fn video_on_the_processor_is_not_offered_and_pictures_say_processor() {
    assert!(matches!(gen_fit(&wan22(), clip(832, 480, 49), &cpu(32), None).not_for, Some(NotFor::VideoNeedsGpu)));
    assert!(gen_fit(&klein(), sq(512), &cpu(32), None).processor_only);
}
#[test] fn a_clip_too_big_suggests_the_largest_preset_that_fits() {
    // 1280×704×121 needs 13.0 GB one part at a time; 832×480×49 needs 7.48 (one part, fits 8), 832×480×81 8.26 (no),
    // 1280×704×17 7.31 (fits, but fewer pixel-frames than 832×480×49)
    let r = gen_fit(&wan22(), clip(1280, 704, 121), &gpu(8, 16), None);
    assert_eq!((r.fit, r.suggestion), (GenFit::PartlyOnProcessor, Some(GenSize { width: 832, height: 480, frames: 49 })));
}
#[test] fn measured_points_win_over_the_fallback() { /* klein with memory points 768² 4.1 GB and 1024² 5.0 GB → needed at 1024² == 5.0 GB; at 896² between */ }
#[test] fn wan_2_2_ti2v_counts_tokens_in_32_pixel_patches() { assert_eq!(tokens(&wan22(), clip(832, 480, 49)), 26 * 15 * 13); }
// gen_eta.rs
#[test] fn no_measured_point_and_no_run_here_means_no_time() { assert_eq!(estimate(&klein(), sq(1024), 4, 1, &class("vulkan", 8), &[]), None); }
#[test] fn the_nearest_class_of_the_same_backend_is_used() {
    let e = with_speed(klein(), "vulkan-8", 6.0, 2.0, 1.0);             // load 6 s, 2 s a step, decode 1 s, at 1024²
    assert_eq!(estimate(&e, sq(1024), 4, 1, &class("vulkan", 12), &[]).unwrap(), Eta { low_ms: 11_250, high_ms: 22_500, measured: false });
    assert_eq!(estimate(&e, sq(1024), 4, 1, &class("metal", 16), &[]), None);
}
#[test] fn this_computers_own_runs_replace_the_guess() { /* three runs of 30 s at 1024², 4 steps → measured true, low 27_000, high 34_500 */ }
#[test] fn bigger_settings_cost_time_by_tokens_to_the_power_1_3() { /* 2048×1024 vs 1024² from one measured run: step part × 2^1.3 */ }
#[test] fn apple_video_gets_the_pad() { /* metal class, video entry: high_ms == 1.3 × the unpadded high_ms */ }
#[test] fn times_are_rounded_for_people() {
    assert_eq!(round_for_display(12_300), 10_000); assert_eq!(round_for_display(430_000), 420_000); assert_eq!(round_for_display(820_000), 840_000);
}
#[test] fn time_left_follows_the_live_speed() { assert_eq!(left_ms(3, 4, 1, 2.0, 1_000), 3_000); }
#[test] fn the_word_after_an_unload_comes_before_red_and_slow() {
    let r = gen_fit(&zimage(), sq(1024), &gpu(3, 32), Some(5 * GB));          // partly on the processor now, one part at a time after
    assert_eq!(fit_word(&zimage(), &r, None), FitWord::AfterUnload);
    assert_eq!(running_fit(&r), (GenFit::OnePartAtATime, true));
    let beside = gen_fit(&klein(), sq(1024), &gpu(8, 32), Some(1 * GB));     // green beside a small chat model: no unload
    assert_eq!((fit_word(&klein(), &beside, None), running_fit(&beside)), (FitWord::Fits, (GenFit::Green, false)));
    let red = gen_fit(&zimage(), sq(1024), &gpu(1, 3), Some(1 * GB));        // red even after the unload
    assert_eq!((fit_word(&zimage(), &red, None), running_fit(&red).1), (FitWord::NotHere, false));
    assert_eq!(fit_word(&klein(), &gen_fit(&klein(), sq(512), &cpu(32), None), None), FitWord::Slow);   // a picture on the processor
}
#[test] fn very_slow_starts_after_15_minutes() {
    let twenty = Eta { low_ms: 900_000, high_ms: 1_500_000, measured: true };
    assert!(very_slow(FitWord::Fits, Some(&twenty)) && over_15_min(FitWord::Fits, Some(&twenty)));
    assert!(very_slow(FitWord::NotHere, None) && !over_15_min(FitWord::NotHere, Some(&twenty)));
    assert!(!very_slow(FitWord::Slow, None));
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p models --lib gen_fit && cargo test -p models --lib gen_eta`
Expected: FAIL to compile, `could not find 'gen_fit'`.

- [ ] **Step 3: Implement** the formulas above. `is_slow` compares the range's midpoint with `SLOW_PICTURE_MS` for picture entries and `SLOW_CLIP_MS` for video entries; `round_for_display` rounds to the nearest step; `fit_word`, `running_fit`, `very_slow` and `over_15_min` are the only place these rules live (Tasks L15 and L16 call them).

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p models`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/models/src
git commit -m "Studio: fit verdicts and words for picture and video models, and honest time estimates measured on this computer"
```

### Task L9: Installed picture and video models, and licence acceptances (store)

**Files:**
- Create: `crates/store/src/gen_models.rs`
- Modify: `crates/store/src/creations.rs` (`local_run_times`), `crates/store/src/lib.rs`
- Test: inline in `gen_models.rs`

**Interfaces:**
- Consumes: Task A8's V3 tables.
- Produces:
```rust
sql_enum!(
    /// Where an installed picture or video model came from.
    GenSource, "gen model source" {
        /// The signed catalog's recommended list.
        Catalog => "catalog",
        /// A Hugging Face search.
        Search => "search",
    }
);
pub struct NewGenModel { pub id: String, pub catalog_id: Option<String>, pub name: String, pub kind: String, pub family: String,
    pub parts: Value, pub size_bytes: u64, pub source: GenSource }          // parts: [{role, repo, revision, path, file, size, sha256, license}]
pub struct GenModel { /* NewGenModel's fields */ pub added_at: i64 }
pub struct LicenseAcceptance { pub sha256: String, pub license_id: String, pub catalog_id: Option<String>, pub accepted_at: i64 }
pub struct LocalRunTime { pub params: Value, pub elapsed_ms: u64, pub created_at: i64 }
impl Store {
    pub async fn insert_gen_model(&self, m: NewGenModel) -> Result<(), StoreError>;          // replaces a row with the same id
    pub async fn list_gen_models(&self) -> Result<Vec<GenModel>, StoreError>;               // newest first
    pub async fn get_gen_model(&self, id: &str) -> Result<Option<GenModel>, StoreError>;
    pub async fn delete_gen_model(&self, id: &str) -> Result<Option<GenModel>, StoreError>;
    /// How many installed models use the file at `path`: chat models (`models.path`) and every part of every picture or video model.
    pub async fn file_uses(&self, path: &str) -> Result<u32, StoreError>;
    /// Every part file of every installed picture or video model (deleting a chat model keeps these, Task L11).
    pub async fn gen_model_files(&self) -> Result<BTreeSet<String>, StoreError>;
    pub async fn record_license_acceptance(&self, a: LicenseAcceptance) -> Result<(), StoreError>;
    pub async fn accepted_license_hashes(&self) -> Result<BTreeSet<String>, StoreError>;
    /// This model's finished local runs on `device`, newest first, at most `limit` (Task L8's measured times).
    pub async fn local_run_times(&self, gen_model_id: &str, device: &str, limit: u32) -> Result<Vec<LocalRunTime>, StoreError>;
}
```

- [ ] **Step 1: Write the failing tests**
```rust
#[tokio::test] async fn an_installed_model_reads_back_with_its_parts() { /* insert, get, list (newest first), delete returns it */ }
#[tokio::test] async fn file_uses_counts_both_tables() {
    // two gen models share the encoder at /m/unsloth/Qwen3-4B-GGUF/q.gguf; a chat model row has the same path
    assert_eq!(store.file_uses("/m/unsloth/Qwen3-4B-GGUF/q.gguf").await.unwrap(), 3);
    store.delete_gen_model("a").await.unwrap();
    assert_eq!(store.file_uses("/m/unsloth/Qwen3-4B-GGUF/q.gguf").await.unwrap(), 2);
    assert!(store.gen_model_files().await.unwrap().contains("/m/unsloth/Qwen3-4B-GGUF/q.gguf"));
}
#[tokio::test] async fn acceptances_are_kept_by_licence_hash() { /* record two; accepted_license_hashes == both; recording again is no error */ }
#[tokio::test] async fn measured_times_are_this_models_finished_local_runs_on_this_device() {
    // runs: local done (model a, vulkan0, 30 000 ms), local done (a, cpu), local failed (a, vulkan0), cloud done, local done (b, vulkan0)
    assert_eq!(store.local_run_times("a", "vulkan0", 10).await.unwrap().iter().map(|r| r.elapsed_ms).collect::<Vec<_>>(), [30_000]);
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p store gen_models`
Expected: FAIL to compile, `no method named 'insert_gen_model'`.

- [ ] **Step 3: Implement** with the existing helpers (`sql_enum!`, `json_text`, `now_ms`). `file_uses` counts `models.path = ?1` plus, over `gen_models`, the parts whose `file` equals `?1` (`json_each(parts_json)`).

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p store`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/store/src
git commit -m "Studio: installed picture and video models, shared files counted, licence acceptances kept"
```

### Task L10: The Studio engine's commands

**Files:**
- Create: `src-tauri/src/studio/engine_cmd.rs`
- Modify: `src-tauri/src/studio/mod.rs`, `src-tauri/src/lib.rs` (`manage(EngineDownloads::default())`, handlers), `src-tauri/commands.list`, `src-tauri/capabilities/main.json`, `src-tauri/src/studio/commands.rs` (`STUDIO_COMMANDS`), `src-tauri/src/dto.rs`
- Test: inline in `engine_cmd.rs`

**Interfaces:**
- Consumes: Tasks L1–L2; `runtime_manager::layout` (the installed llama folder), the pattern of `packs_cmd.rs`.
- Produces:
```text
studio_engine_status() -> EngineStatusDto
studio_engine_download(accept_license: bool) -> ()         // Windows; refused unless accept_license
studio_engine_cancel() -> ()
studio_engine_remove() -> ()
event "studio-engine-progress" → EngineProgressDto (every 300 ms at most)
```
```rust
pub struct EngineStatusDto { pub state: String /* builtIn | notInstalled | partial | downloading | installed | unavailable */,
    pub build: String, pub download_bytes: u64, pub license_name: String, pub license_url: String, pub bytes_done: Option<u64> }
pub struct EngineProgressDto { pub done: u64, pub total: u64, pub unpacking: bool, pub finished: bool, pub error: Option<AppError> }
#[derive(Default)] pub struct EngineDownloads(std::sync::Mutex<Option<CancellationToken>>);
pub fn engine_layout(app: &AppHandle) -> EngineLayout;      // bundled: macOS exe folder; engines: <local data>/engines; llama: layout(app).installed_dir; APP_DEV_SD_DIR
pub fn current_engine(app: &AppHandle) -> Option<Engine>;
/// Starts the pack download when this platform has a pack and it is not installed; returns at once.
pub async fn start_engine_download(app: &AppHandle) -> AppResult<()>;
pub fn status_from(lock: &SdLock, layout: &EngineLayout, platform: &str, downloading: Option<u64>) -> EngineStatusDto;
```

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn a_mac_has_the_engine_built_in() { /* platform "macos-aarch64", bundled dir holding sd-cli → state "builtIn", download_bytes 0 */ }
#[test] fn windows_without_the_pack_offers_its_29_mb_download() {
    let s = status_from(embedded(), &empty_layout(), "windows-x86_64", None);
    assert_eq!(s.state, "notInstalled"); assert_eq!(s.download_bytes, embedded().packs[0].download_size());
    assert_eq!(s.build, "master-1106-532d2c1");
}
#[test] fn linux_without_a_dev_engine_says_unavailable() { assert_eq!(status_from(embedded(), &empty_layout(), "linux-x86_64", None).state, "unavailable"); }
#[tokio::test] async fn the_download_needs_the_licence_accepted() { /* download(false) → bad_request "Accept the licence to download the Studio engine." */ }
// commands.rs: the four names join STUDIO_COMMANDS; Task A16's test finds them registered as `studio::engine_cmd::<name>,`
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::engine_cmd`
Expected: FAIL to compile.

- [ ] **Step 3: Implement** on `packs_cmd.rs`'s pattern (one download at a time; progress throttled to 300 ms; errors through the pack error copy with "Studio engine" in place of the backend's name). Each command checks `only_main_window`.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio && cargo test -p app --lib commands::tests::commands_list_matches_capabilities`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio src-tauri/src/lib.rs src-tauri/src/dto.rs src-tauri/commands.list src-tauri/capabilities/main.json
git commit -m "Studio engine: status, download, cancel and remove"
```

### Task L11: Downloading a local model: several parts, resume, SHA-256, licences and the engine

**Files:**
- Create: `src-tauri/src/studio/local_models.rs`
- Modify: `src-tauri/src/downloads.rs` (`GroupMeta.generator`; `run_group` takes each row's commit from `row_commit`; `register_gen_model` before the "finished download has no model file" error; `DownloadGroupDto.generator_kind`), `src-tauri/src/hub.rs` (`detect` and `current_catalog` become `pub(crate)`; `delete_model` and `delete_preview` also keep files that installed picture and video models use), `src-tauri/src/studio/mod.rs`
- Test: inline in `local_models.rs`, `downloads.rs` and `hub.rs`

**Interfaces:**
- Consumes: Tasks L6, L7, L9, L10; `models::download::{FileSpec, destination}`, `reverse::diskspace::free_bytes`, `hub::{hf_client, models_dir}`, `DownloadManager::start`.
- Produces:
```rust
// downloads.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct GenGroup { pub gen_model_id: String, pub catalog_id: Option<String>, pub name: String, pub kind: String, pub family: String,
    pub source: String, pub parts: Vec<GenGroupPart> }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct GenGroupPart { pub role: String, pub repo: String, pub revision: String, pub path: String, pub size: u64, pub sha256: String, pub license: String }
// on GroupMeta: #[serde(default, skip_serializing_if = "Option::is_none")] pub generator: Option<GenGroup>
// on DownloadGroupDto: pub generator_kind: Option<String>      // "image" | "image-edit" | "video" for Downloads' label
/// The `downloads` table has no commit column: a generator part downloads at its own revision, any other row at the group's.
pub(crate) fn row_commit(meta: &GroupMeta, repo: &str, file: &str) -> String;
/// Every part completed with its pinned SHA-256 → the row to insert; anything missing → Err (the group says so; Resume finishes it).
pub(crate) fn gen_model_from(meta: &GroupMeta, rows: &[store::DownloadRow]) -> AppResult<store::NewGenModel>;
/// The downloaded diffusion part's tensor table must be its family's (`gen_search::family_from_tensors`); `.safetensors` parts were
/// checked by size and SHA-256 only. Otherwise `studio_not_that_model` and nothing is registered.
pub(crate) fn check_diffusion_file(family: GenFamily, path: &Path) -> AppResult<()>;
// hub.rs: what Delete with files may remove from a chat model: never a file another chat model or any picture or video model uses
pub(crate) fn used_elsewhere(others: &[store::ModelRecord], gen_files: &BTreeSet<String>, file: &Path) -> bool;
// local_models.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] #[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LocalSource { Catalog { entry_id: String }, Search { repo: String, commit: String, file: String },
    Installed { gen_model_id: String } }          // {"catalog":{"entryId":…}}; Installed: its saved parts, no network (fit; Download again)
pub struct DownloadPlan { pub entry: GenEntry, pub files: Vec<(FileSpec, PathBuf)>, pub already_here: Vec<PathBuf>, pub total_bytes: u64,
    pub to_fetch_bytes: u64, pub licences: Vec<GenLicence>, pub needs_engine: bool, pub free_bytes: Option<u64> }
impl DownloadPlan { pub fn low_disk(&self) -> bool; }       // less than 10 GB would be left
pub const LOW_DISK_BYTES: u64 = 10_000_000_000;
pub fn plan_download(entry: GenEntry, models_dir: &Path, engine_installed: bool, free_bytes: Option<u64>) -> AppResult<DownloadPlan>;
pub fn check_acceptance(plan: &DownloadPlan, accepted: &BTreeSet<String>, notice_accepted: bool) -> AppResult<()>;
/// The entry a source names, from the current catalog, for a search source from the repository's listing at that commit,
/// recognised again here (the webview's word is never taken for it), or for an installed model from its saved parts and family record.
pub async fn entry_from_source(app: &AppHandle, source: &LocalSource) -> AppResult<GenEntry>;
pub fn entry_from_listing(detail: &models::hf::RepoDetail, file: &str, families: &BTreeMap<GenFamily, FamilyRecord>) -> AppResult<GenEntry>;
pub fn entry_from_installed(model: &store::GenModel, catalog: &GenCatalog) -> AppResult<GenEntry>;   // a catalog entry, or saved parts + family record
pub async fn start_local_download(app: &AppHandle, source: LocalSource, accepted_license_sha256s: Vec<String>) -> AppResult<String>;  // group id
pub async fn delete_local_model(app: &AppHandle, id: &str) -> AppResult<()>;
/// Deletes the row `id`, then each part file no installed model (chat, picture or video) uses any more and no unfinished download
/// writes (`busy`), then empty folders up to `models_dir`; returns the files removed. `delete_local_model` calls it.
pub(crate) async fn delete_files_of(store: &store::Store, models_dir: &Path, id: &str, busy: &dyn Fn(&Path) -> bool) -> AppResult<Vec<PathBuf>>;
```
Errors (in `studio/errors.rs`): `studio_not_that_model` ("This file is not the model its name says, so it was not added." / "Choose another file, or a model from the recommended list."), `studio_notice_needed` ("Read the note about local models first."), `studio_license_not_accepted` ("Accept the licences to download this model."), `studio_disk_too_small` ("There is not enough free space for this model." / "It needs <size>; free up space or choose another models folder."), `studio_not_supported_yet` ("harnessUI cannot run this model yet."), `studio_not_offered` ("harnessUI does not offer this model." with the reason).

- [ ] **Step 1: Write the failing tests**
```rust
// downloads.rs
#[test] fn generator_parts_download_at_their_own_revisions() {
    let meta = GroupMeta { commit: "c".repeat(40), generator: Some(group_with(&[("unsloth/Qwen3-4B-GGUF", "q.gguf", "a".repeat(40))])), ..meta() };
    assert_eq!(row_commit(&meta, "unsloth/Qwen3-4B-GGUF", "q.gguf"), "a".repeat(40));
    assert_eq!(row_commit(&GroupMeta { generator: None, ..meta.clone() }, "unsloth/Qwen3-4B-GGUF", "q.gguf"), "c".repeat(40));
}
#[test] fn a_generator_group_registers_only_when_every_part_is_verified() {
    // three parts; rows: two completed with their sha256, one still downloading → Err, message "A part of this model is not downloaded yet."
    // all three completed → NewGenModel { id, size_bytes: sum, parts: 3 entries with their dest paths as `file`, source: Catalog }
}
#[test] fn groups_saved_before_this_version_still_read() { /* a GroupMeta JSON without "generator" → generator None */ }
#[test] fn a_searched_file_that_is_another_model_is_not_added() {
    // the diffusion part written with models::gguf::testing::diffusion_gguf("klein9b-tensors.txt") for a Flux2Klein4b group
    assert_eq!(check_diffusion_file(GenFamily::Flux2Klein4b, &path).unwrap_err().code, "studio_not_that_model");
    // the same file from "klein4b-tensors.txt" → Ok; register_gen_model with the 9B file → no gen_models row, its files removed (none shared)
}
// hub.rs
#[test] fn deleting_a_chat_model_keeps_a_file_a_picture_model_uses() {
    // a chat model at /m/unsloth/Qwen3-4B-GGUF/Qwen3-4B-Q4_K_M.gguf; gen_files holds that path (FLUX.2 klein's encoder)
    assert!(used_elsewhere(&[], &gen_files, Path::new("/m/unsloth/Qwen3-4B-GGUF/Qwen3-4B-Q4_K_M.gguf")));
    assert!(removal_plan_with(&chat, models_dir, |f| used_elsewhere(&[], &gen_files, f)).delete.is_empty());
}
// local_models.rs
#[test] fn a_part_already_on_disk_is_checked_not_fetched() {
    // the encoder file at its destination with its pinned size → plan.already_here holds it; to_fetch_bytes excludes it;
    // plan.files still lists it (download_file_checked verifies it without fetching)
}
#[test] fn downloads_are_refused_until_the_notice_and_every_licence_are_accepted() {
    assert_eq!(check_acceptance(&plan, &set(&[]), false).unwrap_err().code, "studio_notice_needed");
    assert_eq!(check_acceptance(&plan, &set(&[APACHE_SHA]), true).unwrap_err().code, "studio_license_not_accepted");   // one licence of two
    assert!(check_acceptance(&plan, &set(&[APACHE_SHA, KOMMUNITY_SHA]), true).is_ok());
}
#[test] fn a_disk_too_small_is_refused_before_anything_starts() {
    assert_eq!(plan_download(entry(), dir, true, Some(1_000)).unwrap_err().code, "studio_disk_too_small");
    assert!(plan_download(entry(), dir, true, Some(entry().total_bytes() + 5_000_000_000)).unwrap().low_disk());
}
#[test] fn the_engine_comes_with_the_first_model_where_it_is_a_pack() { /* engine_installed false → needs_engine true */ }
#[test] fn an_unsupported_search_source_is_refused_by_the_backend() {
    let d = detail("repo-unknown-diffusion.json");
    assert_eq!(entry_from_listing(&d, &d.files[0].path, &families()).unwrap_err().code, "studio_not_supported_yet");
    let d = detail("repo-z-image-turbo-gguf.json");
    assert_eq!(entry_from_listing(&d, "not-in-the-listing.gguf", &families()).unwrap_err().code, "not_found");
}
#[tokio::test] async fn deleting_a_model_keeps_a_shared_encoder() {
    // two installed models share q.gguf, and a chat model row uses c.gguf, which "a" also lists:
    // delete_files_of(&store, dir, "a", &|_| false) → removes only a's own diffusion file; deleting "b" then removes q.gguf; c.gguf stays
}
#[test] fn an_installed_model_is_its_own_source() {
    // entry_from_installed(gen_model "hf:someone/Z-Image-Turbo-GGUF:z.gguf", families) → the saved parts at their saved revisions, no request
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::local_models && cargo test -p app --lib downloads`
Expected: FAIL to compile, `cannot find type 'GenGroup'`.

- [ ] **Step 3: Implement.** `plan_download` builds one `FileSpec { repo, commit: revision, path, size, sha256: Some }` and `destination(models_dir, repo, path)` per part, puts a part whose destination already has its pinned size into `already_here`, and refuses when `to_fetch_bytes > free_bytes`. `start_local_download`: `entry_from_source` → `plan_download` (free space of the models folder) → `check_acceptance` with `store.accepted_license_hashes()` plus the hashes sent, and `prefs.local_notice_accepted_at.is_some()` → record each sent hash that matches one of the plan's licences (`record_license_acceptance`; a hash not in the plan is ignored) → `DownloadManager::start(app, GroupMeta { repo: diffusion part's repo, commit: its revision, label: entry name, model_files: [], projector: None, catalog_id, generator: Some(GenGroup …), .. }, plan.files)` → when `needs_engine`, `engine_cmd::start_engine_download(app)`. `register_model` checks `meta.generator` first: `gen_model_from(meta, rows)`, then `check_diffusion_file` on the diffusion part (refused → `delete_files_of`-style removal of the group's files no other model uses, and the group's error says so), then `store.insert_gen_model`; matching is by `dest_path`, because two repositories may hold files of the same name. `delete_local_model` calls `delete_files_of` with `busy` from `downloads::written_by` and the live group list. `hub::delete_model` and `delete_preview` pass `|f| used_elsewhere(&others, &store.gen_model_files(), f)` to `removal_plan_with`, so a chat model's Delete with files keeps FLUX.2 klein's encoder when Klein is installed (the same Qwen3-4B file is a common chat download). An `Installed` source rebuilds the entry from the saved parts (pinned revisions and SHA-256s) and the family record, without a request; `studio_local_download` with it and no new hashes re-checks every part (`download_file_checked` verifies what is on disk and fetches what is missing or does not match), which is *Download again*. A newly pinned revision whose licence hash changed is simply not in `accepted_license_hashes`, so the dialog asks again.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio && cargo test -p app --lib downloads`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio src-tauri/src/downloads.rs src-tauri/src/hub.rs
git commit -m "Studio: download local models part by part, after the notice and every licence, with the engine on first use"
```

### Task L12: Sharing the graphics card with the chat model

**Files:**
- Create: `src-tauri/src/gpu_lease.rs`
- Modify: `src-tauri/src/runtime_manager.rs` (`device_bytes`, `keeps_in_memory`, `StudioGate`, `set_studio_gate`, the gate in `load`), `src-tauri/src/dto.rs` (`RuntimeStatusDto.waiting_for_studio: bool`), `src-tauri/src/lib.rs` (`mod gpu_lease;`, `app.manage(lease.clone())`, `state.runtime.set_studio_gate(lease)`)
- Test: inline in `gpu_lease.rs` (a `FakeChat`), and `runtime_manager.rs`

**Interfaces:**
- Consumes: Task A12 `GpuPolicy`; `RuntimeManager::{before_change, unload_by_user, loaded_on_device, freed_memory, load, backends}`, `state::BusyCounter`.
- Produces:
```rust
// runtime_manager.rs
impl RuntimeManager {
    pub fn device_bytes(&self) -> Option<u64>;                       // loaded_on_device()'s bytes
    pub fn keeps_in_memory(&self) -> bool;                           // the loaded model's resolved keep_in_memory
    pub fn set_studio_gate(&self, gate: Arc<dyn StudioGate>);
}
#[async_trait] pub trait StudioGate: Send + Sync {
    fn holds_card(&self) -> bool;                                    // only from a grant to its hold's drop: never while a job still waits
    async fn wait_until_free(&self);
    fn take_reprobe(&self) -> bool;                                  // true once after a Studio job freed the card
}
// gpu_lease.rs
/// From gen_fit with and without the chat model (Task L15): `alongside` = Green or one part at a time now; `after_unload` = not Red once
/// the chat model is unloaded; `processor_ok` = a picture whose processor-only fit is not Red; `shared_memory` = Apple silicon.
pub struct StudioNeed { pub alongside: bool, pub after_unload: bool, pub processor_ok: bool, pub shared_memory: bool }
pub enum Grant { Alongside, AfterUnload { unloaded: Option<String> }, UseProcessor }
pub enum LeaseWait { ReplyRunning, AnotherStudioJob, NeedsAnswer }
pub enum Refused { Cancelled, DoesNotFit, NeedsAnswer }
#[async_trait] pub trait ChatSide: Send + Sync {
    fn loaded_device_bytes(&self) -> Option<u64>;
    fn keeps_in_memory(&self) -> bool;
    /// before_change(when_idle = true) then unload_by_user(); `on_wait(true)` while a reply is being written. The unloaded path.
    async fn unload_when_idle(&self, on_wait: &(dyn Fn(bool) + Send + Sync)) -> AppResult<Option<String>>;
    async fn reload(&self, path: &str);
}
pub struct RuntimeChat { pub runtime: Arc<RuntimeManager>, pub busy: Arc<BusyCounter> }      // impl ChatSide
pub struct StudioHold { /* dropping it frees the card and wakes a waiting chat load; it never reloads the chat model */ }
pub struct GpuLease { /* chat, a semaphore of one, the granted flag, reprobe flag */ }
impl GpuLease {
    pub fn new(chat: Arc<dyn ChatSide>) -> Arc<Self>;
    /// `answered`: the person's answer to "Unload the chat model?" under GpuPolicy::Ask (None: not asked yet).
    pub async fn acquire_for_studio(self: &Arc<Self>, need: &StudioNeed, policy: GpuPolicy, answered: Option<bool>,
        cancel: &CancellationToken, on_wait: &(dyn Fn(LeaseWait) + Send + Sync)) -> Result<(Grant, StudioHold), Refused>;
    /// "Reload it after Studio": the manager calls this once, when the local queue is empty (Task L15), never per job.
    pub async fn reload_chat(&self, path: &str);
}
#[async_trait] impl StudioGate for GpuLease { /* … */ }
```

- [ ] **Step 1: Write the failing tests**
```rust
#[tokio::test] async fn a_job_that_fits_beside_the_chat_model_runs_alongside() {
    let (g, _hold) = lease(FakeChat::loaded(5 * GB)).acquire_for_studio(&need(true, true, true), GpuPolicy::Unload, None, &tok(), &|_| {}).await.unwrap();
    assert_eq!(g, Grant::Alongside); assert_eq!(chat.unloads(), 0);
}
#[tokio::test] async fn a_job_that_does_not_fit_even_after_an_unload_is_refused() {
    // need(alongside false, after_unload false, processor_ok false) under Unload → Err(Refused::DoesNotFit), unloads() == 0;
    // with processor_ok true → Grant::UseProcessor
}
#[tokio::test] async fn a_job_that_does_not_fit_waits_for_the_reply_then_unloads() {
    // FakeChat with a reply running for 100 ms → on_wait saw ReplyRunning; Grant::AfterUnload { unloaded: Some("/m/chat.gguf") }; unloads() == 1
}
#[tokio::test] async fn ask_without_an_answer_asks_and_the_answer_decides() {
    // Ask, answered None → Err(Refused::NeedsAnswer) and on_wait(NeedsAnswer); Some(true) → AfterUnload; Some(false) with processor_ok → UseProcessor;
    // Some(false) without → Err(DoesNotFit)
}
#[tokio::test] async fn the_processor_policy_never_unloads() { /* Processor, processor_ok → UseProcessor; unloads() == 0 */ }
#[tokio::test] async fn keep_in_memory_always_unloads_on_shared_memory() {
    // shared_memory, keeps_in_memory, alongside true → AfterUnload, also under GpuPolicy::Processor (the processor uses the same memory);
    // on a graphics card (shared_memory false) keep_in_memory pins system memory only → Alongside
}
#[tokio::test] async fn one_studio_job_holds_the_card_at_a_time() {
    // second acquire waits (on_wait AnotherStudioJob) until the first hold drops
}
#[tokio::test] async fn a_chat_load_waits_while_studio_holds_the_card() {
    // holds_card() true while held; wait_until_free resolves only after drop; take_reprobe() true once, then false
}
#[tokio::test] async fn a_chat_load_while_a_job_waits_for_the_card_goes_first() {
    // FakeChat busy (a Model check holds a model use); acquire waits in unload_when_idle → holds_card() false, so the check's load()
    // is not held; when the use ends the job unloads and takes the card
}
#[tokio::test] async fn reload_chat_loads_the_chat_model_once_and_a_hold_never_does() {
    // grant AfterUnload, drop the hold → reloads() == 0; reload_chat("/m/chat.gguf") → reloads() == 1
}
#[tokio::test] async fn cancel_ends_any_wait() { /* reply running forever; cancel → Err(Refused::Cancelled), unloads() == 0 */ }
// runtime_manager.rs (the source-reading pattern of its existing load tests)
#[test] fn the_load_path_asks_the_studio_gate_first() {
    // in `pub async fn load(`, "wait_for_studio(" appears before "self.choose(&settings)" (the backend is chosen, from a re-probe after a
    // Studio job, before the server stops) and so before "self.stop_server().await"
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib gpu_lease && cargo test -p app --lib runtime_manager::tests::the_load_path_asks_the_studio_gate_first`
Expected: FAIL to compile, `file not found for module 'gpu_lease'`.

- [ ] **Step 3: Implement.** Decision order in `acquire_for_studio`: take the one permit (another holder → `on_wait(AnotherStudioJob)` while waiting); no chat model on a device → `Alongside`; `alongside` and not (`shared_memory && keeps_in_memory`) → `Alongside`; `!after_unload` → `UseProcessor` when `processor_ok` (unloading first on shared memory), else `Refused::DoesNotFit`; else by policy: `Unload` → `unload_when_idle` (`on_wait(ReplyRunning)` while it waits) → `AfterUnload`; `Ask` → by `answered` as the test says; `Processor` → `UseProcessor` when `processor_ok` and not (`shared_memory && keeps_in_memory`), else unload. The granted flag (what `holds_card` reads) is set only when the grant is returned, so a chat load during the wait goes first and the unload simply waits for it to end. Every wait selects on `cancel`. `RuntimeManager::load` starts with `wait_for_studio()`: when a gate is set and `holds_card()`, it sets `waiting_for_studio` in the status, awaits `wait_until_free()`, clears it, and when `take_reprobe()` calls `self.backends(true)` before `self.choose(` (Vulkan drivers can free memory late). `BusyKind` is not used: it counts users of the chat model, not graphics-card tenants.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib gpu_lease && cargo test -p app --lib runtime_manager`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/gpu_lease.rs src-tauri/src/runtime_manager.rs src-tauri/src/dto.rs src-tauri/src/lib.rs
git commit -m "Studio: share the graphics card with the chat model, unloading it only when a local job needs the room"
```

### Task L13: The AI label in local files, and the prompt check

**Files:**
- Create: `src-tauri/src/studio/label.rs`, `src-tauri/src/studio/prompt_check.rs`
- Modify: `src-tauri/src/studio/mod.rs`, `src-tauri/src/studio/errors.rs` (`wont_make()`: code `studio_wont_make`, message "Studio won't make this.", next step "Change the prompt.")
- Test: inline in both (`test_support::png`; a WebP and a WebM built in the test module)

**Interfaces:**
- Consumes: Task A9 `GalleryError`, `io_error`.
- Produces:
```rust
pub const IPTC_TRAINED: &str = "http://cv.iptc.org/newscodes/digitalsourcetype/trainedAlgorithmicMedia";
pub struct AiLabel { pub app_version: String, pub model: String, pub revision: String, pub id: String, pub created: String }   // RFC 3339; never a prompt
pub fn xmp(label: &AiLabel) -> String;           // Iptc4xmpExt:DigitalSourceType, dc:description "Made with harnessUI <v>, <model> <revision>", xmpMM:DocumentID, xmp:CreateDate
/// PNG: drops every tEXt, zTXt and iTXt chunk, then one iTXt "XML:com.adobe.xmp" before IDAT. WebP: an XMP chunk, VP8X added or flagged.
/// WebM: drops the Segment's Tags, appends one Tags element (TITLE, COMMENT, DATE_RECORDED, a DigitalSourceType tag) and rewrites the Segment size.
pub fn label_file(path: &Path, mime: &str, label: &AiLabel) -> Result<(), GalleryError>;
pub fn read_label(path: &Path, mime: &str) -> Option<String>;            // the XMP or the tags, as text
pub fn refuses(prompt: &str) -> bool;                                    // prompt_check
```

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn a_png_carries_the_ai_label_and_no_prompt() {
    let f = scratch.write("a.png", &png_with_text_chunk("parameters", "a lighthouse at dusk, steps 4"));   // what sd-cli would embed
    label_file(&f, "image/png", &label()).unwrap();
    let text = read_label(&f, "image/png").unwrap();
    assert!(text.contains(IPTC_TRAINED) && text.contains("Made with harnessUI 0.1.0, FLUX.2 klein 4B"));
    assert!(!String::from_utf8_lossy(&std::fs::read(&f).unwrap()).contains("lighthouse"));
    assert!(image::load_from_memory(&std::fs::read(&f).unwrap()).is_ok());           // still a valid picture
}
#[test] fn a_webp_carries_the_label() { /* simple (VP8) and extended (VP8X) WebP → read_label has IPTC_TRAINED; image decodes */ }
#[test] fn a_webm_carries_the_label_and_its_old_tags_go() { /* a WebM with a Tags element holding "prompt" → after: label tags only; EBML walk is consistent */ }
#[test] fn the_check_refuses_sexual_words_paired_with_words_for_children() {
    // a table of at least 20 prompts pairing a SEXUAL word with a MINOR word (lists in prompt_check.rs), in mixed case,
    // with punctuation and spacing tricks → all refused; and at least 20 that must pass ("a child's birthday party",
    // "a sexy sports car", "school portrait of a class") → none refused
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::label && cargo test -p app --lib studio::prompt_check`
Expected: FAIL to compile.

- [ ] **Step 3: Implement** without a new dependency: PNG chunks with their CRC-32 (a small table in `label.rs`, so `src-tauri` gains no direct dependency), RIFF sizes for WebP, and a minimal EBML reader and writer for WebM (element ids and variable-length sizes; the Segment's size field is rewritten in its existing width). The prompt check lower-cases, folds look-alike characters and punctuation to spaces, and refuses when a word from `SEXUAL` and a word from `MINOR` both occur as whole words.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio::label && cargo test -p app --lib studio::prompt_check`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio
git commit -m "Studio: label every local file as AI-made without the prompt, and a small always-on prompt check"
```

### Task L14: `LocalMediaProvider` behind `MediaProvider`

**Files:**
- Create: `src-tauri/src/studio/local.rs`
- Modify: `src-tauri/src/studio/mod.rs`
- Test: inline in `local.rs`; the fake engine is this test binary: the test `fake_sd_process` calls `runtime::sd::testing::fake_main` with the arguments after `--` when `RUNTIME_FAKE_SD` is set (the pattern of `attachments::tests::picture_worker_process`), and `fake_engine(mode)` is `Engine { program: current_exe(), leading_args: ["studio::local::tests::fake_sd_process", "--exact", "--nocapture", "--test-threads=1", "--"], extra_env: [(FAKE_MODE_ENV, mode), (FAKE_RECORD_ENV, dir)], .. }`

**Interfaces:**
- Consumes: Task A1 `MediaProvider` and types, Tasks L4–L9, L13 (`prompt_check` is used by the manager, Task L15).
- Produces:
```rust
#[async_trait] pub trait GenCatalogSource: Send + Sync { async fn gen_catalog(&self) -> GenCatalog; }
    // the app: hub::current_catalog → gen_catalog(&c, runtime::sd::lock::embedded().sd_cpp.build), so a pin bump moves minBuild gating with it
pub enum MemoryChoice { Automatic, Off, On }                    // the `memory_saving` field; the manager resolves Automatic (Task L15)
/// What the lane decided for one run, after the lease's grant and the fit: everything the engine job takes beyond the request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LocalJobOptions { pub processor_only: bool, pub memory_saving: bool, pub threads: Option<u32>,
    pub device_description: Option<String> }                   // llama's probed device, matched to sd-cli's --list-devices for --backend
pub struct LocalRunStatus { pub progress: Option<LocalProgress>, pub stalled: bool, pub retrying: bool, pub outputs: usize, pub failure: Option<SdFailure> }
/// What the manager's local lane needs beyond `MediaProvider` (Task L15 fakes it in its tests).
#[async_trait] pub trait LocalProvider: MediaProvider {
    /// `MediaProvider::start` with the lane's options; `start` itself is `start_local(req, scratch, &LocalJobOptions::default())`.
    async fn start_local(&self, req: &MediaRequest, scratch: &Path, opts: &LocalJobOptions) -> Result<Started, MediaError>;
    /// Lists the engine's devices once per engine when a run with `opts` matches llama's card among them; the manager calls it before
    /// `preview_local`, so a first preview carries the `--backend` the run would (ruling L14-preview-backend). A failed listing is kept
    /// per engine too (no `--backend`, as the run falls back), and it waits 5 s at most: a listing still going then goes on for the run.
    async fn list_devices_for(&self, opts: &LocalJobOptions);
    fn preview_local(&self, req: &MediaRequest, opts: &LocalJobOptions) -> RequestPreview;      // the command this run would use
    /// After the lane moved the outputs (or the run failed): removes `.partial/<run>` (the prompt files with it) and forgets the run.
    async fn release(&self, remote_id: &str);
    fn status(&self, remote_id: &str) -> Option<LocalRunStatus>;
    async fn entry_of(&self, model_id: &str) -> Option<(store::GenModel, GenEntry)>;
    fn engine_build(&self) -> Option<u32>;
}
pub struct LocalMediaProvider { /* store, catalog source, engine (RwLock<Option<Engine>>), llama_dir, devices cache, runs: Mutex<HashMap<String, LocalRun>> */ }
impl LocalMediaProvider {
    pub fn new(store: store::Store, catalog: Arc<dyn GenCatalogSource>, engine: Option<Engine>, llama_dir: Option<PathBuf>) -> Self;
    pub fn set_engine(&self, engine: Option<Engine>);
    pub fn engine(&self) -> Option<Engine>;
    /// Installed models with their catalog entries (a searched model's entry rebuilt from its saved parts and the family record).
    pub async fn installed(&self) -> Vec<(store::GenModel, GenEntry)>;
    pub fn params_for(entry: &GenEntry) -> Vec<ParamSpec>;
    pub fn status(&self, remote_id: &str) -> Option<LocalRunStatus>;
    /// The engine job for a validated request: Shape → width/height on the entry's multiple, How many → batch, Length → frames by the
    /// family's rule, the start frame → init image, references → ref images, the entry's flow shift; `opts` → processor only (`cpu`,
    /// threads), memory saving and `backend` (the device name `devices::match_llama_device` found for `opts.device_description`); quirks → env.
    pub fn job_for(&self, entry: &GenEntry, model: &store::GenModel, req: &MediaRequest, opts: &LocalJobOptions, backend: Option<String>,
        out_dir: &Path, run_id: &str) -> Result<SdJob, MediaError>;
}
impl MediaProvider for LocalMediaProvider {
    // company() → Company::Local; host() → ""; fetch_catalog() → RawCatalog { images: JSON of the installed models as MediaModels
    //   (price Price::Free, params from params_for), videos: None }, never the network; fetch_image_price() → Value::Null;
    // preview() → preview_local(req, &Default::default()): RequestPreview { method: "run", url: "sd-cli", host: "",
    //   body: {"argv": [...], "prompt": "…", "negativePrompt": "…"} } (paths shown, the prompt beside the argv that names its file: "See the exact command");
    // start() → start_local(req, scratch, &Default::default()): checks, spawns run_job in a task, Started::Job { remote_id: "local:<run id>" };
    // check() → Pending | Running{progress: overall} |
    //   Done(JobDone { outputs, signed: [], cost: Some(Cost { usd: 0.0, reported: true }) }) | Failed(MediaFailure { kind, charged: Some(false), .. });
    // download(i) → moves output i out of .partial/<run>/ to `dest` → Fetched::Saved; cancel() → kills the run, waits, removes .partial/<run>
}
impl LocalProvider for LocalMediaProvider { /* start_local, preview_local, release; status from the run's last RunEvents; entry_of from installed();
    engine_build from the engine */ }
```
`params_for` (all `canonical` roles as the cloud ones, so `canonical.ts` carries values between local and cloud models): Simple: `aspect_ratio` (Shape: 1:1, 4:3, 3:4, 16:9, 9:16), `n` (Count, 1–4, pictures only), `resolution` (video: `480p` initial, `720p`), `duration` (video Length: whole seconds from 1 to the entry's `max_frames` at its fps, initial 2), `first_frame` (StartFrame, `Images{0,1}`, when the entry takes a start frame), `input_references` (References, `Images{0,4}`, when it takes references). Advanced: `seed` (`Seed{max: SEED_MAX}`), `steps` (range 1–50, initial the entry's), `cfg_scale` (range 1–20 step 0.5; hidden when the default is 1.0, as 4-step models need), `sampler` and `scheduler` (enums from the whitelists), `size` (Size, multiples of the entry's `multiple_of`), `negative_prompt` (an `Extra`-free text field shown only when the default cfg is above 1), `memory_saving` (enum Automatic | Off | On), `run_on` (enum Graphics card | Processor; pictures only, as local video runs on the graphics card only, and `runs_on_processor` ignores a processor sent with a video: ruling L14-video-processor). No `extra` field. Shapes and sizes use `gen_fit`'s own `SHAPES`, `shape_of` and `at_shape` (with the entry's longest side), so the fit and the job describe one size (ruling L14-shape-helpers).

- [ ] **Step 1: Write the failing tests**
```rust
#[tokio::test] async fn installed_models_become_media_models_with_local_controls() {
    let m = parse_catalog(Company::Local, &provider.fetch_catalog().await.unwrap(), &BTreeMap::new());
    let klein = m.iter().find(|m| m.id == "flux2-klein-4b-q4").unwrap();
    assert_eq!(klein.price, Price::Free);
    assert_eq!(keys(klein, Layer::Simple), ["aspect_ratio", "n", "input_references"]);
    assert!(klein.params.iter().all(|p| !matches!(p.kind, ParamKind::Extra { .. })));
    let wan = m.iter().find(|m| m.kind == MediaKind::Video).unwrap();
    assert_eq!((param(wan, "resolution").initial.clone(), param(wan, "duration").initial.clone()), (Some(json!("480p")), Some(json!(2))));
}
#[tokio::test] async fn the_preview_is_the_exact_command() {
    let p = provider.preview(&req("a lighthouse"));
    assert_eq!((p.host.as_str(), p.body["prompt"].as_str()), ("", Some("a lighthouse")));
    let argv = p.body["argv"].as_array().unwrap();
    assert!(argv.iter().any(|a| a == "--prompt-file") && !argv.iter().any(|a| a.as_str().unwrap().contains("lighthouse")));
}
#[test] fn the_job_follows_the_lanes_options() {
    let opts = LocalJobOptions { processor_only: true, threads: Some(3), ..Default::default() };
    let a = strings(build_args(&provider.job_for(&klein, &row, &req("x"), &opts, Some("vulkan0".into()), dir, "r1").unwrap()).unwrap());
    assert_eq!((after(&a, "--backend"), after(&a, "--threads")), ("cpu", "3"));
    let opts = LocalJobOptions { memory_saving: true, ..Default::default() };
    let a = strings(build_args(&provider.job_for(&klein, &row, &req("x"), &opts, Some("vulkan0".into()), dir, "r1").unwrap()).unwrap());
    assert_eq!(after(&a, "--backend"), "vulkan0"); assert!(a.contains(&"--offload-to-cpu".into()));
}
#[tokio::test] async fn start_local_matches_the_device_by_description() {
    // fake engine mode ok; opts.device_description "AMD Radeon RX 7600 (RADV NAVI33)" → the recorded argv has "--backend" "vulkan0"
}
#[tokio::test] async fn a_picture_run_ends_done_free_with_its_file() {
    let Started::Job { remote_id } = provider.start(&req("x"), scratch).await.unwrap() else { panic!() };
    let done = poll_until_done(&provider, &remote_id).await;                // JobState::Done(job)
    assert_eq!(done.cost, Some(Cost { usd: 0.0, reported: true }));
    assert!(matches!(provider.download(&remote_id, &done, 0, &dest).await.unwrap(), Fetched::Saved(s) if s.mime == "image/png"));
}
#[tokio::test] async fn cancel_kills_the_run_and_removes_its_partial_folder() { /* mode hang; cancel → Ok within 1 s; .partial/<run> absent; status(remote_id) None */ }
#[tokio::test] async fn a_file_of_another_family_is_refused_before_the_engine_starts() {
    // klein's diffusion part replaced by models::gguf::testing::diffusion_gguf("klein9b-tensors.txt") (no metadata keys, a 9B table)
    assert!(matches!(provider.start(&req("x"), scratch).await, Err(MediaError::Answered(f)) if f.kind == FailureKind::ModelFileBad));
    assert!(no_argv_recorded(&rec));
}
#[tokio::test] async fn release_removes_the_run_folder_and_its_prompt_file() { /* after download(0) and release(remote_id): .partial/<run> absent */ }
#[tokio::test] async fn a_missing_part_says_download_it_again() { /* the vae file deleted → Answered(ModelFileBad), charged Some(false) */ }
#[tokio::test] async fn no_engine_says_engine_missing() { /* set_engine(None) → Answered(EngineMissing) */ }
#[tokio::test] async fn out_of_memory_twice_is_a_named_failure_not_charged() {
    // mode oom → check → Failed(f): f.kind == OutOfMemory, f.charged == Some(false)
}
#[test] fn the_metal_quirk_sets_its_variable_on_macos_only() {
    // job_for on z-image-turbo-q4: env contains ("GGML_METAL_TENSOR_DISABLE","1") iff cfg!(target_os = "macos")
}
#[test] fn a_clip_length_becomes_frames_by_the_familys_rule() { /* wan 2 s at 24 fps → frames 49; 5 s → 121 */ }
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::local`
Expected: FAIL to compile, `cannot find type 'LocalMediaProvider'`.

- [ ] **Step 3: Implement.** Before spawning: the engine is set (else `EngineMissing`); every part file exists with its recorded size (else `ModelFileBad`); when the diffusion part is a `.gguf`, `gen_search::family_from_tensors(models::gguf::read_tensor_shapes(path))` is the entry's family (else `ModelFileBad`; a `.safetensors` part, such as Wan 2.1 1.3B's fp16 file, was checked by size and SHA-256 when it downloaded); a request whose prompt is over `PROMPT_MAX_CHARS` is `BadRequest` (the manager refuses it earlier, Task L15); the device for `--backend` is `devices::match_llama_device(list_devices(engine) (cached per engine), opts.device_description)`, else none (the engine's default), and `cpu` when `opts.processor_only`. An `ArgsError` from `build_args` is `BadRequest` with its sentence (the prompt length, a size, the `%d` folder). `SdFailure` maps to `FailureKind`: `OutOfMemory` → `OutOfMemory`, `DeviceLost` → `DeviceLost`, `BadModel` → `ModelFileBad`, `EngineMissing`/`EngineBlocked`/`MissingLibrary` → `EngineMissing`, `Crashed`/`Io` → `EngineFailed`; `charged` is always `Some(false)`. Video output is WebM, or WebP on macOS for an entry with the `video-webp-on-mac` quirk.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio::local && bash scripts/check-spawn.sh`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio
git commit -m "Studio: local models behind the same MediaProvider, free, with the exact command and checks before the engine starts"
```

### Task L15: The manager's local lane

**Files:**
- Create: `src-tauri/src/studio/quit.rs`
- Modify: `src-tauri/src/studio/manager.rs` (`start_local`, `estimate_local`, `preview_local`, the lane, `cancel` and `resume_interrupted` for local runs, `local_running`), `src-tauri/src/studio/deps.rs` (`LocalLane`, `LocalHardware`, `CardLease`, their app implementations), `src-tauri/src/studio/manager_tests.rs` (fakes: `FakeLocal` provider with `Company::Local` that records each `start_local`'s `LocalJobOptions`, `FakeLease` that records grants, hold drops and reloads, `FixedHardware`), `src-tauri/src/studio/errors.rs`, `src-tauri/src/studio/commands.rs` (`studio_quit_now`, `STUDIO_COMMANDS`), `src-tauri/src/lib.rs` (builds `LocalMediaProvider` and `GpuLease`, passes `local: Some(LocalLane { .. })`; `WindowEvent::CloseRequested` on the main window and `RunEvent::ExitRequested` both go through `quit::ask_before_quit`: when it says ask, `api.prevent_close()` / `api.prevent_exit()` and emit `studio-quit-requested`), `src-tauri/src/updates.rs` (`install_update` refuses while `local_running()`), `src-tauri/commands.list`, `src-tauri/capabilities/main.json`
- Test: `manager_tests.rs`, inline in `quit.rs`; the existing harness passes `local: None` until a test gives it one

**Interfaces:**
- Consumes: Tasks A13–A14, L8, L12–L14.
- Produces:
```rust
// deps.rs
#[async_trait] pub trait CardLease: Send + Sync {
    async fn acquire(&self, need: &StudioNeed, policy: GpuPolicy, answered: Option<bool>, cancel: &CancellationToken,
        on_wait: &(dyn Fn(LeaseWait) + Send + Sync)) -> Result<(Grant, Box<dyn std::any::Any + Send + Sync>), Refused>;   // the box is the hold
    async fn reload_chat(&self, path: &str);
}
#[async_trait] pub trait LocalHardware: Send + Sync {
    async fn target(&self) -> (models::fit::Target, String /* backend */, Option<String> /* device description */);
    fn chat_device_bytes(&self) -> Option<u64>;
    fn logical_cores(&self) -> usize;                          // std::thread::available_parallelism in the app
}
pub struct LocalLane { pub provider: Arc<dyn LocalProvider>, pub lease: Arc<dyn CardLease>, pub hardware: Arc<dyn LocalHardware> }
pub struct AppLease(pub Arc<GpuLease>); pub struct AppHardware(pub AppHandle);           // the app's implementations
// manager.rs: StudioDeps gains `pub local: Option<LocalLane>`
pub const LOCAL_CHECK_EVERY: Duration = Duration::from_millis(250);
impl StudioManager {
    pub fn local_running(&self) -> bool;                       // a local run is queued or running
    pub async fn answer_unload(&self, run_id: &str, unload: bool) -> AppResult<RunDto>;   // the answer under GpuPolicy::Ask
}
/// What one run's engine job gets: processor only from the grant (`UseProcessor`), the `run_on` field (a picture's) or a processor-only computer, with
/// `processor_threads`; memory saving from the `memory_saving` field, `Automatic` → `gen_fit::needs_memory_saving` of the fit it will run
/// with (`running_fit`, the after-unload one when the grant unloaded the chat model); the device description from `LocalHardware::target`.
pub fn job_options(grant: &Grant, fit: &GenFitResult, req: &MediaRequest, device: Option<&str>, logical_cores: usize) -> LocalJobOptions;
// quit.rs
/// Closing the main window (`exit_code` None) or an exit request without a code (the app menu's Quit, ⌘Q) asks while a local job works;
/// `app.exit(0)` from `studio_quit_now` carries a code and is never asked again.
pub fn ask_before_quit(local_running: bool, exit_code: Option<i32>) -> bool;
```
```text
studio_quit_now() -> ()                  // cancels the local run, then app.exit(0) ("Quit and stop")
studio_answer_unload(run_id: String, unload: bool) -> RunDto
```
Errors: `studio_prompt_too_long_here` ("Keep the prompt under 4,000 characters for local models." / "Shorten it, or choose a cloud model."), `studio_does_not_fit` ("This model does not fit this computer." / "Choose a smaller size or a cloud model."), `studio_busy_for_update` ("A picture or video is being made on this computer." / "Stop it in Studio, or install the update when it finishes.").

- [ ] **Step 1: Write the failing tests** (`manager_tests.rs`)
```rust
#[tokio::test] async fn a_local_picture_runs_without_confirmation_and_costs_nothing() {
    let h = harness_with_local().await;
    let run = h.manager.start(h.local_req("lighthouse"), false).await.unwrap();
    let done = h.wait_status(&run.id, "done").await;
    assert_eq!(h.statuses(&run.id), ["queued", "running", "done"]);
    assert_eq!((done.runs_on.as_str(), done.cost_usd, done.provider_name.as_str()), ("local", Some(0.0), "This computer"));
    assert!(done.local.as_ref().unwrap().labelled); assert!(read_label(Path::new(&done.outputs[0].path), "image/png").is_some());
    assert_eq!(h.manager.usage().await.unwrap().spent_today_usd, 0.0);
    assert_eq!(h.local.releases(), 1);                                        // .partial/<run> and its prompt file gone
}
#[tokio::test] async fn a_local_video_never_asks_and_never_meets_the_daily_limit() {
    h.seed_done_run_costing(9.99).await;
    assert!(h.manager.start(h.local_video_req(), false).await.is_ok());
}
#[tokio::test] async fn one_local_job_at_a_time_while_cloud_runs_go_on_beside_it() {
    // FakeLocal holds each run; two local starts → [running, queued]; a cloud picture meanwhile → sending
}
#[tokio::test] async fn waiting_for_the_graphics_card_is_queued_with_its_reason() {
    // FakeLease waits with ReplyRunning → status queued, local.waiting_for == Some("reply")
}
#[tokio::test] async fn the_unloaded_chat_model_is_named_on_the_card() {
    // FakeLease → Grant::AfterUnload { unloaded: Some("/m/chat.gguf") } → running card local.unloaded_chat == Some("/m/chat.gguf")
    // FixedHardware gpu(3, 32) with a 5 GB chat model and z-image → FakeLease saw StudioNeed { alongside: false, after_unload: true, .. }
}
#[tokio::test] async fn a_job_that_fits_after_an_unload_can_be_created() {
    // FixedHardware gpu(3, 32), chat 5 GB, z-image (partly on the processor now, one part at a time after the unload):
    let e = h.manager.estimate(&h.local_req_for("z-image-turbo-q4", "x")).await.unwrap();
    assert_eq!(e.local.as_ref().map(|l| (l.fit.as_str(), l.unload_chat)), Some(("afterUnload", true)));
    assert!(e.invalid.is_empty()); assert!(h.manager.start(h.local_req_for("z-image-turbo-q4", "x"), false).await.is_ok());
}
#[tokio::test] async fn the_grant_and_the_fit_reach_the_engine_job() {
    // prefs gpu_policy Processor, FakeLease → Grant::UseProcessor, logical_cores 8 → FakeLocal's opts.processor_only,
    //   opts.threads == Some(processor_threads(8, cfg!(target_arch = "x86_64")))
    // FixedHardware gpu(6, 32), z-image (one part at a time), memory_saving unset (Automatic), Grant::Alongside → opts.memory_saving
    // FixedHardware's device description "AMD Radeon RX 7600 (RADV NAVI33)" → opts.device_description == that
    // memory_saving "off" in the request → opts.memory_saving false, whatever the fit
    // a running card: local.on_processor for the first, local.alongside for the Alongside grant
}
#[tokio::test] async fn the_card_stays_held_through_the_out_of_memory_retry() {
    // FakeLocal reports Running(retrying: true), then Done; FakeLease (AfterUnload): hold dropped exactly once, after done; reload_chat never called
}
#[tokio::test] async fn the_chat_model_reloads_once_when_the_queue_empties() {
    // prefs reload_chat_after true; two local runs queued, the first granted AfterUnload { unloaded: Some("/m/chat.gguf") }, the second Alongside
    // → FakeLease.reloads() == ["/m/chat.gguf"], recorded after the second run is done; with reload_chat_after false → no reload
}
#[tokio::test] async fn a_job_that_does_not_fit_even_after_an_unload_is_refused() {
    // FixedHardware cpu(16) and a video entry → start → "studio_does_not_fit"; nothing stored, FakeLocal starts == 0
}
#[tokio::test] async fn ask_waits_for_the_answer() {
    // prefs gpu_policy Ask; FakeLease → Refused::NeedsAnswer → status queued, waiting_for "answer"; answer_unload(id, true) → running
}
#[tokio::test] async fn stop_on_a_local_run_kills_it_and_says_nothing_was_charged() {
    let r = h.manager.cancel(&run.id).await.unwrap();
    assert_eq!(r.status, "cancelled"); assert_eq!(h.local.cancels(), 1);
    assert_eq!(r.error.unwrap().message, "Stopped. Your computer is no longer working on it. Nothing was charged.");
}
#[tokio::test] async fn a_local_run_left_running_becomes_interrupted_after_a_restart() {
    // seeded rows: local running, local queued → resume_interrupted → both interrupted, charged Some(false),
    // message "The app closed while this was being made."; FakeLocal starts == 0
}
#[tokio::test] async fn studio_wont_make_what_the_check_refuses() { /* a refused prompt → "studio_wont_make"; nothing stored, no start */ }
#[tokio::test] async fn a_local_prompt_over_4000_characters_is_refused_before_anything_is_stored() {
    let long = "a".repeat(4_001);
    assert_eq!(h.manager.start(h.local_req(&long), false).await.unwrap_err().code, "studio_prompt_too_long_here");
    assert!(h.store_runs().await.is_empty());
    assert!(h.manager.estimate(&h.local_req(&long)).await.unwrap().invalid.contains(&"Keep the prompt under 4,000 characters for local models.".to_string()));
}
#[tokio::test] async fn the_estimate_for_a_local_run_is_a_time_not_a_price() {
    // FixedHardware gpu(8, 32), an entry with a measured run → EstimateDto { usd: Some(0.0), reasons: [], eta: Some(_),
    //   local: Some(LocalEstimateDto { fit: "fits", unload_chat: false, very_slow: false, over_15_min: false }) }
    // a measured run of 20 minutes → very_slow true and over_15_min true; FixedHardware cpu(16) + video → fit "notHere", very_slow true,
    // over_15_min false
}
#[tokio::test] async fn local_progress_is_sent_at_most_four_times_a_second() { /* 100 FakeLocal progress steps within 1 s → <= 4 progress-only events */ }
#[tokio::test] async fn the_audit_row_says_this_computer_and_never_the_prompt() {
    // arguments["runsOn"] == "local", arguments["engine_build"] == 1106, arguments["catalog_id"] == "flux2-klein-4b-q4"; no prompt text
}
#[tokio::test] async fn quitting_is_asked_only_while_a_local_job_works() { /* local_running() true while running, false after done */ }
#[tokio::test] async fn an_update_waits_for_a_local_job() { /* local_running → install_update's check → "studio_busy_for_update"; idle → passes */ }
// quit.rs
#[test] fn the_window_and_the_app_menu_ask_the_same_way() {
    assert!(ask_before_quit(true, None)); assert!(!ask_before_quit(false, None));
    assert!(!ask_before_quit(true, Some(0)));                                 // studio_quit_now's own app.exit(0)
}
// commands.rs: studio_quit_now and studio_answer_unload join STUDIO_COMMANDS (listed, granted, registered under studio::commands)
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::manager && cargo test -p app --lib studio::quit`
Expected: FAIL to compile, `no field 'local' on type 'StudioDeps'`.

- [ ] **Step 3: Implement.** `start_local`, under the `starting` lock: the client token as for cloud; `prompt_check::refuses` → `wont_make()`; a prompt over `PROMPT_MAX_CHARS` → `studio_prompt_too_long_here`; the model from `parse_catalog(Company::Local, provider.fetch_catalog())`; `spec::validate`; a seed when unset; `billing_of(&Price::Free, ..)` makes the confirmation and the limit pass (never consulted for refusal); the fit and estimate from `provider.entry_of(model)`, `LocalHardware::target()`, `gen_fit` (with `chat_device_bytes`) and `gen_eta::estimate` (measured runs from `store.local_run_times(model, device, 10)`); the word from `gen_eta::fit_word`; `NotHere` → `studio_does_not_fit`; the lease's `StudioNeed` is `alongside`: `fit` is Green or one part at a time, `after_unload`: `after_unload` is set and not Red, `processor_ok`: a picture entry whose `gen_fit` on `Target::Cpu` (the computer's memory) is not Red, `shared_memory`: the target is `AppleUnified`; insert the run (`runs_on: Local`, `provider_id: None`, `provider_name: "This computer"`, `provider_host: ""`, `estimate_usd: Some(0.0)`, `gen_model_id`, `engine_build`, `device`); the audit row (arguments as cloud plus `runsOn`, `catalog_id`, `revision`, `engine_build`; preview "Made a picture on this computer with <model>"); enqueue in the single local queue. The lane task: `queued` → `lease.acquire` (waits shown as `local.waiting_for`; `NeedsAnswer` parks the run until `answer_unload`; `DoesNotFit` → `failed` with `studio_does_not_fit`) → `running` with `started_at`, `local.alongside` (grant `Alongside` with a chat model loaded) and `local.on_processor` → `provider.start_local(req, scratch, &job_options(..))` → `check` every `LOCAL_CHECK_EVERY` (events at most 4 a second; `local` filled from `provider.status`, `eta.left_ms` from `gen_eta::left_ms`) → `Done`: each output `download`ed into `.partial/<output id>`, `gallery.finalize`, `label::label_file` (model, revision, the output id, now), output row, thumbnail, sidecar, `done` with `cost_usd: Some(0.0)`, `elapsed_ms`; `Failed(f)` → `failed` with `from_failure` and `charged: Some(false)`; on both, `provider.release(remote_id)`. The hold lives for the whole run (an out-of-memory retry happens inside it) and is dropped on every exit; the chat model a grant unloaded is remembered while the local queue has work, and when the queue empties and `prefs.reload_chat_after` is on, `lease.reload_chat(path)` runs once. Cancel: `queued` → `cancelled`; `running` → `provider.cancel` → `cancelled` with the stop sentence. `resume_interrupted` handles local rows first (as in the test) and leaves cloud rows to Task A14's rules. `estimate_local` and `preview_local` use the same steps without inserting: `LocalEstimateDto { fit: fit_word(..), unload_chat: running_fit(..).1, very_slow: very_slow(word, eta), over_15_min: over_15_min(word, eta) }`, `invalid` carrying the prompt-length sentence when it applies, and `preview_local` asking `provider.preview_local` with the options the run would get (a grant of `Alongside`, or `AfterUnload` when the word is `afterUnload`). In `lib.rs`, `RunEvent::ExitRequested { code, api, .. }` with `ask_before_quit(studio.local_running(), code)` → `api.prevent_exit()` and the event; the main window's `CloseRequested` the same with code `None`. `install_update` first asks `app.try_state::<Arc<StudioManager>>()` and refuses with `studio_busy_for_update` while a local job runs (the updater's restart would otherwise kill it without the question).

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio && cargo test -p app --lib updates && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio src-tauri/src/lib.rs src-tauri/src/updates.rs src-tauri/commands.list src-tauri/capabilities/main.json
git commit -m "Studio: one local job at a time beside the cloud queues, free, labelled, stoppable, and asked about on quit"
```

### Task L16: Get local models: commands and DTOs

**Files:**
- Create: `src-tauri/src/studio/local_cmd.rs`
- Modify: `src-tauri/src/dto.rs` (local DTOs), `src-tauri/src/studio/mod.rs`, `src-tauri/src/lib.rs`, `src-tauri/commands.list`, `src-tauri/capabilities/main.json`, `src-tauri/src/studio/commands.rs` (`STUDIO_COMMANDS`)
- Test: inline in `local_cmd.rs`

**Interfaces:**
- Consumes: Tasks L6–L15.
- Produces (each checks `only_main_window`; only `studio_local_search`, `studio_local_repo` and `studio_local_download` touch the network, each on a click):
```text
studio_local_models() -> LocalModelsDto                           // no network
studio_local_search(text: String, kind: MediaKind) -> LocalSearchPage   // Task L7's three queries, merged: one page, no "more"
studio_local_repo(repo: String) -> LocalRepoDto                   // the listing at its latest commit, recognised here
studio_local_fit(source: LocalSource, width: u32, height: u32, frames: u32) -> LocalFitDto   // no network for a catalog or installed source
studio_local_download(source: LocalSource, accepted_license_sha256s: Vec<String>) -> String
studio_local_delete(gen_model_id: String) -> ()
studio_accept_local_notice() -> StudioPrefs                        // sets local_notice_accepted_at = now
```
```rust
pub struct HardwareLineDto { pub device: String, pub graphics_bytes: Option<u64>, pub shared_memory: bool, pub ram_bytes: Option<u64>, pub free_disk_bytes: Option<u64> }
pub struct LicenceDto { pub id: String, pub name: String, pub url: String, pub sha256: String, pub badge: String /* green|amber|red */,
    pub plain_words: String, pub obligations: Vec<String> }
pub struct GateDto { pub mode: String /* none|auto|manual */, pub collects: Vec<String> }
pub struct PartDto { pub role: String, pub label: String /* "makes the picture" | "reads your prompt" | "picture decoder" */, pub repo: String,
    pub path: String, pub size_bytes: u64, pub licence_id: String, pub already_here: bool }
pub struct LocalFitDto { pub word: String /* fits|slow|afterUnload|notHere */, pub verdict: String /* green|yellow|red */, pub reason: Option<String>,
    pub suggestion: Option<String>, pub eta: Option<EtaDto>, pub very_slow: bool }
pub struct LocalEntryDto { pub id: String, pub name: String, pub summary: String, pub publisher: String, pub kind: MediaKind, pub size_bytes: u64,
    pub licence: LicenceDto /* the strictest, for the badge */, pub licences: Vec<LicenceDto> /* every distinct part licence: all are accepted */,
    pub gate: GateDto, pub fit: LocalFitDto, pub recommended: bool, pub installed: bool, pub parts: Vec<PartDto> }
pub struct InstalledLocalDto { pub id: String, pub name: String, pub kind: MediaKind, pub size_bytes: u64, pub fit: LocalFitDto, pub model: MediaModelDto }
pub struct LocalModelsDto { pub hardware: HardwareLineDto, pub engine: EngineStatusDto, pub installed: Vec<InstalledLocalDto>,
    pub recommended: Vec<LocalEntryDto>, pub not_here: Vec<LocalEntryDto>, pub notice_accepted: bool, pub video_possible: bool,
    pub installed_bytes: u64, pub more_fit: u32 }                     // more_fit: recommended entries that fit and are not installed
pub struct LocalSearchHitDto { pub repo: String, pub downloads: u64, pub likes: u64, pub gated: String, pub license_label: Option<String> }
pub struct LocalSearchPage { pub items: Vec<LocalSearchHitDto> }
pub struct LocalFileChoiceDto { pub file: String, pub quant: String, pub parts: Vec<PartDto>, pub licences: Vec<LicenceDto>, pub total_bytes: u64,
    pub to_fetch_bytes: u64, pub fit: LocalFitDto, pub needs_engine: bool, pub low_disk: bool }
pub struct LocalRepoDto { pub repo: String, pub commit: String, pub status: String /* supported|notSupportedYet|notOffered */,
    pub reason: Option<String>, pub family: Option<String>, pub files: Vec<LocalFileChoiceDto>, pub licence: Option<LicenceDto>,
    pub licences: Vec<LicenceDto>, pub label_mismatch: Option<String>, pub gate: GateDto }
```
Every `EtaDto` the commands return is already rounded with `gen_eta::round_for_display`. The word is `gen_eta::fit_word` (Task L8: a job that fits once the chat model is unloaded is `afterUnload` before any red or slow rule); the reason for `notHere` comes from `NotFor`, judged with the chat model's memory counted back ("needs 6 GB of graphics memory" (the need rounded up to whole GB), "needs a Mac with 32 GB or more", "video needs a graphics card or an Apple-silicon Mac", "local video needs a graphics card on this computer"). The verdict colour is that of `running_fit`: Green → green, one part at a time or partly on the processor → yellow, Red → red. `very_slow` is `gen_eta::very_slow`. The ETA is computed for the fit the run would have. "Recommended for this computer" marks, per kind, the first id in the highest tier at or below this computer's usable memory whose word is `fits`.

- [ ] **Step 1: Write the failing tests**
```rust
#[tokio::test] async fn opening_get_local_models_makes_no_request() {
    // local_models(&deps) with an HfClient pointed at a MockServer → server.requests().is_empty()
}
#[tokio::test] async fn the_recommended_list_is_the_starters_with_one_marked_per_kind() {
    // gpu(8, 32): recommended ids == the 3 starters; exactly one image and one video with recommended == true; more_fit == 3
}
#[tokio::test] async fn models_that_do_not_fit_are_listed_with_a_plain_reason() {
    // gpu(2, 4): z-image in not_here with reason "needs 6 GB of graphics memory"; klein listed (partly on the processor: "slow")
}
#[tokio::test] async fn a_model_that_fits_after_the_chat_model_is_unloaded_says_so() {
    // gpu(3, 32) with a 5 GB chat model: z-image word "afterUnload", verdict "yellow", in recommended, not in not_here
}
#[tokio::test] async fn every_licence_of_a_model_is_listed_for_acceptance() {
    // an entry whose parts are Apache-2.0 and Kommunity → licence.id "kommunity" (amber badge), licences ids ["apache-2.0", "kommunity"]
}
#[tokio::test] async fn an_installed_model_gets_its_fit_without_a_request() {
    // the installed searched model "hf:someone/Z-Image-Turbo-GGUF:z.gguf", source {"installed":{"genModelId":…}} at 832×480 → a LocalFitDto;
    // the MockServer behind the HfClient saw no request
}
#[tokio::test] async fn video_is_hidden_on_a_processor_only_computer() {
    // cpu(16): video_possible false; the video entry in not_here with "video needs a graphics card or an Apple-silicon Mac"
}
#[tokio::test] async fn a_searched_repo_of_an_unknown_family_offers_no_download() {
    // MockServer answers repo-unknown-diffusion.json → status "notSupportedYet", files empty
}
#[tokio::test] async fn a_searched_repo_lists_each_file_with_the_shared_parts_already_here() {
    // repo-z-image-turbo-gguf.json and the Qwen3 encoder already installed → files[0].parts[1].already_here, to_fetch_bytes < total_bytes,
    // licence badge "green"
}
#[tokio::test] async fn accepting_the_notice_records_when() { /* fixed clock 1000 → prefs.local_notice_accepted_at == Some(1000) */ }
// commands.rs: the seven names above join STUDIO_COMMANDS; Task A16's test finds them registered as `studio::local_cmd::<name>,`
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::local_cmd`
Expected: FAIL to compile.

- [ ] **Step 3: Implement** each command as a thin wrapper over a plain function taking its dependencies (store, catalog source, hardware, an `Option<HfClient>`), so the tests call the functions directly. `studio_local_search` uses `hf_client(app)` (the stored token applies, so gated repositories list) and `search_generators`; `studio_local_repo` calls `HfClient::repo(repo, None)` then `recognise`, `entry_for` per file, `plan_download` and `gen_fit` at the entry's default size, and `searched_licence` for the licence and its mismatch sentence; `licences` lists every distinct licence of the entry's parts, each with its pinned SHA-256, so `studio_local_download` gets them all.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio && cargo test -p app --lib commands`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio src-tauri/src/dto.rs src-tauri/src/lib.rs src-tauri/commands.list src-tauri/capabilities/main.json
git commit -m "Studio: Get local models commands: the list, Hugging Face search, a repository's parts, fit, download and the notice"
```

### Task L17: Apply the benchmark results (gated)

Done when the owner checklist's **Local benchmark → Results** table (Task L1) is filled in on the owner's machines; it can land any time after Task L16, nothing else in the plan waits for it, and the 0.1.0 release does. Until then the provisional video entry and the hand-written fixtures stand, and the UI shows "Time shown after your first run here".

**Files:**
- Modify: `catalog/models.json` (the video entry kept with the benchmark's steps, cfg, flow shift and quant, or replaced; `wan21-t2v-1.3b` added with its tier (`"video": {"6": ["wan21-t2v-1.3b"], "8": ["wan22-ti2v-5b-fast-q6", "wan21-t2v-1.3b"]}`) and the Wan 2.2 entry's `runsOnApple` when the Results say so; every starter's `memory[]` and `speed[]`; each family's fallback raised where a measured peak exceeds what it predicts; one shared `llm` part for both picture families when one encoder served both; the `video-webp-on-mac` quirk when WKWebView could not play WebM; `sequence` bumped and the file signed again), `crates/runtime/tests/fixtures/` (the recordings over the hand-written `sd_progress_*`, `sd_stderr_out_of_memory_*`, `sd_list_devices_*` and `sd_help.txt`; `sd_fixtures_build.txt`), `crates/runtime/src/sd/progress.rs` and `failure.rs` (only where a recording differs from the markers and patterns of Task L3), `crates/models/src/gen_search.rs` (the test below), `docs/dev/0.1.0/owner-checklist.md` (each Results row marked applied)
- Test: the new test below; Tasks L3–L5's tests over the recordings; `the_recommended_list_is_free_for_any_use_and_ungated` (3 or 4 entries)

**Interfaces:** none new (catalog data and fixtures).

- [ ] **Step 1: Write the failing test** (`crates/models/src/gen_search.rs`)
```rust
#[test] fn every_starter_has_measured_points() {
    for e in gen_catalog(&Catalog::bundled(), runtime_build()).entries.values() {
        assert!(!e.memory.is_empty() && !e.speed.is_empty(), "{}: no measured memory or speed point", e.id);
        assert!(e.speed.iter().all(|p| !p.measured_on.is_empty()), "{}", e.id);
        assert!(e.memory.iter().all(|m| m.build == runtime_build()), "{}: a memory point from another engine build", e.id);
    }
}
```

- [ ] **Step 2: Run to see it fail**
Run: `cargo test -p models --lib gen_search::tests::every_starter_has_measured_points`
Expected: FAIL (`flux2-klein-4b-q4: no measured memory or speed point`).

- [ ] **Step 3: Apply the Results**, row by row: the video decision of Task L1 item 2 (keep, replace, or add Wan 2.1 1.3B as a fourth entry), the measured points of item 4 (each speed point with `measuredOn`), the playback quirk of item 5, the shared encoder of item 7, and the recordings of item 8; every changed part pinned again from its listing (canonical id, revision, size, SHA-256). Re-run Tasks L3–L5's tests on the recordings and change `progress.rs`'s markers or `failure.rs`'s patterns only where a recording differs, with the recording as the test.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p models && cargo test -p runtime --lib sd:: && cargo test -p runtime --test sd_cli && python3 -I scripts/docs_test.py`
Expected: PASS (the docs test now allows time figures, because the catalog has speed points).

- [ ] **Step 5: Commit**
```bash
git add catalog/models.json crates/runtime/tests/fixtures crates/runtime/src/sd crates/models/src/gen_search.rs docs/dev/0.1.0/owner-checklist.md
git commit -m "Studio: the benchmark's measured points, video entry and recordings in the catalog and the engine fixtures"
```

---

## Phase B: Webview

All webview tests mock `src/components/studio/api.ts` with `vi.mock("./api")` (or the relative path) and build data from `MODELS`, `RUN`, `LOCAL` and `LOCAL_RUN` in `src/components/studio/fixtures/models.ts` (generated in Task B1), as `ChatView.cloud.test.tsx` and `settings/Endpoints.test.tsx` mock `lib/ipc`. A model the fixture lacks (for example a video model without a start frame) is made in the test by copying a fixture model and removing a field; a computer the fixture lacks (no usable graphics card, nothing installed) by copying `LOCAL` and changing its fields. Every Studio screen is local-first: "On this computer" before cloud, a time before a price for local models, cloud offered as the backup.

### Task B1: Studio store and navigation

**Files:**
- Create: `src/components/studio/types.ts`, `src/components/studio/api.ts`, `src/store/studio.ts`, `src/store/studio.test.ts`
- Create (generated by the Rust test below): `src/components/studio/fixtures/models.ts`
- Modify: `src-tauri/src/studio/commands.rs` (the fixture test), `src/App.tsx`, `src/App.test.tsx`, `src/components/Sidebar.tsx`, `src/components/Sidebar.test.tsx`, `src/components/ShortcutsDialog.tsx`, `src/lib/words.ts`

**Interfaces:**
- Consumes: Tasks A13, A15, A16, L10, L15 and L16 commands, events and DTOs.
- Produces:
```ts
// types.ts: TS mirrors with the DTO names, camelCase fields exactly as Tasks A13, A15, L10 and L16:
// MediaKind, Company, Choice, SizeRules, ParamKind, Canonical, ParamSpec, RefRole, PriceHint, EndpointCompany, MediaModel,
// StudioEndpoint, RunStatus, RunError, Output, RefItem, Run, Eta, LocalRun, Limit, ConfirmReason, Estimate, LocalEstimate, Routing,
// RunTarget, GpuPolicy, StartRequest, Setup, StudioPrefs, StudioPrefsPatch (= Partial<StudioPrefs> without localNoticeAcceptedAt), Usage,
// RunsFilter, RunsPage, RequestPreview, ImageInput (from lib/types), EngineStatus, EngineProgress, HardwareLine, Licence, Gate, Part,
// LocalFit, LocalEntry, InstalledLocal, LocalModels, LocalSearchHit, LocalSearchPage, LocalFileChoice, LocalRepo, LocalSource
export type RunTarget = { endpoint: number } | "thisComputer";
export type LocalSource = { catalog: { entryId: string } } | { search: { repo: string; commit: string; file: string } }
  | { installed: { genModelId: string } };
export type ConfirmReason = "video" | "atLeast" | "unknownPrice";
export type ParamKind = { type: "enum"; values: Choice[] } | { type: "range"; min: number; max: number; step?: number | null; unit?: string | null }
  | { type: "boolean" } | { type: "seed"; max: number } | { type: "images"; min: number; max: number; role: RefRole }
  | { type: "size"; rules: SizeRules } | { type: "extra"; allowedKeys: string[] };
// api.ts: one wrapper per command, e.g.
export const studioModels = () => invoke<{ endpoints: StudioEndpoint[] }>("studio_models");
export const studioStart = (req: StartRequest, confirmed: boolean) => invoke<Run>("studio_start", { req, confirmed });
export const studioOutputBytes = (outputId: string, thumb: boolean) => invoke<ArrayBuffer>("studio_output_bytes", { outputId, thumb });
export const studioSetPrefs = (patch: StudioPrefsPatch) => invoke<StudioPrefs>("studio_set_prefs", { patch });
export const studioLocalModels = () => invoke<LocalModels>("studio_local_models");
export const studioLocalDownload = (source: LocalSource, acceptedLicenseSha256s: string[]) =>
  invoke<string>("studio_local_download", { source, acceptedLicenseSha256s });
// … and studioLocalSearch, studioLocalRepo, studioLocalFit, studioLocalDelete, studioAcceptLocalNotice,
// studioEngineStatus, studioEngineDownload, studioEngineCancel, studioEngineRemove, studioAnswerUnload, studioQuitNow
export function onStudioProgress(handler: (run: Run) => void): Promise<UnlistenFn>;     // "studio-progress"
export function onEngineProgress(handler: (p: EngineProgress) => void): Promise<UnlistenFn>;   // "studio-engine-progress"
export function onQuitRequested(handler: () => void): Promise<UnlistenFn>;                     // "studio-quit-requested"
// store/studio.ts
export type StudioTab = MediaKind;
export interface StudioState {
  view: "chat" | "studio"; tab: StudioTab; live: Record<string, Run>; readyNotice: string | null;
  showStudio(tab?: StudioTab): void; showChat(): void; applyProgress(run: Run): void;
}
export const useStudio: UseBoundStore<StoreApi<StudioState>>;
export function runningCount(s: StudioState): number;       // queued, sending, running, downloading, and waiting while watched
export function followStudio(): () => void;                 // studio-progress through listenInEffect, plus studioLive() once
```

- [ ] **Step 1: Write the failing tests**
```rust
// src-tauri/src/studio/commands.rs
#[test] fn the_webview_fixture_is_current() {
    // six models (gpt-image-2.6-flare with its recorded /endpoints answer, gemini-nano-banana-2.1, krea/k3-edit, veo-3.1,
    // grok-imagine-video-1.5-lite, seedance-2.5-flash) as MediaModelDto from the catalogue fixtures, plus one RunDto; and
    // LocalModelsDto for an 8 GB Vulkan card with FLUX.2 klein 4B installed (from the bundled catalog, a fixed hardware line,
    // klein's fit "fits" with a measured time) and one finished local RunDto, rendered as
    //   // Generated by studio::commands::tests::the_webview_fixture_is_current. Do not edit.
    //   import type { LocalModels, MediaModel, Run } from "../types";
    //   export const MODELS: MediaModel[] = <json>;
    //   export const RUN: Run = <json>;
    //   export const LOCAL: LocalModels = <json>;
    //   export const LOCAL_RUN: Run = <json>;
    // and compared with src/components/studio/fixtures/models.ts; with STUDIO_WRITE_FIXTURE=1 it writes the file instead.
    // Typed literals (not a .json import, whose strings widen) make `npm run typecheck` fail when serde and types.ts disagree.
}
```
```ts
// App.test.tsx (StudioView mocked to a marker; studio api mocked)
it("Ctrl+G opens Studio and Esc there stops nothing", async () => {
  useApp.setState({ activeChatId: 1, generation: turn(1) });          // Esc would stop this reply in the chat view
  fireEvent.keyDown(window, { key: "g", ctrlKey: true });
  expect(useStudio.getState().view).toBe("studio");
  fireEvent.keyDown(window, { key: "Escape" });
  expect(ipcMock.stopGeneration).not.toHaveBeenCalled();
  act(() => useStudio.getState().showChat());
  fireEvent.keyDown(window, { key: "Escape" });
  expect(ipcMock.stopGeneration).toHaveBeenCalledWith(1);            // the seed was live
});
it("the stop hotkey leaves Studio runs alone", () => {
  useStudio.setState({ live: { r1: run({ status: "waiting", watching: true }) } });
  listeners.emit("stop-requested");                                   // what hotkey::stop_now sends
  expect(studioApi.studioCancel).not.toHaveBeenCalled();
  expect(useStudio.getState().live.r1.status).toBe("waiting");
});
it("app:open-studio opens Studio on the tab it names", () => {
  window.dispatchEvent(new CustomEvent("app:open-studio", { detail: "video" }));
  expect(useStudio.getState()).toMatchObject({ view: "studio", tab: "video" });
});
it("the chat stays mounted, hidden, while Studio is shown", () => { /* ChatView marker still in the DOM, inside a hidden element */ });
it("Ctrl+N goes back to the chat", () => { /* view "chat"; newChat called */ });
// Sidebar.test.tsx
it("the Studio button sits between Models and Settings and counts working runs", () => {
  useStudio.setState({ live: { r1: run({ status: "waiting", watching: true }) } });
  const buttons = screen.getAllByRole("button").map((b) => b.getAttribute("aria-label")).filter(Boolean);
  expect(buttons.slice(0, 3)).toEqual(["Models", "Studio (1 working)", "Settings"]);
});
it("a chat click or New chat leaves Studio", () => { /* view becomes "chat" */ });
// studio.test.ts
it("a video that finishes while the chat is shown raises the ready notice", () => {
  useStudio.getState().applyProgress(run({ id: "v", kind: "video", status: "done" }));
  expect(useStudio.getState().readyNotice).toBe("v");
});
it("runningCount ignores stopped and stale runs and counts a local run", () => {
  const live = { a: run({ status: "sending" }), b: run({ status: "waiting", watching: true }), c: run({ status: "waiting", watching: false, stale: true }),
                 d: run({ status: "cancelled" }), e: run({ status: "done" }), f: { ...LOCAL_RUN, id: "f", status: "running" } };
  expect(runningCount({ ...useStudio.getState(), live })).toBe(3);
});
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::commands::tests::the_webview_fixture_is_current; npx vitest run src/App.test.tsx src/components/Sidebar.test.tsx src/store/studio.test.ts`
Expected: FAIL (no fixture file yet; `Failed to resolve import "./store/studio"`).

- [ ] **Step 3: Implement.** App keeps `<ChatView/>` and `<Workbench/>` mounted inside a wrapper with `hidden` while `view === "studio"` (the composer's draft survives, and `app:compose-images` reaches it), and mounts `<StudioView/>` on first open and keeps it. Key handler: `mod+g` → `showStudio()`; `Escape` returns early when `view === "studio"`; `mod+n`, `mod+l` call `showChat()` first. App's effect calls `followStudio()`. Sidebar: `Sparkles` icon button, `aria-label` "Studio" or "Studio (N working)", title "Studio (Ctrl+G)", with a small count badge; chat rows and New chat call `showChat()`. Shortcut row `[\`${MOD} G\`, "Studio"]`. Write `types.ts`, then run `STUDIO_WRITE_FIXTURE=1 cargo test -p app --lib studio::commands::tests::the_webview_fixture_is_current` once.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio::commands && npx vitest run src/App.test.tsx src/components/Sidebar.test.tsx src/store/studio.test.ts && npm run typecheck`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio/types.ts src/components/studio/api.ts src/components/studio/fixtures/models.ts src-tauri/src/studio/commands.rs src/store/studio.ts src/store/studio.test.ts src/App.tsx src/App.test.tsx src/components/Sidebar.tsx src/components/Sidebar.test.tsx src/components/ShortcutsDialog.tsx src/lib/words.ts
git commit -m "Studio: sidebar button, Ctrl+G, and a view that leaves the chat running"
```

### Task B2: Studio view shell

**Files:**
- Create: `src/components/studio/StudioView.tsx`, `src/components/studio/StudioView.test.tsx`, `src/components/studio/LocalMark.tsx`
- Modify: `src/store/studio.ts` (endpoints, local, drafts, runs, prefs, usage, `load`, `refreshModels`, `refreshLocal`, `showLocalModels`), `src/lib/words.ts`

**Interfaces:**
- Produces:
```ts
export interface Draft { target: RunTarget | null; model: string | null; prompt: string; values: Record<string, unknown>;
  extra: Record<string, unknown>; refs: RefItem[]; seedLocked: boolean; routing: Routing | null; note: string | null }
// added to StudioState:
endpoints: StudioEndpoint[]; local: LocalModels | null; loaded: boolean; drafts: Record<StudioTab, Draft>; runs: Run[]; next: number | null;
filter: RunsFilter; prefs: StudioPrefs | null; usage: Usage | null; grid: boolean; localView: boolean;
load(): Promise<void>;                        // studioModels, studioLocalModels, studioRuns, studioLive, studioGetPrefs, studioUsage; never a refresh
refreshModels(providerId: number): Promise<void>;
refreshLocal(): Promise<void>;                // studioLocalModels again (after a download or a delete)
setTab(tab: StudioTab): void;
showLocalModels(show: boolean): void;         // Get local models in place of the card stream (Task B13)
selectInitialModel(tab: StudioTab): void;     // prefs.lastModel[tab] when still offered; else the default below; sets the draft only, never fetches
export function defaultModel(tab: StudioTab, local: LocalModels | null, endpoints: StudioEndpoint[]): { target: RunTarget; model: string } | null;
export type EmptyState = "getLocal" | "cloudBackup" | "addEndpoint" | null;
export function emptyState(tab: StudioTab, local: LocalModels | null, endpoints: StudioEndpoint[]): EmptyState;
export function StudioView(): JSX.Element;
```
`defaultModel`: an installed local model of the tab's kind whose fit word is `fits` (not `slow`, `afterUnload` counts as fitting), else the cloud default below, else `null`. `emptyState`, one ordered rule, the first that applies: (1) `null` when the tab has a model to use (an installed local model of its kind, or an endpoint model of its kind); (2) `getLocal` when a recommended entry of the tab's kind has the word `fits` or `afterUnload` (a second, smaller line names the cloud backup); (3) `cloudBackup` when no endpoint is saved at all; (4) `addEndpoint`: endpoints are saved but none makes this kind (on the Video tab with only OpenAI, name OpenRouter).

- [ ] **Step 1: Write the failing tests**
```ts
it("opening Studio reads only what is saved", async () => {
  await show();
  expect(api.studioModels).toHaveBeenCalledTimes(1); expect(api.studioLocalModels).toHaveBeenCalledTimes(1);
  expect(api.studioRefreshModels).not.toHaveBeenCalled();
  expect(api.studioModelPrice).not.toHaveBeenCalled(); expect(api.studioLocalSearch).not.toHaveBeenCalled();
});
it("with nothing set up, Studio leads with local models", async () => {   // getLocal
  await show({ endpoints: [], local: { ...LOCAL, installed: [] } });
  screen.getByText("Make pictures and videos on this computer: free, and nothing is sent.");
  screen.getByText(`${klein.name} fits this computer (${amountGb(klein.sizeBytes)}).`);   // klein = LOCAL.recommended[0]: "FLUX.2 klein 4B", about 5.3 GB
  fireEvent.click(screen.getByRole("button", { name: "Get local models" }));
  expect(useStudio.getState().localView).toBe(true);
  screen.getByText("No good graphics card? Cloud models are the backup: OpenRouter or OpenAI.");   // second, smaller
  fireEvent.click(screen.getByRole("button", { name: "Open Endpoints" }));
  expect(opened).toEqual(["endpoints"]);           // app:open-settings detail
});
it("each empty state follows one ordered rule", () => {
  const none = { ...LOCAL, installed: [] }, nothingFits = { ...none, recommended: [], notHere: LOCAL.recommended, videoPossible: false };
  expect(emptyState("image", LOCAL, [])).toBeNull();                                   // klein is installed
  expect(emptyState("image", none, [])).toBe("getLocal");
  expect(emptyState("image", nothingFits, [])).toBe("cloudBackup");
  expect(emptyState("video", nothingFits, [endpoint({ company: "openai" })])).toBe("addEndpoint");
  expect(emptyState("image", nothingFits, [openrouter])).toBeNull();                   // a cloud model to use
});
it("when no local model fits this computer, Studio offers cloud as the backup", async () => {   // cloudBackup
  await show({ endpoints: [], local: { ...LOCAL, installed: [], recommended: [], notHere: LOCAL.recommended, videoPossible: false } });
  screen.getByText("This computer cannot run local models well. Cloud models run on a company's computers instead: add OpenRouter or OpenAI.");
  screen.getByRole("button", { name: "Open Endpoints" });
  expect(screen.queryByRole("button", { name: "Get local models" })).toBeNull();
});
it("the first-run suggestion leads with local models and can be put away", async () => {
  await show({ local: { ...LOCAL, installed: [] }, prefs: prefs({ suggestionDismissed: false }) });
  screen.getByText(`Studio works on this computer. ${klein.name} fits here: free and private.`);
  fireEvent.click(screen.getByRole("button", { name: "Not now" }));
  expect(api.studioSetPrefs).toHaveBeenCalledWith({ suggestionDismissed: true });
});
it("the default model is an installed local model that fits", async () => {
  await show({ prefs: prefs({ lastModel: {} }) });
  expect(useStudio.getState().drafts.image).toMatchObject({ target: "thisComputer", model: "flux2-klein-4b-q4" });
});
it("the model you used last still wins, even a cloud one", async () => {
  await show({ prefs: prefs({ lastModel: { image: { target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" } } }) });
  expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" });
});
it("a slow local model is not picked by itself", () => {
  const slow = { ...LOCAL, installed: [{ ...LOCAL.installed[0], fit: { ...LOCAL.installed[0].fit, word: "slow" } }] };
  expect(defaultModel("image", slow, [openrouter])).toEqual({ target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" });
});
it("an endpoint without a saved list offers to get it", async () => {
  await show({ endpoints: [endpoint({ fetchedAt: null, models: [] })] });
  screen.getByText("Get OpenRouter's picture and video models");
  fireEvent.click(screen.getByRole("button", { name: "Get the list" }));
  expect(api.studioRefreshModels).toHaveBeenCalledWith(1);
});
it("the top bar shows Pictures and Video, where the chosen model runs, and cloud spending", async () => {
  await show({ usage: usage({ spentTodayUsd: 0.84 }) });                          // the default: a local model
  screen.getByRole("tab", { name: "Pictures" }); screen.getByRole("tab", { name: "Video" });
  screen.getByText("$0.84 today"); expect(document.querySelector("[data-local-mark]")).not.toBeNull();
  expect(document.querySelector("[data-cloud-mark]")).toBeNull();
  act(() => useStudio.setState((s) => ({ drafts: { ...s.drafts, image: { ...s.drafts.image, target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" } } })));
  expect(document.querySelector("[data-cloud-mark]")).not.toBeNull();
});
it("each tab keeps its own draft", async () => {
  await show(); typePrompt("lighthouse"); fireEvent.click(screen.getByRole("tab", { name: "Video" }));
  expect(promptBox().value).toBe(""); typePrompt("waves");
  fireEvent.click(screen.getByRole("tab", { name: "Pictures" })); expect(promptBox().value).toBe("lighthouse");
});
it("Update model lists refreshes every endpoint and says when", async () => {
  await show({ endpoints: [endpoint({ fetchedAt: Date.UTC(2026, 10, 17, 9) })] });
  fireEvent.click(screen.getByRole("button", { name: "Update model lists" }));
  screen.getByText("OpenRouter's lists updated 17 Nov"); expect(api.studioRefreshModels).toHaveBeenCalledWith(1);
});
it("the Video tab offers a local video model first, and OpenRouter as the backup", async () => {   // getLocal
  await show({ endpoints: [endpoint({ company: "openai" })], local: { ...LOCAL, installed: [] } });
  fireEvent.click(screen.getByRole("tab", { name: "Video" }));
  screen.getByText(`${LOCAL.recommended.find((e) => e.kind === "video")!.name} fits this computer.`);   // "Wan 2.2 5B, fast" unless the benchmark chose Wan 2.1
  screen.getByText("Or make videos with OpenRouter: add it in Settings → Endpoints.");
});
it("on a computer without a usable graphics card, the Video tab is cloud only", async () => {   // addEndpoint
  await show({ endpoints: [endpoint({ company: "openai" })], local: { ...LOCAL, videoPossible: false } });
  fireEvent.click(screen.getByRole("tab", { name: "Video" }));
  screen.getByText("Video on this computer needs a graphics card or an Apple-silicon Mac. OpenRouter makes videos in the cloud: add it in Settings → Endpoints.");
});
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/StudioView.test.tsx`
Expected: FAIL (`Failed to resolve import "./StudioView"`).

- [ ] **Step 3: Implement** the layout of Wireframe 1: top bar (title, `[Pictures | Video]` Radix tabs, search, Starred, Grid toggle, `LocalMark` for a local model or a `CloudMark`-style chip naming the chosen endpoint's host for a cloud one, "$X today" for cloud spending), the card stream in the middle (or *Get local models*, Task B13, when `localView`), the right panel slot (Task B5), the prompt dock slot (Task B7). Write `LocalMark.tsx` here: "On this computer" with a chip icon, `data-local-mark`, tooltip "Your prompt and pictures stay on this computer." `selectInitialModel` picks `prefs.lastModel[tab]` when that model is still offered, else `defaultModel`: an installed local model that fits, else the first usable cloud model of the first endpoint with a list (OpenAI's `gpt-image-2.6-flare` or OpenRouter's `openai/gpt-image-2.6-flare` for pictures, `google/veo-3.1-fast` for video, when present), never an "Auto" choice; it writes the draft directly and never asks for a price (Task B3's `chooseModel` is for the user's own pick). The empty states come from `emptyState`, local first; the first-run suggestion card shows once (until `suggestionDismissed`) when nothing is installed locally and a recommended entry fits; both name the recommended entry and its total size from `LOCAL.recommended`. The top bar's menu holds *Update model lists*, with each endpoint's "lists updated <date>", and *Get local models*.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio src/store/studio.ts src/lib/words.ts
git commit -m "Studio: the view's frame, local-first empty states and default, and the saved lists"
```

### Task B3: Model picker

**Files:**
- Create: `src/components/studio/ModelPicker.tsx`, `src/components/studio/ModelPicker.test.tsx`
- Modify: `src/store/studio.ts` (`chooseModel`, picture price on pick), `src/components/FitBadge.tsx` (`export function FitDot(props: { verdict: Verdict; word: string; line?: string })`, which `FitBadge` now renders through, its output unchanged), `src/lib/words.ts` (`LOCAL_FIT_WORDS`: fits "Fits", slow "Slow here", afterUnload "Fits after the chat model is unloaded", notHere "Not for this computer")

**Interfaces:**
- Produces:
```ts
export type PickerFilter = "edit" | "references" | "startFrame" | "endFrame" | "sound" | "transparent";
export interface PickerGroup { place: "local" | "cloud"; endpoint: StudioEndpoint | null; family: string; models: MediaModel[] }
/// "On this computer" first (installed local models of this kind), then "Recent" (cloud), then each endpoint by family.
export function pickerGroups(local: LocalModels | null, endpoints: StudioEndpoint[], kind: StudioTab, query: string,
  filters: PickerFilter[], recent: string[]): PickerGroup[];
export function priceHint(m: MediaModel, company: Company): string;  // "from $0.04 a picture", "$0.10–$0.40 a second",
                                                           // "price shown when picked" (OpenRouter picture, priceFetchedAt null), "price unknown"
export function localHint(m: InstalledLocal, kind: StudioTab): string;   // "about 25 s a picture" | "about 3 min for a 2 s clip" |
                                                                         // "time shown after your first run here"
export function ModelPicker(props: { tab: StudioTab }): JSX.Element;
// store: chooseModel(tab, target: RunTarget, modelId): void   // the user's pick: applies canonical.carry (Task B4) and asks studioModelPrice
//                                                             // once for an OpenRouter picture model whose priceFetchedAt is null; never for a local one
// store: fetchPrice(tab): Promise<void>                   // the dock's "Get the price" button for a restored model
```

- [ ] **Step 1: Write the failing tests**
```ts
it("On this computer comes first, then recent cloud models, then each endpoint by family", () => {
  const g = pickerGroups(LOCAL, [openrouter], "image", "", [], ["krea/k3-edit"]);
  expect(g[0]).toMatchObject({ place: "local", family: "On this computer", models: [expect.objectContaining({ id: "flux2-klein-4b-q4" })] });
  expect(g[1]).toMatchObject({ family: "Recent", models: [expect.objectContaining({ id: "krea/k3-edit" })] });
  expect(g.slice(2).map((x) => x.family)).toEqual(["openai/gpt-image", "google/gemini-nano-banana", "krea/k3-edit"]);  // a recent model stays in its family too
});
it("local rows show the fit word and the time here, with no Cloud mark and no price", () => {
  render(<ModelPicker tab="image" />);
  const row = screen.getByRole("option", { name: /FLUX.2 klein 4B/ });
  within(row).getByText("Fits"); within(row).getByText(localHint(LOCAL.installed[0], "image"));
  expect(row.querySelector("[data-cloud-mark]")).toBeNull(); expect(within(row).queryByText(/\$/)).toBeNull();
  screen.getByText("free · private · nothing is sent");                       // the group's tag
});
it("the last local row offers more models that fit", () => {
  render(<ModelPicker tab="image" />);
  fireEvent.click(screen.getByRole("button", { name: `+ Get local models (${LOCAL.moreFit} more fit this computer)` }));
  expect(useStudio.getState().localView).toBe(true);
});
it("with no local model yet, the group says which one fits", () => {
  useStudio.setState({ local: { ...LOCAL, installed: [] } }); render(<ModelPicker tab="image" />);
  screen.getByText(`No local models yet. ${klein.name} fits this computer (${amountGb(klein.sizeBytes)}).`);
  screen.getByRole("button", { name: "Get local models" });
});
it("picking a local model asks for no price", async () => {
  render(<ModelPicker tab="image" />); fireEvent.click(screen.getByRole("option", { name: /FLUX.2 klein 4B/ }));
  expect(api.studioModelPrice).not.toHaveBeenCalled();
  expect(useStudio.getState().drafts.image).toMatchObject({ target: "thisComputer", model: "flux2-klein-4b-q4" });
});
it("names where the prompt goes", () => {
  render(<ModelPicker tab="image" />);
  screen.getByText("via OpenRouter"); expect(screen.getByTitle("Your prompt and added pictures go to openrouter.ai.")).toBeTruthy();
});
it("greys out an endpoint without a key", () => {
  // hasKey false → option disabled with "Add a key in Settings → Endpoints"
});
it("greys out a video model that needs a video, and says why", () => {
  // { ...veo, id: "x/needs-video", displayName: "Needs Video", usable: "Needs a video to start from, which Studio does not offer yet." }
  expect(screen.getByRole("option", { name: /Needs Video/ })).toHaveAttribute("aria-disabled", "true");
  screen.getByText("Needs a video to start from, which Studio does not offer yet.");
});
it("filters by Edit, References, Start frame, End frame, Sound and Transparent", () => {
  const ids = (f: PickerFilter[], tab: StudioTab = "image") => pickerGroups(null, [openrouter], tab, "", f, []).flatMap((g) => g.models.map((m) => m.id));
  expect(pickerGroups(LOCAL, [], "image", "", ["edit"], [])[0].models.map((m) => m.id)).toEqual(["flux2-klein-4b-q4"]);   // klein takes references
  expect(ids(["transparent"])).toEqual(["openai/gpt-image-2.6-flare"]);
  expect(ids(["endFrame"], "video")).toEqual(["google/veo-3.1", "bytedance/seedance-2.5-flash"]);
  expect(ids(["sound"], "video")).toHaveLength(3);                     // every fixture video model makes sound
  expect(ids(["edit"])).toHaveLength(3);
});
it("a restored model fetches no price when Studio opens", async () => {
  // prefs.lastModel.image = OpenRouter "openai/gpt-image-2.6-flare" with priceFetchedAt null; mount StudioView
  expect(api.studioModelPrice).not.toHaveBeenCalled(); expect(api.studioRefreshModels).not.toHaveBeenCalled();
  screen.getByText("price shown when picked"); fireEvent.click(screen.getByRole("button", { name: "Get the price" }));
  expect(api.studioModelPrice).toHaveBeenCalledTimes(1);
});
it("picking an OpenRouter picture model asks for its price once", async () => {
  // before: "price shown when picked"; pick → api.studioModelPrice(1, "openai/gpt-image-2.6-flare") once; pick again → still once
});
it("never offers an Auto model", () => { expect(screen.queryByText(/auto/i)).toBeNull(); });
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/ModelPicker.test.tsx`
Expected: FAIL (module missing).

- [ ] **Step 3: Implement** as a Radix Popover with a search box, filter chips, groups ("On this computer" with the tag "free · private · nothing is sent" and `LocalMark`, then "Recent", then each endpoint with the Cloud mark, then each family), one line per model: name, badges from its params (Edit when it accepts pictures, Refs N, Start/End frame, Sound, Transparent, GIF, SVG), then for a local model the fit word through `FitDot` and `localHint`, for a cloud model the price hint, and the model notes in the tooltip. The local group ends with *+ Get local models (N more fit this computer)* when `local.moreFit > 0`; with nothing installed it shows the "No local models yet" line, or, when nothing fits here, "Local models need a graphics card or an Apple-silicon Mac; the cloud models below are the backup." `localHint` uses `fit.eta` rounded (`round_for_display` already applied by the backend) and the entry's default clip length for video. `filters`: `edit`/`references` = References field with max ≥ 1; `startFrame`/`endFrame` = those fields; `sound` = Sound field; `transparent` = `background` offers `transparent`.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio/ModelPicker.tsx src/components/studio/ModelPicker.test.tsx src/components/FitBadge.tsx src/store/studio.ts src/lib/words.ts
git commit -m "Studio: model picker with On this computer first, then cloud by endpoint and family"
```

### Task B4: Carrying settings between models

**Files:**
- Create: `src/components/studio/canonical.ts`, `src/components/studio/canonical.test.ts`

**Interfaces:**
- Produces:
```ts
export type Values = Record<string, unknown>;
export function defaultsFor(model: MediaModel): Values;               // the `initial` values only (video length and resolution)
export function carry(from: MediaModel | null, to: MediaModel, values: Values, extra: Values, refs: RefItem[]):
  { values: Values; extra: Values; refs: RefItem[]; note: string | null };
export function familyOf(id: string): string;                         // the same rule as Rust `family_of`
export function closestModel(models: MediaModel[], retired: { id: string; family: string }): MediaModel | null;  // same family, newest `created`
export function changedCount(model: MediaModel, values: Values, extra: Values): number;   // Advanced fields set, plus each extra key
```

- [ ] **Step 1: Write the failing tests** (models from `MODELS` in `fixtures/models.ts`)
```ts
it("names what changed, in one note", () => {
  const r = carry(k3Edit, nanoBanana, { aspect_ratio: "16:9", seed: 81723 }, {}, []);   // K3 Edit takes a seed; Nano Banana does not
  expect(r.values).toEqual({ aspect_ratio: "16:9" });
  expect(r.note).toBe("Nano Banana 2.1 has no seed setting, so the seed was removed.");
});
it("moves a shape to the nearest ratio", () => {
  expect(carry(a, withoutWide, { aspect_ratio: "21:9" }, {}, []).values.aspect_ratio).toBe("16:9");
  // note "Shape 21:9 became 16:9."
});
it("lowers how many to the new maximum", () => { /* n 4 → model without n: removed; max 2: 2, "How many 4 became 2." */ });
it("keeps the quality name or moves one step cheaper", () => {
  // "xhigh" → [low, medium, high] → "high" ("Quality xhigh became high."); "max" → "high"; "low" → "low"
});
it("moves a length to the nearest one, the shorter on a tie", () => { /* 7 → [4, 6, 8] → 6, "Length 7 s became 6 s." */ });
it("moves a resolution to the same or the next lower one", () => { /* "4K" → [720p, 1080p] → "1080p" */ });
it("keeps added pictures up to the new limit", () => {
  // 5 references → max 3 → 3, "Only 3 added pictures fit, so 2 were left out."
});
it("drops an extra field the new model does not accept", () => { /* "Veo 3.1 does not accept foo, so it was removed." */ });
it("defaults give a cheap first video", () => { expect(defaultsFor(veo)).toEqual({ duration: 4, resolution: "720p" }); });
it("counts Advanced changes", () => {
  expect(changedCount(flare, { output_format: "png", background: "transparent", quality: "high" }, {})).toBe(2);   // quality is simple
  expect(changedCount(flare, {}, { moderation: "low" })).toBe(1);
});
it("offers the newest model of the same family for a retired one", () => {
  const newer = { ...veo, id: "google/veo-3.2", created: veo.created! + 1 };
  expect(closestModel([veo, newer, grokLite], { id: "google/veo-3.0", family: "google/veo" })?.id).toBe("google/veo-3.2");
  expect(closestModel([grokLite], { id: "google/veo-3.0", family: "google/veo" })).toBeNull();
});
it("carries between a cloud model and a local one by role", () => {
  const local = LOCAL.installed[0].model;                                  // FLUX.2 klein 4B: shape, how many, references; seed
  const r = carry(flare, local, { aspect_ratio: "3:2", n: 4, quality: "high", seed: 81723 }, { moderation: "low" }, []);
  expect(r.values).toEqual({ aspect_ratio: "4:3", n: 4, seed: 81723 });
  expect(r.extra).toEqual({});
  expect(r.note).toBe("Shape 3:2 became 4:3. FLUX.2 klein 4B has no quality setting, so it was removed. FLUX.2 klein 4B does not accept moderation, so it was removed.");
});
it("agrees with the Rust family rule", () => {
  expect(familyOf("google/veo-3.1-fast")).toBe("google/veo"); expect(familyOf("kwaivgi/kling-v3.0-pro")).toBe("kwaivgi/kling");
});
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/canonical.test.ts`
Expected: FAIL (module missing).

- [ ] **Step 3: Implement.** Values are carried by `canonical` role, not by key, so `aspect_ratio` on one model fills the Shape field of another. Shape: nearest by `|ln(w/h) − ln(w'/h')|`; `auto` only when offered. Quality ladder: low < medium < high < xhigh < max (`auto` kept when offered). Resolution: compare by the number in the label (`4K` = 2160, `1K` = 1024). Anything else not offered is removed and named. Sentences joined with one space, in field order.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio/canonical.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio/canonical.ts src/components/studio/canonical.test.ts
git commit -m "Studio: carry settings to another model and say what changed"
```

### Task B5: Simple controls and added pictures

**Files:**
- Create: `src/components/studio/ParamField.tsx`, `SettingsPanel.tsx`, `RefTray.tsx` and their tests
- Modify: `src/store/studio.ts` (`setValue`, `resetValue`, `addRef`, `removeRef`, `setRefRole`), `src/lib/words.ts`

**Interfaces:**
- Produces:
```ts
export type Widget = "segmented" | "select" | "slider" | "switch" | "seed" | "drop" | "size" | "json";
export function widgetFor(spec: ParamSpec): Widget;     // enum ≤ 5 values: segmented; longer: select; range: slider + number box; …
export function visibleFields(params: ParamSpec[], layer: "simple" | "advanced", values: Values): ParamSpec[];   // applies `requires`
export function ParamField(props: { spec: ParamSpec; value: unknown; onChange(v: unknown): void; onReset(): void }): JSX.Element;
export function SettingsPanel(props: { tab: StudioTab }): JSX.Element;
export function RefTray(props: { tab: StudioTab }): JSX.Element;
// store: addRef(tab, ref: RefItem): string | null   (returns why it was not added)
```

- [ ] **Step 1: Write the failing tests**
```ts
it("draws each kind with its widget", () => {
  expect(widgetFor(spec({ kind: { type: "enum", values: choices(5) } }))).toBe("segmented");
  expect(widgetFor(spec({ kind: { type: "enum", values: choices(6) } }))).toBe("select");
  expect(widgetFor(spec({ kind: { type: "range", min: 0, max: 100 } }))).toBe("slider");
});
it("shows only the simple fields the model declares", () => { /* nano banana: no How many, no Quality; Resolution 1K 2K 4K */ });
it("shows five shapes and the rest under +", () => {
  renderPanel(flare);                                                  // 11 ratios
  expect(within(field("Shape")).getAllByRole("radio")).toHaveLength(5);
  fireEvent.click(within(field("Shape")).getByRole("button", { name: "More shapes" }));
  expect(screen.getAllByRole("menuitemradio")).toHaveLength(6);
});
it("How many offers 1, 2 and 4, and more under + up to the model's maximum", () => {
  renderPanel(flare);                                                  // n 1–10
  expect(within(field("How many")).getAllByRole("radio").map((r) => r.textContent)).toEqual(["1", "2", "4"]);
  fireEvent.click(within(field("How many")).getByRole("button", { name: "More" }));
  expect(screen.getByRole("spinbutton", { name: "How many" })).toHaveAttribute("max", "10");
});
it("starts a video at the shortest length of at least 4 s and 720p", () => { /* veo: [4 s] pressed, [720p] pressed */ });
it("a local video starts at 2 s and 480p, and each chip shows its own time", () => {
  // the local video model from LOCAL: [2 s] and [480p] pressed; the Length chips read "2 s · ~3 min", "5 s · ~9 min" from studioLocalFit,
  // called with { installed: { genModelId } } and the chip's size
});
it("the panel says where a local model runs", () => {
  renderPanel(LOCAL.installed[0].model, { target: "thisComputer" });
  screen.getByText(`On this computer · ${LOCAL.hardware.device}`); expect(screen.queryByText(/via OpenRouter/)).toBeNull();
});
it("hides compression unless the file type is JPEG or WebP", () => {
  renderFields(flare, "advanced", { output_format: "png" }); expect(screen.queryByText("Compression")).toBeNull();   // ParamFields through visibleFields()
  setValue("output_format", "webp"); screen.getByText("Compression");
});
it("choosing a transparent background switches to PNG and says so", () => {
  // output_format "jpeg" → background "transparent" → output_format "png", note "Transparent needs PNG or WebP, so the file type is now PNG."
});
it("the tray refuses a picture past the model's limit", () => { /* max 1: second add → "This model takes 1 added picture." */ });
it("a start frame drop zone appears only when the model takes one", () => {
  renderPanel(veo); screen.getByText("Start frame");
  renderPanel({ ...veo, params: veo.params.filter((p) => p.key !== "first_frame") }); expect(screen.queryByText("Start frame")).toBeNull();
});
it("an added picture goes to studioAddRef and shows a role chip", async () => {
  api.studioAddRef.mockResolvedValue({ sha256: "a".repeat(64), mime: "image/png", sizeBytes: 1200, role: null });
  dropFile(screen.getByText("+ Add picture"), pngFile());
  await waitFor(() => expect(api.studioAddRef).toHaveBeenCalledTimes(1));
  screen.getByRole("button", { name: "Reference" });                  // the role chip
});
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/ParamField.test.tsx src/components/studio/SettingsPanel.test.tsx src/components/studio/RefTray.test.tsx`
Expected: FAIL (modules missing).

- [ ] **Step 3: Implement.** Every label reads "Automatic (company default)" until set ("Automatic" for a local model); an unset field is not sent. Under the Model field a local model shows "On this computer · <hardware.device>" where a cloud model shows "via <company> · Cloud". For a local video, each Length and Resolution chip asks `studioLocalFit({ installed: { genModelId: model.id } }, w, h, frames)` once per value when the panel opens (no network, for a catalog model and a searched one alike; the store caches the answers per model) and shows "<value> · ~<time>". The tray accepts drop, paste and a file (through `readImage` from `Composer.tsx`, then `studioAddRef`); role chips Reference / Start frame / End frame follow the model's Images fields. The panel holds Model (Task B3), the simple fields in spec order, My setups (Task B6) and the Advanced toggle with its change count.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio src/store/studio.ts src/lib/words.ts
git commit -m "Studio: controls drawn from the model's own settings, and added pictures"
```

### Task B6: Advanced drawer, chips, Extra fields, request preview, My setups

**Files:**
- Create: `src/components/studio/AdvancedDrawer.tsx`, `AdvancedDrawer.test.tsx`
- Modify: `src/components/studio/SettingsPanel.tsx` (My setups), `src/store/studio.ts` (`saveSetup`, `applySetup`, `newSeed`, `lockSeed`), `src/lib/words.ts`

**Interfaces:**
- Produces:
```ts
export function chipsFor(model: MediaModel, values: Values, extra: Values): { key: string; text: string }[];   // "seed 81723", "background: transparent"
export function parseExtra(text: string, allowed: string[], typed: string[]): { ok: true; value: Values } | { ok: false; message: string };
export function AdvancedDrawer(props: { tab: StudioTab }): JSX.Element;
```

- [ ] **Step 1: Write the failing tests**
```ts
it("counts changed fields in the header", () => {
  renderDrawer(flare, { output_format: "png", background: "transparent" }); screen.getByText("Advanced · 2 changed");
});
it("each changed field shows as a chip that removes it", () => {
  expect(chipsFor(flare, { background: "transparent" }, { moderation: "low" })).toEqual(
    [{ key: "background", text: "background: transparent" }, { key: "moderation", text: "moderation: low" }]);
  fireEvent.click(screen.getByRole("button", { name: "Remove background: transparent" }));
  expect(useStudio.getState().drafts.image.values.background).toBeUndefined();
});
it("Reset all and each field's reset go back to Automatic", () => {
  fireEvent.click(screen.getByRole("button", { name: "Reset Background" })); screen.getByText("Automatic (company default)");
  fireEvent.click(screen.getByRole("button", { name: "Reset all" })); screen.getByText("Advanced");   // no "changed" count left
});
it("Extra fields accept only the names the model lists", () => {
  expect(parseExtra('{"moderation":"low"}', ["moderation"], [])).toEqual({ ok: true, value: { moderation: "low" } });
  expect(parseExtra('{"foo":1}', ["moderation"], [])).toEqual({ ok: false, message: "This model does not accept foo." });
  expect(parseExtra("{nope", ["moderation"], [])).toEqual({ ok: false, message: "This is not valid JSON." });
  expect(parseExtra('{"duration":4}', ["duration"], ["duration"])).toEqual({ ok: false, message: "Set duration with its own field above." });
});
it("lists the accepted names as hints", () => {
  renderDrawer(flare); screen.getByText("This model accepts: moderation, complexity");   // from flare's saved /endpoints answer
  renderDrawer({ ...flare, params: flare.params.filter((p) => p.key !== "extra"), priceFetchedAt: null });
  screen.getByText("Pick the model to see its extra settings.");
});
it("See the exact request shows where it goes, without the key", async () => {
  // api.studioPreview resolves {host:"openrouter.ai", body:{…}} → "goes to openrouter.ai" and "key removed"
});
it("the dice gives a new seed and Keep it locks it", () => {
  renderDrawer(k3Edit, { seed: 81723 }); fireEvent.click(screen.getByRole("button", { name: "New seed" }));
  expect(draftSeed()).not.toBe(81723);
  fireEvent.click(screen.getByRole("checkbox", { name: "Keep it" })); expect(useStudio.getState().drafts.image.seedLocked).toBe(true);
});
it("a setup brings back where it runs, the model and every setting", () => {
  useStudio.getState().applySetup({ name: "Product shot", kind: "image", target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare",
    params: { quality: "high", background: "transparent" }, extra: {}, routing: null });
  expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare",
    values: { quality: "high", background: "transparent" } });
  useStudio.getState().applySetup({ name: "Local square", kind: "image", target: "thisComputer", model: "flux2-klein-4b-q4",
    params: { aspect_ratio: "1:1", steps: 4 }, extra: {}, routing: null });
  expect(useStudio.getState().drafts.image).toMatchObject({ target: "thisComputer", model: "flux2-klein-4b-q4", values: { aspect_ratio: "1:1", steps: 4 } });
});
it("a routing choice is offered for OpenRouter pictures only", () => {
  renderDrawer(flare); screen.getByText("Company that runs it");
  renderDrawer(veo); expect(screen.queryByText("Company that runs it")).toBeNull();
});
it("a local model offers steps, memory saving and where it runs, and no Extra fields", () => {
  renderDrawer(LOCAL.installed[0].model, {}, { target: "thisComputer" });
  screen.getByText("Steps"); screen.getByText("Memory saving"); screen.getByText("Run on");
  expect(screen.queryByText(/Extra fields/)).toBeNull(); expect(screen.queryByText("Company that runs it")).toBeNull();
});
it("See the exact command shows the engine's arguments for a local model", async () => {
  // api.studioPreview resolves {method:"run", host:"", body:{argv:["--diffusion-model","/m/…","--prompt-file","/g/.partial/r/prompt.txt", …],
  //   prompt:"a lighthouse"}}
  fireEvent.click(screen.getByRole("button", { name: "See the exact command" }));
  await screen.findByText("--prompt-file"); screen.getByText("Prompt (passed in prompt.txt): a lighthouse");
  screen.getByText("Runs on this computer; nothing is sent.");
});
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/AdvancedDrawer.test.tsx`
Expected: FAIL (module missing).

- [ ] **Step 3: Implement** Wireframe 2: Advanced fields through `ParamField`, each with a reset; seed with Dice and Keep it and the help "A seed is not a saved copy; the same seed on another model or version gives a different picture."; the OpenRouter section (company that runs it from `MediaModelDto.companies` by `name`, sent as its `tag`; *Allow another company if that one is busy*; *Only companies that keep no data*) for pictures; Extra fields (JSON) with hints; *See the exact request* (read-only JSON, "key removed · goes to <host>"). For a local model the OpenRouter section and Extra fields are absent (the model lists no `extra` field), and the button is *See the exact command* (the `argv` list, one argument a line, then "Prompt (passed in prompt.txt): <prompt>" and the negative prompt when there is one, with "Runs on this computer; nothing is sent."). `applySetup` sets the draft's `target`, `model`, values, extra and routing, through `carry` when the model is not the draft's. `prefs.advancedOpen` remembers the drawer. My setups: a select plus *Save as setup…* (name), at most 50.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio src/store/studio.ts src/lib/words.ts
git commit -m "Studio: every setting one click away, shown as chips, with the exact request"
```

### Task B7: Time or price on Create, where the prompt goes, the confirm dialog, the daily limit and cloud as the backup

**Files:**
- Create: `src/components/studio/cost.ts`, `cost.test.ts`, `PromptDock.tsx`, `PromptDock.test.tsx`, `ConfirmRun.tsx`, `ConfirmRun.test.tsx`
- Modify: `src/store/studio.ts` (`estimate` debounced 150 ms, `start`), `src/lib/words.ts`

**Interfaces:**
- Produces:
```ts
export function formatUsd(usd: number): string;                         // "$0.16"; below $0.01 "< $0.01"
export function costTone(usd: number | null): "plain" | "amber" | "red" | "unknown";
export function createLabel(estimate: Estimate | null): string;         // "~ $0.16" | "price unknown"; local: "about 4 min" | "free"
export function placeLine(target: RunTarget, host: string, estimate: Estimate | null): string;
  // "On this computer: your prompt and pictures stay here" (+ " · the chat model will be unloaded to make room" when local.unloadChat)
  // | "Cloud: your prompt and added pictures go to <host>"
export function cloudBackup(estimate: Estimate | null, endpoints: StudioEndpoint[]): "useCloud" | "addEndpoint" | null;
  // a local run that does not fit here or is very slow: offer a cloud model when an endpoint can make this kind, else Settings → Endpoints
export function PromptDock(props: { tab: StudioTab }): JSX.Element;
export function ConfirmRun(props: { open: boolean; tab: StudioTab; onCancel(): void; onConfirm(dontAskVideos: boolean): void }): JSX.Element;
// store: estimate(tab): Promise<void>; start(tab, confirmed: boolean): Promise<Run | null>; starting: boolean
```

- [ ] **Step 1: Write the failing tests**
```ts
// cost.test.ts
it("formats and colours prices", () => {
  expect(formatUsd(0.16)).toBe("$0.16"); expect(formatUsd(0.004)).toBe("< $0.01");
  expect(costTone(0.99)).toBe("plain"); expect(costTone(1)).toBe("amber"); expect(costTone(2.5)).toBe("amber");
  expect(costTone(2.51)).toBe("red"); expect(costTone(null)).toBe("unknown");
  expect(createLabel(est({ usd: null }))).toBe("price unknown");
});
// PromptDock.test.tsx
it("Create shows the price for the whole run", () => { screen.getByRole("button", { name: /Create/ }); screen.getByText("~ $0.16"); });
it("names where the prompt goes", () => { screen.getByText("Cloud: your prompt and added pictures go to openrouter.ai"); });
it("Enter creates and Shift+Enter adds a line", () => {
  typePrompt("a"); fireEvent.keyDown(promptBox(), { key: "Enter", shiftKey: true });
  expect(api.studioStart).not.toHaveBeenCalled();
  fireEvent.keyDown(promptBox(), { key: "Enter" }); expect(api.studioStart).toHaveBeenCalledTimes(1);
});
it("Create is disabled while a start is on its way", async () => {
  // api.studioStart pending; click Create twice and press Enter → studioStart called once
});
it("a local model's Create shows the time, not a price", () => {
  // estimate { usd: 0, reasons: [], eta: { lowMs: 180000, highMs: 300000, measured: true }, local: { fit: "fits", .. } }
  screen.getByText("about 4 min"); expect(screen.queryByText(/\$/)).toBeNull();
  expect(createLabel(est({ usd: 0, eta: null, local: localEst({}) }))).toBe("free");
});
it("the line above Create says the prompt stays here, and when the chat model will be unloaded", () => {
  screen.getByText("On this computer: your prompt and pictures stay here · the chat model will be unloaded to make room");   // local.unloadChat
  expect(document.querySelector("[data-cloud-mark]")).toBeNull();
});
it("a local run never opens the confirm dialog, even for a video", async () => {
  // local video draft, estimate.reasons [] → click Create → api.studioStart(req with target "thisComputer", false) at once; no dialog
});
it("a run over 15 minutes says so in amber, without a dialog", () => {
  // local.over15Min true, eta 25 min → "This will take about 25 min and keep the graphics card busy. On a MacBook it will get warm."
});
it("when a local run would be very slow, Studio offers cloud as the backup", () => {
  // local.verySlow true, an OpenRouter endpoint → "Too slow here? A cloud model makes this in minutes, for a price you see first."
  fireEvent.click(screen.getByRole("button", { name: "Choose a cloud model" }));   // opens the picker on the first cloud group
  // no endpoint able to make this kind → "Add OpenRouter or OpenAI in Settings → Endpoints to use a cloud model instead." + "Open Endpoints"
});
it("a local model that does not fit here offers cloud instead of Create", () => {
  // local.fit "notHere" → Create disabled with its reason; the cloud backup line as above
});
it("a local model that fits once the chat model is unloaded can be created", () => {
  // local { fit: "afterUnload", unloadChat: true } → Create enabled; the place line ends " · the chat model will be unloaded to make room"
});
it("a local prompt over 4,000 characters says so before Create", () => {
  // estimate.invalid ["Keep the prompt under 4,000 characters for local models."] → that line under the prompt; Create disabled
});
it("the daily limit never replaces Create for a local model", () => {
  // limit reached (spentTodayUsd 10, limitUsd 10) and a local model → Create enabled; "Daily limit reached" absent
});
it("the daily limit replaces Create", () => {
  // estimate.limit = {limitUsd:10, spentTodayUsd:9.5, wouldPass:true}
  screen.getByText("Daily limit reached: $9.50 of $10 spent today");
  fireEvent.click(screen.getByRole("button", { name: "Raise the limit" }));     // app:open-settings "studio"
  expect(screen.queryByRole("button", { name: /Create/ })).toBeNull();
});
// ConfirmRun.test.tsx
it.each([["a video", "video", 0.09, ["video"]], ["a $1.00 picture run", "image", 1, ["atLeast"]],
         ["an unknown price", "image", null, ["unknownPrice"]]])("asks before %s", () => { /* dialog shown */ });
it("does not ask for a $0.99 picture run", () => { /* studioStart(req, false) at once */ });
it("says a started video is charged even if you stop watching", () => {
  screen.getByText("Make this video?");
  screen.getByText("OpenRouter charges for a video once it has started, even if you stop watching it.");
});
it("Don't ask again for videos under $1.00 saves the choice and starts", async () => {
  // → api.studioSetPrefs({ confirmEveryVideo: false }) (only that field) then api.studioStart(req, true)
});
it("Cancel sends nothing", () => { expect(api.studioStart).not.toHaveBeenCalled(); });
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/cost.test.ts src/components/studio/PromptDock.test.tsx src/components/studio/ConfirmRun.test.tsx`
Expected: FAIL (modules missing).

- [ ] **Step 3: Implement.** The dock: *+ Add picture*, the prompt box, the chips row (Task B6), the place line (`placeLine`: the local sentence for a local model, the Cloud line for a cloud one), the price in text with `costTone` colour as a second cue for a cloud model or the time for a local one, and Create. Each `estimate.invalid` sentence shows under the prompt and disables Create (for a local model, the 4,000-character limit). For a local model: no confirmation (the backend never asks; a `studio_needs_confirmation` cannot come back for it), `notHere` disables Create while `afterUnload` does not, the daily limit is not shown, the amber 15-minute line when `local.over15Min`, and the cloud backup when `cloudBackup(..)` is not null (it never switches the draft by itself; *Choose a cloud model* opens the picker). The store's `start` makes one `clientToken` (`crypto.randomUUID()`) per press, sets `starting`, calls `studioStart(req, false)` unless `estimate.reasons` is not empty (then `ConfirmRun`), and on `studio_needs_confirmation` from the backend opens the dialog anyway. The dialog lists model, company, length, size, sound, count, the estimate with "(<company>'s price list, <date of fetchedAt>)", where the data goes, and for video the charge sentence; the checkbox ("Don't ask again for videos under $<estimate.confirmAtUsd>") appears only when the only reason is `video`.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio src/store/studio.ts src/lib/words.ts
git commit -m "Studio: time or price on Create, where the prompt goes, confirmation and the limit for cloud, and cloud as the backup"
```

### Task B8: Run cards and progress

**Files:**
- Create: `src/components/studio/RunCard.tsx`, `RunCard.test.tsx`, `src/components/studio/StudioNotice.tsx`, `StudioNotice.test.tsx`, `src/components/studio/QuitWhileMaking.tsx`, `QuitWhileMaking.test.tsx`
- Modify: `src/components/studio/StudioView.tsx` (queue strip, card stream), `src/App.tsx` (render `StudioNotice` in chat view and `QuitWhileMaking` everywhere), `src/store/studio.ts` (`cancel`, `fetchAnyway`, `answerUnload`, `downloadEngine`, `repairLocal`), `src/components/RuntimeIndicator.tsx` and `RuntimeIndicator.test.tsx` (the wait for Studio), `src/lib/types.ts` (`RuntimeStatus.waitingForStudio: boolean`), `src/lib/words.ts`

**Interfaces:**
- Produces:
```ts
export function RunCard(props: { run: Run }): JSX.Element;
export function usualSeconds(runs: Run[], model: string): number | null;   // median of the last 10 finished runs of that model
export function queueSummary(runs: Run[]): string;                         // "1 making · 1 waiting · 3 done · 1 stopped"
export function StudioNotice(): JSX.Element | null;
export function localLine(run: Run, now: number): string;                  // "Picture 1 of 2 · step 3 of 4 · 0:21 so far · about 0:30 left"
export function QuitWhileMaking(): JSX.Element | null;                     // on "studio-quit-requested"
```

- [ ] **Step 1: Write the failing tests**
```ts
it("a picture card shows making it, a timer and Cancel", () => { screen.getByText(/Making it · 0:0\d/); screen.getByRole("button", { name: "Cancel" }); });
it("a stopped picture shows the backend's charge line", () => {
  // run({ status: "cancelled", error: err({ code: "studio_stopped", charged: null, message: "Stopped waiting. OpenAI may still charge for it." }) })
  screen.getByText("Stopped waiting. OpenAI may still charge for it."); screen.getByText("May have been charged");
  // the OpenRouter case: charged false, "Stopped. OpenRouter does not charge for a picture you did not receive." and "Not charged"
});
it("a video card shows time so far and the usual time", () => {
  screen.getByText("Making it · 1:12 so far · usually about 2 min");
  screen.getByText("You can keep working or close the app; it will appear here.");
});
it("Stop watching keeps a way to get the video", async () => {
  // cancel → card "OpenRouter may still finish and charge for this." + "Get it anyway" → api.studioFetchAnyway(id)
});
it("a still-not-ready video offers Check again", () => {
  render(<RunCard run={run({ kind: "video", status: "waiting", watching: false, stale: true })} />);
  screen.getByText("Still not ready."); fireEvent.click(screen.getByRole("button", { name: "Check again" }));
  expect(api.studioFetchAnyway).toHaveBeenCalledWith(RUN.id); expect(api.studioStart).not.toHaveBeenCalled();
});
it("a refused run explains it and offers no retry", () => {
  // error {code:"studio_blocked", message:"OpenAI's safety filter refused the prompt.", charged:false, requestId:"req_8f2"}
  screen.getByText("OpenAI's safety filter refused the prompt."); screen.getByText("Not charged · request req_8f2");
  screen.getByRole("button", { name: "Edit prompt" }); expect(screen.queryByRole("button", { name: /try again/i })).toBeNull();
});
it("an interrupted run says it may or may not have started", () => {
  render(<RunCard run={run({ status: "interrupted", error: err({ code: "studio_interrupted", charged: null,
    message: "The app closed while sending; it may or may not have started." }) })} />);
  screen.getByText("The app closed while sending; it may or may not have started."); screen.getByText("May have been charged");
  screen.getByText("Check your activity page on openrouter.ai.");
});
it("progress events update the card", () => { /* applyProgress → progress bar 40% */ });
it("the queue strip counts", () => { expect(queueSummary(runs)).toBe("1 making · 1 waiting · 3 done · 1 stopped"); });
it("a video finished while in chat says so, with a way to open it", () => {
  // readyNotice "v" → "Your video is ready" + "Open" → view studio, viewer on its first output
});
it("Esc does not cancel a running card", () => { fireEvent.keyDown(window, { key: "Escape" }); expect(api.studioCancel).not.toHaveBeenCalled(); });
it("a local card shows the stage, the step and the time left", () => {
  // LOCAL_RUN running, local {stage:"drawing", step:3, steps:4, picture:1, pictures:2}, eta.leftMs 30000, started 21 s ago
  screen.getByText("Picture 1 of 2 · step 3 of 4 · 0:21 so far · about 0:30 left"); screen.getByRole("button", { name: "Stop" });
  screen.getByText(/on this computer/); screen.getByText("free"); expect(document.querySelector("[data-cloud-mark]")).toBeNull();
});
it("waiting for the graphics card says why, and asks when the setting says Ask", () => {
  // waitingFor "reply" → "Waiting for the graphics card: a chat reply is being written. Starts when it ends." + "Cancel"
  // waitingFor "answer" → "This needs the graphics card's memory. Unload the chat model?" [Unload] → api.studioAnswerUnload(id, true)
});
it("the unloaded chat model can be loaded again from the card", () => {
  // local.unloadedChat "/m/chat.gguf" → "The chat model was unloaded to make room." + "Load it again" → useApp.pickModel("local:/m/chat.gguf")
});
it("Stop on a local card says nothing was charged and offers Run again", () => {
  // cancelled, error.message "Stopped. Your computer is no longer working on it. Nothing was charged." → that text, "Run again" (again(run))
});
it("a local run beside the chat model, or on the processor, says what that means", () => {
  // local.alongside → "Chat replies may be slower while this runs"; local.onProcessor → "Running on the processor: slower, and chat will be slow while it runs."
});
it("a missing engine or a damaged part offers the download again", () => {
  // failed studio_engine_missing, useStudio.local.engine.state "notInstalled" → "Download the Studio engine (29 MB, MIT)" → api.studioEngineDownload(true)
  // failed studio_model_file_bad → "Download again" → api.studioLocalDownload({ installed: { genModelId: run.model } }, [])
});
it("an out-of-memory retry and failure are said plainly", () => {
  // local.retrying → "Ran out of graphics memory, trying again with memory saving (slower)";
  // failed studio_out_of_memory → "Ran out of graphics memory." and "Try 480p or a shorter clip"
});
it("a stalled local run offers Stop and Keep waiting", () => { /* local.stalled → "No progress for 10 minutes" + "Stop" + "Keep waiting" (hides the line) */ });
it("an interrupted local run offers Run again for free", () => { /* "The app closed while this was being made." + "Run again"; no charge line */ });
it("quitting while a local job works asks first", async () => {
  // onQuitRequested handler fires → "A video is being made on this computer. Quitting stops it." [Keep working] [Quit and stop]
  fireEvent.click(screen.getByRole("button", { name: "Quit and stop" })); expect(api.studioQuitNow).toHaveBeenCalledTimes(1);
});
// RuntimeIndicator.test.tsx
it("a chat load waiting for Studio says so and can stop the Studio job", () => {
  // runtime.waitingForStudio true, a running local run in useStudio.live →
  // "Studio is using the graphics card. The chat model loads when it finishes." + "Stop Studio job" → studioCancel(that run's id)
});
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/RunCard.test.tsx src/components/studio/StudioNotice.test.tsx src/components/studio/QuitWhileMaking.test.tsx src/components/RuntimeIndicator.test.tsx`
Expected: FAIL (modules missing).

- [ ] **Step 3: Implement** Wireframe 3's card: prompt, the settings line ("Veo 3.1 Fast · OpenRouter · 8 s · 16:9 · 720p · sound on · start frame"; for a local run "FLUX.2 klein 4B · on this computer · 1024×1024 · ×2 · seed 81723 · free"), estimate then actual cost (cloud) or the time (local), status line (`localLine` for a local run, Wireframe L4), results row (Task B9 tiles), actions (Again, Reuse, Star, …; Stop on a running local card). The charge line reads "Not charged", "May have been charged" or "Charged $0.04". Parts of the strip with 0 are left out; failed and blocked runs count as "failed". A local card adds "Chat replies may be slower while this runs" (`local.alongside`) or the processor line (`local.onProcessor`); a failed `studio_engine_missing` card offers the engine's download when `local.engine.state` is `notInstalled` or `partial` (`downloadEngine` = `studioEngineDownload(true)`, the licence having been shown with the first model), and a `studio_model_file_bad` card offers *Download again* (`repairLocal(run.model)` = `studioLocalDownload({ installed: { genModelId } }, [])`: the parts are checked and what is missing or damaged is fetched again).

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio src/App.test.tsx src/components/RuntimeIndicator.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio src/App.tsx src/store/studio.ts src/components/RuntimeIndicator.tsx src/components/RuntimeIndicator.test.tsx src/lib/types.ts src/lib/words.ts
git commit -m "Studio: run cards with progress, honest charge lines, local stages and Stop, and the quit question"
```

### Task B9: Gallery, viewer and actions

**Files:**
- Create: `src/components/studio/Grid.tsx`, `Grid.test.tsx`, `Viewer.tsx`, `Viewer.test.tsx`
- Modify: `src/components/studio/RunCard.tsx` (tiles, actions), `src/store/studio.ts` (`again`, `reuse`, `animate`, `editWithWords`, `useAsReference`, `star`, `remove`, `saveSeveral`, `openViewer`, filters), `src/components/ShortcutsDialog.tsx` (viewer keys), `src/lib/words.ts`

**Interfaces:**
- Produces:
```ts
export function useOutputUrl(outputId: string, thumb: boolean): string | null;   // blob: URL from studioOutputBytes, revoked on unmount
export function Grid(props: { onOpen(outputId: string): void }): JSX.Element;
export function Viewer(props: { outputId: string; onClose(): void }): JSX.Element;
// store
again(run: Run): Promise<void>;               // same settings, new seed unless locked; goes through start (confirm rules apply)
reuse(run: Run, withSeed: boolean): void;     // fills the draft; never starts
animate(output: Output): void; editWithWords(output: Output): void; useAsReference(output: Output): Promise<void>;
saveSeveral(outputIds: string[]): Promise<void>;   // folder picker, then studioExport per output (original names, settings off)
```

- [ ] **Step 1: Write the failing tests**
```ts
it("arrow keys move, F stars, R reuses all, Shift+R reuses without seed, Esc closes the viewer only", () => {
  fireEvent.keyDown(window, { key: "ArrowRight" }); screen.getByText("2 / 4");
  fireEvent.keyDown(window, { key: "f" }); expect(api.studioStar).toHaveBeenCalledWith("o2", true);
  fireEvent.keyDown(window, { key: "R", shiftKey: true }); expect(useStudio.getState().drafts.image.values.seed).not.toBe(81723);
  fireEvent.keyDown(window, { key: "Escape" }); expect(onClose).toHaveBeenCalled(); expect(api.studioCancel).not.toHaveBeenCalled();
});
it("shows every setting, the cost, the time taken, the file and where it ran", () => {
  screen.getByText("$0.04 · took 13.8 s");
  screen.getByText("OpenRouter (runs at OpenAI)");                     // RUN.upstream "OpenAI"; plain "OpenRouter" when null
});
it("a local result says it was free, how long it took, where it ran and that the file is labelled", () => {
  // LOCAL_RUN: elapsedMs 228000, device "Radeon RX 7600 (Vulkan)", engineBuild 1106, labelled
  screen.getByText("Cost: free · took 3 min 48 s · on Radeon RX 7600 (Vulkan) · engine build 1106");
  screen.getByText("Labelled as AI-made in the file"); screen.getByRole("button", { name: "See the exact command" });
  expect(document.querySelector("[data-cloud-mark]")).toBeNull();
});
it("Delete says what the audit log keeps", async () => {
  screen.getByText("The audit log keeps a line saying a run happened (model and cost), but not your prompt. It still counts toward today's spending.");
});
it("Save as leaves the settings file off unless asked", async () => {
  // saveDialog mocked → path; api.studioExport("o1", path, false)
});
it("Reuse all fills the prompt and every setting and starts nothing", () => {
  fireEvent.click(screen.getByRole("button", { name: "Reuse all" }));
  expect(api.studioStart).not.toHaveBeenCalled();
  expect(useStudio.getState().drafts.image).toMatchObject({ prompt: RUN.prompt, values: RUN.params, extra: RUN.extra });
});
it("Again pressed twice while starting starts once", async () => {
  api.studioStart.mockReturnValue(new Promise(() => {}));           // still on its way
  fireEvent.click(screen.getByRole("button", { name: "Again" })); fireEvent.click(screen.getByRole("button", { name: "Again" }));
  expect(api.studioStart).toHaveBeenCalledTimes(1);
});
it("Reuse on a retired model picks the closest one and says what changed", () => {
  // none in the family → "OpenRouter no longer offers this model."
});
it("Animate switches to Video with the picture as the start frame", async () => {
  // video draft on noFrames = { ...veo, id: "test/no-frames", displayName: "No Frames", params without first_frame/last_frame };
  // recently used: grok-imagine-video-1.5-lite → "No Frames takes no start frame. Use Grok Imagine Video 1.5 Lite instead?"
  fireEvent.click(screen.getByRole("button", { name: "Use Grok Imagine Video 1.5 Lite" }));
  expect(useStudio.getState()).toMatchObject({ tab: "video", drafts: { video: { model: "x-ai/grok-imagine-video-1.5-lite" } } });
  expect(useStudio.getState().drafts.video.refs[0].role).toBe("startFrame");
});
it("Edit puts the picture in the tray and focuses the prompt", () => {
  fireEvent.click(screen.getByRole("button", { name: "Edit" }));
  expect(useStudio.getState().drafts.image.refs.map((r) => r.role)).toEqual(["reference"]);
  expect(document.activeElement).toBe(promptBox());
});
it("Copy falls back to Save as when the clipboard refuses", async () => {
  // navigator.clipboard.write rejects → "Copy is not available here. Use Save as… instead."
});
it("the grid filters by Pictures, Video, Starred, date and search", async () => {
  fireEvent.click(screen.getByRole("button", { name: "Video" })); typeSearch("waves"); pickDate("Last 7 days");
  expect(api.studioRuns).toHaveBeenLastCalledWith(expect.objectContaining({ kind: "video", text: "waves", since: expect.any(Number) }), null, 60);
});
it("several can be selected and deleted together", async () => {
  selectTiles("o1", "o2"); fireEvent.click(screen.getByRole("button", { name: "Delete 2" })); confirmDialog();
  expect(api.studioDelete).toHaveBeenCalledWith(["o1", "o2"]);
});
it("several can be saved together into one folder", async () => {
  selectTiles("o1", "o2"); dialogOpen.mockResolvedValue("/tmp/out"); fireEvent.click(screen.getByRole("button", { name: "Save 2…" }));
  await waitFor(() => expect(api.studioExport).toHaveBeenCalledTimes(2));
  expect(api.studioExport).toHaveBeenCalledWith("o1", expect.stringMatching(/^\/tmp\/out\//), false);
});
it("an SVG result is shown with an img element only", () => { expect(container.querySelector("object, iframe, embed")).toBeNull(); });
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/Grid.test.tsx src/components/studio/Viewer.test.tsx`
Expected: FAIL (modules missing).

- [ ] **Step 3: Implement** Wireframe 4 (for a local run: "On this computer" in place of the company, "Cost: free · took … · on <device> · engine build <n>", the label note, and *See the exact command*). Animate offers the installed local video model that takes a start frame before a cloud one; Edit offers the local model that takes references before a cloud one. Tiles use the thumbnail when `hasThumb`, else the original (GIF, SVG), else for video the length and the start frame. Viewer notes come from the model's notes (for example "Carries Google's SynthID watermark"). Copy draws non-PNG pictures to a canvas and writes `image/png` with `navigator.clipboard.write`. Viewer keys: ←/→, F, R, Shift R, A, S, C, Del, Space, Esc.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio src/store/studio.ts src/components/ShortcutsDialog.tsx src/lib/words.ts
git commit -m "Studio: gallery grid, full-size viewer and the actions on each result"
```

### Task B10: Settings → Studio and the endpoint badges

**Files:**
- Create: `src/components/settings/StudioSettings.tsx`, `StudioSettings.test.tsx`
- Modify: `src/components/SettingsDialog.tsx` (tab `studio` after Endpoints), `src/components/settings/endpointPresets.ts` (`media?: { image: boolean; video: boolean }`), `src/components/settings/Endpoints.tsx`, `Endpoints.test.tsx`, `e2e/m8_offline.py` (`"Studio"` after `"Endpoints"` in `SETTINGS_PAGES`), `src/lib/words.ts`

**Interfaces:**
- Produces: `export function StudioSettings(): JSX.Element;`, `export function abilitiesOf(kind: ProviderKind): string;` ("Chat · Pictures · Video").

- [ ] **Step 1: Write the failing tests**
```ts
it("says how much Studio holds and where", async () => {
  screen.getByText("Studio uses 1.2 GB in ~/harnessUI/studio (312 pictures, 14 videos)");
  screen.getByRole("button", { name: "Open folder" });
});
it("opening the page sends nothing", async () => {
  expect(api.studioRefreshModels).not.toHaveBeenCalled(); expect(api.studioLocalSearch).not.toHaveBeenCalled();
});
it("the local block comes first: engine, models on this computer, and the chat model choice", async () => {
  // order of headings: "On this computer" before "Cloud spending"
  screen.getByText(`Picture and video models: 1 (${amountGb(LOCAL.installedBytes)})`); screen.getByRole("button", { name: "Get local models" });
  screen.getByText("Studio engine: built in");                       // LOCAL.engine.state "builtIn"; Windows: "installed · build master-1106-532d2c1 · Remove"
  fireEvent.click(screen.getByRole("radio", { name: "Ask" }));
  expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ gpuPolicy: "ask" });
  fireEvent.click(screen.getByRole("checkbox", { name: "Reload it after Studio" }));
  expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ reloadChatAfter: true });
});
it("removing the Windows engine asks first", async () => {
  // engine installed → "Remove" → confirm → api.studioEngineRemove once
});
it("a missing engine on Windows offers its download", async () => {
  // LOCAL with engine.state "notInstalled" and one installed model → "Download the Studio engine (29 MB, MIT)" → api.studioEngineDownload(true);
  // with no installed model → "Studio engine: comes with your first model (29 MB, MIT)" and no button
});
it("the daily limit is on at $10 and can be changed or turned off", async () => {
  // input "20" → api.studioSetPrefs({ dailyLimitUsd: 20 }) (only that field); untick → { dailyLimitUsd: null };
  // "0.5" → "Choose an amount from $1 to $1,000." and nothing sent
});
it("the confirmation choices can be changed", async () => {
  fireEvent.click(screen.getByRole("checkbox", { name: "Ask before every video" }));
  expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ confirmEveryVideo: false });
  setAmount("Ask before runs of", "2.5"); expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ confirmAtUsd: 2.5 });
});
it("Delete stopped and failed runs asks first", async () => {
  fireEvent.click(screen.getByRole("button", { name: "Delete stopped and failed runs" }));
  expect(api.studioDeleteUnfinished).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Delete" })); await waitFor(() => expect(api.studioDeleteUnfinished).toHaveBeenCalledTimes(1));
});
it("each model list says when it was updated, and Update now refreshes it", async () => {
  screen.getByText("OpenRouter · updated 17 Nov 2026 09:00");
  fireEvent.click(screen.getByRole("button", { name: "Update now" })); expect(api.studioRefreshModels).toHaveBeenCalledWith(1);
});
it("changing the folder says existing results stay where they are", async () => {
  dialogOpen.mockResolvedValue("/data/studio"); fireEvent.click(screen.getByRole("button", { name: "Change…" }));
  await waitFor(() => expect(api.studioSetPrefs).toHaveBeenCalledWith({ galleryDir: "/data/studio" }));
  screen.getByText("New results go to the new folder; existing ones stay where they are.");
});
// Endpoints.test.tsx
it("cards say what each company can do", () => {
  expect(abilitiesOf("openrouter")).toBe("Chat · Pictures · Video");
  expect(abilitiesOf("openai")).toBe("Chat · Pictures"); expect(abilitiesOf("anthropic")).toBe("Chat");
});
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/settings/StudioSettings.test.tsx src/components/settings/Endpoints.test.tsx`
Expected: FAIL.

- [ ] **Step 3: Implement** the page, local block first (addendum L1/L2): "On this computer": the local models' disk use with *Get local models* (opens Studio's view), the engine line ("Studio engine: built in" on a Mac; "comes with your first model (29 MB, MIT)", "installed · build <build> · Remove" or the download progress on Windows), "When a local job needs the graphics card": *Unload the chat model* (default) | *Ask* | *Use the processor for local pictures*, *Reload it after Studio* (off) (Task B14 adds the content notice link). When `engine.state` is `notInstalled` or `partial` and a local model is installed (antivirus quarantine, *Remove*, or a failed engine download while the model finished), the engine line offers *Download the Studio engine (<size>, MIT)* (`studioEngineDownload(true)`, its progress from `onEngineProgress`). Then the cloud parts from design §5 and §4: usage line and Open folder; Delete stopped and failed runs; *Ask before every video*; *Ask before runs of $X or more*; *Daily limit* (switch and amount); model lists with dates and Update now; gallery folder with *Change…* (folder picker through the dialog plugin) and the note "New results go to the new folder; existing ones stay where they are." `endpointPresets`: openrouter `{image: true, video: true}`, openai `{image: true, video: false}`.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/settings src/components/SettingsDialog.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/settings src/components/SettingsDialog.tsx e2e/m8_offline.py src/lib/words.ts
git commit -m "Studio: its settings page, and what each endpoint can make"
```

### Task B11: Send to chat and Open in Studio

**Files:**
- Modify: `src/components/Composer.tsx`, `Composer.test.tsx` (`app:compose-images`), `src/components/MessageItem.tsx`, `MessageItem.test.tsx` (*Open in Studio*), `src/store/studio.ts` (`sendToChat`), `src/components/studio/Viewer.tsx`, `RunCard.tsx`, `src/lib/words.ts`

**Interfaces:**
- Produces:
```ts
// window event "app:compose-images", detail: ImageInput[] → Composer adds them, up to MAX_IMAGES (8)
sendToChat(output: Output): Promise<void>;    // studioOutputForChat → showChat() (new chat when none is open) → app:compose-images
```

- [ ] **Step 1: Write the failing tests**
```ts
it("Send to chat puts the picture in the message box", async () => {
  // api.studioOutputForChat → {mime:"image/png", fileName:"lighthouse-1.png", data:"…"}; view becomes "chat"; event detail has it
});
it("Send to chat is hidden for videos", () => {
  render(<Viewer outputId="v1" onClose={vi.fn()} />);                  // a video/mp4 output
  expect(screen.queryByRole("button", { name: "Send to chat" })).toBeNull();
});
it("a picture over 20 MB cannot be sent and says why", async () => {
  api.studioOutputForChat.mockRejectedValue(appError("bad_request", "This picture is over 20 MB, so it cannot go into a chat."));
  fireEvent.click(screen.getByRole("button", { name: "Send to chat" }));
  await screen.findByText("This picture is over 20 MB, so it cannot go into a chat."); expect(useStudio.getState().view).toBe("studio");
});
// Composer.test.tsx
it("app:compose-images adds pictures up to 8", () => {
  // 6 already attached; event with 3 → 8 attached and the composer's own "8 pictures at most" notice
  expect(screen.getAllByRole("img", { name: /attached picture/i })).toHaveLength(8);
});
// MessageItem.test.tsx
it("Open in Studio puts an attached picture in the tray", async () => {
  // click → api.studioAddRef({mime, fileName, data}) → drafts.image.refs has it; view "studio"
});
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/Composer.test.tsx src/components/MessageItem.test.tsx src/components/studio`
Expected: FAIL.

- [ ] **Step 3: Implement.** The picture goes through the normal send path (`save_images`: 20 MB, 8 pictures, type check, WebP conversion, copied into `attachments/`), so deleting it from the gallery never breaks the chat.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/Composer.tsx src/components/Composer.test.tsx src/components/MessageItem.tsx src/components/MessageItem.test.tsx src/components/studio src/store/studio.ts src/lib/words.ts
git commit -m "Studio: send a picture to a chat, and open a chat picture in Studio"
```

### Task B12: Video playback (CSP) and poster frames

**Files:**
- Modify: `src-tauri/tauri.conf.json` (add `media-src blob:` after `img-src 'self' data: blob:;`), `src-tauri/src/commands.rs` (`capabilities_are_listed_explicitly`), `src/components/studio/Viewer.tsx`, `Grid.tsx`, `Viewer.test.tsx`

**Interfaces:**
- Produces: `export function capturePoster(video: HTMLVideoElement): string | null;` (base64 JPEG, longest side 512) and a call to `studioSaveThumb(outputId, base64)` the first time a video without a thumbnail plays.

- [ ] **Step 1: Write the failing tests**
```rust
// commands.rs: the pinned policy becomes
"default-src 'self'; script-src 'self'; connect-src 'self' ipc: http://ipc.localhost; \
 img-src 'self' data: blob:; media-src blob:; style-src 'self' 'unsafe-inline'; font-src 'self' data:; \
 object-src 'none'; frame-src artifact://sandbox.localhost http://artifact.sandbox.localhost; \
 form-action 'none'; base-uri 'self'; require-trusted-types-for 'script'"
```
```ts
it("a video plays from a blob: address", () => { expect(screen.getByTestId("studio-video").getAttribute("src")).toMatch(/^blob:/); });
it("the first play saves a poster frame", async () => { /* canvas mocked → api.studioSaveThumb("o9", "…") once */ });
it("a video too big to play here offers Show in folder", async () => {
  api.studioOutputBytes.mockRejectedValue(appError("studio_too_big_to_show", "This video is too big to play here."));
  await screen.findByText("This video is too big to play here.");
  fireEvent.click(screen.getByRole("button", { name: "Show in folder" })); expect(api.studioShowInFolder).toHaveBeenCalledWith("o9");
});
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib commands::tests::capabilities_are_listed_explicitly && npx vitest run src/components/studio/Viewer.test.tsx`
Expected: FAIL (the policy string differs; no poster call).

- [ ] **Step 3: Implement** the CSP change in `tauri.conf.json` and the poster capture (draw the current frame to a canvas scaled to 512 px, `toDataURL("image/jpeg", 0.8)`). Only `media-src blob:` is needed: the bytes come through `studio_output_bytes` (`tauri::ipc::Response`), so there is no asset protocol, `protocol-asset` feature or `convertFileSrc`, and `img-src` already allows `blob:` for thumbnails. Videos over `SHOW_CAP` (256 MiB, Task A15) are not played in the app; the main-window-only `studio:` scheme is left for later.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib commands && npx vitest run src/components/studio`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/tauri.conf.json src-tauri/src/commands.rs src/components/studio
git commit -m "Studio: let videos play from blob: addresses, and save their poster frames"
```

### Task B13: Get local models (the view, Hugging Face search, the Models dialog lines)

**Files:**
- Create: `src/components/studio/LocalModels.tsx`, `LocalModelRow.tsx`, `LocalSearch.tsx`, `LocalModels.test.tsx`
- Modify: `src/components/studio/StudioView.tsx` (shows `LocalModels` when `localView`), `src/store/studio.ts` (`localTab`, `onlyFits`, `showNonCommercial`, `search`, `openRepo`, `deleteLocal`, `offer`, `noticeOpen`), `src/components/models/Installed.tsx` and `Installed.test.tsx` (the Studio line), `src/components/models/Downloads.tsx` and `Downloads.test.tsx` (labels), `src/lib/words.ts`
- Test: `LocalModels.test.tsx`, `Installed.test.tsx`, `Downloads.test.tsx`

**Interfaces:**
- Consumes: Task B1 api and types (`studioLocalModels`, `studioLocalSearch`, `studioLocalRepo`, `studioLocalDelete`, `studioEngineStatus`, `studioEngineDownload`, `onEngineProgress`), Task B3 `FitDot` and `LOCAL_FIT_WORDS`, Task B8's `downloadEngine` and `repairLocal`.
- Produces:
```ts
export function hardwareLine(h: HardwareLine): string;    // "This computer: Radeon RX 7600, 8 GB graphics memory · 32 GB memory · 412 GB free"
                                                          // | "This computer: Apple M5, 16 GB shared memory · 180 GB free" | "This computer: processor only · …"
export function licenceBadge(l: Licence): { tone: "green" | "amber" | "red"; text: string };   // "Free for any use, including commercial" |
                                                          // "Commercial use only under $5M revenue" | "Non-commercial"
export function visibleEntries(local: LocalModels, tab: StudioTab, onlyFits: boolean): { shown: LocalEntry[]; notHere: LocalEntry[] };
export function LocalModels(): JSX.Element;               // replaces the card stream, with Back
export function LocalSearch(props: { tab: StudioTab }): JSX.Element;
/// What a Download button offers; Task B14's dialog shows it and sends every licence's hash.
export interface DownloadOffer { name: string; source: LocalSource; parts: Part[]; licences: Licence[]; totalBytes: number; toFetchBytes: number;
  needsEngine: boolean; engineBytes: number; lowDisk: boolean; gate: Gate }
export function offerForEntry(e: LocalEntry, local: LocalModels): DownloadOffer;
export function offerForFile(repo: LocalRepo, f: LocalFileChoice, local: LocalModels): DownloadOffer;
// store: search(text, tab): Promise<void> (only on Enter or the Search button); openRepo(repo): Promise<LocalRepo>; deleteLocal(id): Promise<void>;
//        offer: DownloadOffer | null (set by Download); noticeOpen: "accept" | "read" | null (set by the footer's What that means: "read")
```

- [ ] **Step 1: Write the failing tests**
```ts
it("the header says what this computer has, so the estimates can be checked", async () => {
  await showLocal(); screen.getByText(hardwareLine(LOCAL.hardware));
});
it("opening the view sends nothing; searching asks Hugging Face only when told to", async () => {
  await showLocal(); expect(api.studioLocalSearch).not.toHaveBeenCalled();
  typeIn(screen.getByRole("searchbox", { name: "Search Hugging Face" }), "klein"); expect(api.studioLocalSearch).not.toHaveBeenCalled();
  fireEvent.keyDown(screen.getByRole("searchbox", { name: "Search Hugging Face" }), { key: "Enter" });
  expect(api.studioLocalSearch).toHaveBeenCalledWith("klein", "image");
  screen.getByText("Searching sends your words to huggingface.co.");
});
it("Pictures and Video tabs, with Only what fits on by default", async () => {
  await showLocal(); screen.getByRole("tab", { name: "Pictures" }); screen.getByRole("tab", { name: "Video" });
  expect(screen.getByRole("checkbox", { name: "Only what fits" })).toBeChecked();
  expect(screen.getByRole("checkbox", { name: "Show non-commercial models" })).not.toBeChecked();
});
it("each model shows what it is good at, size, licence in plain words, fit and time", async () => {
  await showLocal(); const row = screen.getByRole("article", { name: /Z-Image Turbo/ });
  within(row).getByText("Photo-real pictures in 8 steps."); within(row).getByText(amountGb(zimage.sizeBytes));
  within(row).getByText("Free for any use, including commercial"); within(row).getByText("Fits");
  within(row).getByRole("button", { name: "Download" });
});
it("one picture model and one video model are marked Recommended for this computer", async () => {
  await showLocal(); expect(screen.getAllByText("Recommended for this computer")).toHaveLength(1);   // the Pictures tab
});
it("models that do not fit are folded away with a plain reason and no Download", async () => {
  await showLocal({ ...LOCAL, notHere: [{ ...zimage, fit: { ...zimage.fit, word: "notHere", reason: "needs 8 GB of graphics memory" } }] });
  fireEvent.click(screen.getByRole("button", { name: "Not for this computer (1)" }));
  screen.getByText("needs 8 GB of graphics memory");
  expect(within(screen.getByRole("article", { name: /Z-Image Turbo/ })).queryByRole("button", { name: "Download" })).toBeNull();
});
it("a recognised search result shows each file with its parts, total size, licence and fit", async () => {
  // api.studioLocalRepo resolves { status: "supported", files: [{ file: "z-image-turbo-Q5_K_M.gguf", parts: [3 parts, the encoder alreadyHere],
  //   totalBytes, toFetchBytes, fit: fits }], licence: green }
  await openResult("someone/Z-Image-Turbo-GGUF");
  screen.getByText("3 parts · 7.1 GB in all · 4.6 GB to download"); screen.getByText("Free for any use, including commercial");
  screen.getByRole("button", { name: "Download" });
});
it("an unknown family says Not supported yet and offers no Download", async () => {
  // studioLocalRepo → { status: "notSupportedYet", reason: "harnessUI does not know this kind of model yet.", files: [] }
  await openResult("someone/Mystery-Diffusion-GGUF");
  screen.getByText("Not supported yet"); screen.getByText("harnessUI does not know this kind of model yet.");
  expect(screen.queryByRole("button", { name: "Download" })).toBeNull();
});
it("a model harnessUI does not offer says why", async () => { /* status notOffered, reason RegionLimited → "Not offered in harnessUI: its licence excludes some regions." */ });
it("a label that differs from the family's licence is pointed out", async () => { /* labelMismatch → that sentence under the badge */ });
it("non-commercial results stay hidden unless asked for", async () => {
  // a supported result whose licence badge is red → hidden; tick "Show non-commercial models" → shown with the red "Non-commercial" badge
});
it("the footer shows the engine and the content notice", async () => {
  await showLocal(); screen.getByText("Studio engine: built in");
  screen.getByText("Local models run only on this computer; no company checks prompts or results.");
  fireEvent.click(screen.getByRole("button", { name: "What that means" })); expect(useStudio.getState().noticeOpen).toBe("read");
});
it("the footer offers the engine again when it went missing", async () => {
  await showLocal({ ...LOCAL, engine: { ...LOCAL.engine, state: "notInstalled", downloadBytes: 30_303_000 } });   // klein is installed
  fireEvent.click(screen.getByRole("button", { name: "Download the Studio engine (29 MB, MIT)" }));
  expect(api.studioEngineDownload).toHaveBeenCalledWith(true);
});
it("Download offers every part and every licence of the model", async () => {
  await showLocal(); fireEvent.click(within(screen.getByRole("article", { name: /Z-Image Turbo/ })).getByRole("button", { name: "Download" }));
  expect(useStudio.getState().offer).toMatchObject({ source: { catalog: { entryId: "z-image-turbo-q4" } }, licences: zimage.licences, parts: zimage.parts });
});
it("an installed model can be checked and repaired", async () => {
  // installed row "Download again" → api.studioLocalDownload({ installed: { genModelId: "flux2-klein-4b-q4" } }, [])
});
it("an installed model can be deleted, and Back returns to the cards", async () => {
  // installed row "Delete" → confirm → api.studioLocalDelete("flux2-klein-4b-q4") then refreshLocal; Back → localView false
});
// Installed.test.tsx
it("Installed has one line for picture and video models that opens Studio", () => {
  // studioLocalModels mocked → `Picture and video models: 1 (${amountGb(LOCAL.installedBytes)}) · Manage in Studio`; click → app:open-studio with detail "local"
  // and the list never offers Use for them
});
// Downloads.test.tsx
it("a picture or video model's download says so", () => { /* group.generatorKind "video" → "Video model" beside its name */ });
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/LocalModels.test.tsx src/components/models/Installed.test.tsx src/components/models/Downloads.test.tsx`
Expected: FAIL (`Failed to resolve import "./LocalModels"`).

- [ ] **Step 3: Implement** Wireframe L2: header (`hardwareLine`), `[Pictures | Video]`, *Only what fits* (on) and *Show non-commercial models* (off), the recommended entries as `LocalModelRow`s (name and summary, publisher, total size, `licenceBadge`, `FitDot` with the fit word and reason, the time line from `fit.eta` with "(first guess)" when not measured or "Time shown after your first run here" when there is none, *Download* or *Installed · Delete*), "Not for this computer (n)" folded, the search box and its results (`LocalSearch`: hits with downloads and the repository's own licence label; a click asks `studioLocalRepo` and shows its files, each with its parts and *Download*, or "Not supported yet"/"Not offered in harnessUI" with the reason), one line "Local video is slower and simpler than cloud video, but free and private." on the Video tab, and the footer (engine state from `local.engine` and `onEngineProgress`; *Download the Studio engine (<size>, MIT)* when the state is `notInstalled` or `partial` and a local model is installed; the notice link, which sets `noticeOpen` to `"read"`). *Download* sets `offer` (`offerForEntry` or `offerForFile`, licences from the DTO's `licences`); Task B14 renders the dialog and the notice from `offer` and `noticeOpen`. An installed row has *Download again* (`repairLocal`) beside *Delete*. `app:open-studio` with detail `"local"` opens Studio with `localView` on. `Downloads.tsx` labels a group with `generatorKind` "Picture model" or "Video model".

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio src/components/models`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio src/store/studio.ts src/components/models src/lib/words.ts
git commit -m "Studio: Get local models, with the recommended list, Hugging Face search and honest fit"
```

### Task B14: The licence confirmation and the one-time content notice

**Files:**
- Create: `src/components/studio/DownloadLocalModel.tsx`, `DownloadLocalModel.test.tsx`, `src/components/studio/LocalNotice.tsx`, `LocalNotice.test.tsx`
- Modify: `src/components/studio/LocalModels.tsx` (renders `DownloadLocalModel` for `useStudio.offer` and `LocalNotice` for `noticeOpen`), `src/components/settings/StudioSettings.tsx` and `StudioSettings.test.tsx` (the *What local models mean* link, which opens the notice read only), `src/components/studio/AdvancedDrawer.tsx` (a local model's seed help adds "The same seed on another graphics card, or on the processor, can give a slightly different picture."), `src/store/studio.ts` (`downloadLocal`, `acceptNotice`), `src/lib/words.ts` (the notice, word for word)
- Test: both test files

**Interfaces:**
- Consumes: Task B1 `studioLocalDownload`, `studioAcceptLocalNotice`; Task B13's `DownloadOffer`, `offer` and `noticeOpen`.
- Produces:
```ts
export function verdict(licences: Licence[]): string;   // the strictest: "Pictures you make: free for any use, including commercial." | the amber line | the red line
export function DownloadLocalModel(props: { offer: DownloadOffer | null; onClose(): void }): JSX.Element;
export function LocalNotice(props: { open: boolean; readOnly?: boolean; onAccepted(): void; onClose(): void }): JSX.Element;
// store: downloadLocal(offer): Promise<void>   // notice first when local.noticeAccepted is false, then the dialog's Accept → studioLocalDownload(source, shas)
```

- [ ] **Step 1: Write the failing tests**
```ts
it("the first local download shows the content notice first", async () => {
  useStudio.setState({ local: { ...LOCAL, noticeAccepted: false } }); clickDownload("Z-Image Turbo");
  screen.getByText("Local models run only on your computer.");
  screen.getByText(/Don't create sexual images of children, or sexual or intimate images of real people without their consent/);
  fireEvent.click(screen.getByRole("button", { name: "I understand" }));
  await waitFor(() => expect(api.studioAcceptLocalNotice).toHaveBeenCalledTimes(1));
  screen.getByText("Download Z-Image Turbo?");                       // then the dialog
});
it("the dialog lists every part with its size and licence, then the verdict", () => {
  renderDialog(offer(zimage));
  screen.getByText(`3 files, ${amountGb(zimage.sizeBytes)} in all, from Hugging Face (huggingface.co)`);
  screen.getByText("makes the picture"); screen.getByText("reads your prompt"); screen.getByText("picture decoder");
  screen.getByText("Pictures you make: free for any use, including commercial.");
  screen.getByText("Downloading means you accept these licences."); screen.getByRole("link", { name: "Read the licences" });
});
it("Accept and download sends the licence hashes it showed", async () => {
  fireEvent.click(screen.getByRole("button", { name: "Accept and download" }));
  expect(api.studioLocalDownload).toHaveBeenCalledWith({ catalog: { entryId: "z-image-turbo-q4" } }, zimage.licences.map((l) => l.sha256));
});
it("a model with two licences shows and sends both", async () => {
  // offer.licences [apache, kommunity] → two "Read the licences" links (each licence's pinned URL); the amber verdict;
  // Accept → api.studioLocalDownload(source, [apache.sha256, kommunity.sha256])
});
it("Cancel downloads nothing", () => { fireEvent.click(screen.getByRole("button", { name: "Cancel" })); expect(api.studioLocalDownload).not.toHaveBeenCalled(); });
it("the engine comes with the first model on Windows, and says so", () => {
  renderDialog({ ...offer(zimage), needsEngine: true, engineBytes: 30_303_000 });
  screen.getByText("Also downloads the Studio engine (29 MB, MIT licence).");
});
it("a part already here, a gated part, an amber licence and low disk space each say so", () => {
  // part.alreadyHere → "already on this computer"; gate { mode: "auto", collects: ["username","email"] } →
  // "The publisher receives your Hugging Face username and email." + "Open the model page"; amber →
  // "Commercial use only if your organisation earns less than $5M a year. The licence asks you to review pictures before you share them.";
  // lowDisk → "This leaves less than 10 GB free on this disk."
});
it("the footer and Settings show the notice again, read only", () => { /* LocalNotice readOnly → no "I understand" button, a "Close" */ });
// StudioSettings.test.tsx
it("What local models mean opens the content notice, read only", async () => {
  fireEvent.click(screen.getByRole("button", { name: "What local models mean" }));
  screen.getByText("Local models run only on your computer."); expect(screen.queryByRole("button", { name: "I understand" })).toBeNull();
});
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/DownloadLocalModel.test.tsx src/components/studio/LocalNotice.test.tsx src/components/settings/StudioSettings.test.tsx`
Expected: FAIL (modules missing; no "What local models mean" button).

- [ ] **Step 3: Implement** Wireframe L3 as a Radix AlertDialog, following `BackendDownloads.tsx`'s licence dialog: the parts list (label from the part's role, size, licence name), "Pictures you make" (`verdict`), the engine line when `needsEngine`, the gated-repository lines (which form, what the publisher receives, *Open the model page* in the browser), the amber line, the low-disk line, *Read the licences* (links to each licence's pinned URL), *Cancel* and *Accept and download*. The accepted hashes are exactly the licences shown: every distinct part licence (`offer.licences`), not only the strictest. The notice text is addendum L1's, word for word, from `words.ts`.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio src/components/settings`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio src/components/settings src/store/studio.ts src/lib/words.ts
git commit -m "Studio: every licence shown and accepted before a local download, and the one-time content notice"
```

---

## Phase C: Docs, privacy and version

### Task C1: Docs and their checks

**Files:**
- Modify: `README.md`, `ARCHITECTURE.md` (7.1: the Studio engine's pin, `sd.lock.json`, the Windows pack (what it keeps and why: every DLL the program imports) and the macOS build with its three submodules, changed only with passing contract tests and the engine-output fixtures re-recorded from the new build (`sd_fixtures_build.txt` names it); 7.5 providers: `agent::media`; new **7.9 Local generation (0.1.0)**: `runtime::sd`, one `sd-cli` process per job through the spawn helper, no `sd-server`, arguments only from typed fields, the prompt passed by file (`--prompt-file`, never in argv) and capped at 4,000 characters, `--disable-image-metadata` and the AI label, `.ckpt`/`.pt`/`.pth` refused, the engine folder never `resources/llama/main`, the graphics-card lease and the chat model's unload, fit and time estimates and the order of the fit words, the family table, tensor signatures and Hugging Face search; 11: picture and video models in the catalog's `generators` and how old apps ignore them; 12 storage: the gallery folder, V3 with the local columns, `gen_models`, `license_acceptances` and spending stubs; 13.2: the CSP string with `media-src blob:` (it already lacks `font-src 'self' data:`, so the whole string is copied from `tauri.conf.json`); 13.6: a `| Studio |` row and a `| Studio on this computer |` row; 13.7: a *Studio (0.1.0)* bullet; 14 frontend: the Studio view, local first; 16: the Windows installer is unchanged (the engine is a pack), the Mac app adds `sd-cli`, CI builds and checks it; 17 testing: the new tests), `SECURITY.md` ("Secrets and privacy": prompts, added pictures, results, "OpenRouter passes it on", the key never goes to a file host, a local Studio run sends nothing, Hugging Face and GitHub only on a click; "Running programs": the Studio engine pinned by SHA-256, its arguments from typed fields only, its prompt cap, the refused model formats; "The webview": `media-src blob:`; "Audit log": the Studio row keeps the prompt's length only; "Content": the notice, the prompt check, the AI label), `docs/SPEC.md` (line 121), `docs/dev/0.1.0/README.md` (owner's words kept verbatim, both requests and the four local decisions; links to both designs, this plan and the owner checklist), `scripts/notices.py` and `THIRD-PARTY-NOTICES.md` (stable-diffusion.cpp under "Bundled native components" for macOS, with what it builds in: ggml MIT, libwebp and libwebm BSD-3-Clause, Oniguruma BSD-2-Clause, utf8proc MIT with the Unicode data licence, cpp-httplib MIT, nlohmann json MIT, miniz, the zip library, darts-clone BSD-2-Clause, stb public domain or MIT, each licence read from the pinned tree rather than assumed (the zip library's in particular), libwebp's sharpyuv with libwebp; the Windows pack's components the same way, since its zip carries only `ggml.txt` and `stable-diffusion.cpp.txt`; and beside the GPU packs sentence "The Studio engine is not part of the Windows installer. It is downloaded with your first local model, after its licence is shown."), `third_party/licenses/` (each text above, copied from the pinned commit's `thirdparty/` and submodules: `stable-diffusion.cpp-LICENSE`, `ggml-LICENSE`, `libwebp-COPYING`, `libwebm-LICENSE.TXT`, `oniguruma-COPYING`, `utf8proc-LICENSE.md`, `cpp-httplib-LICENSE`, `nlohmann-json-LICENSE.MIT`, `miniz-LICENSE`, the zip library's licence file, `darts-clone-COPYING`, `stb-LICENSE`; `models/apache-2.0.txt` is Task L6's), `scripts/docs_test.py`
- Test: `scripts/docs_test.py` (new class `StudioDocs`)

**Interfaces:** none (docs).

- [ ] **Step 1: Write the failing tests** (`scripts/docs_test.py`)
```python
class StudioDocs(unittest.TestCase):
    """What the docs say about Studio matches the code: hosts, money rules, audit row, CSP, scope note."""

    def test_13_6_has_a_studio_row_with_its_hosts(self):
        row = paragraph(section("### 13.6 Network use"), "| Studio |")
        for words in ("`openrouter.ai`", "`api.openai.com`", "guarded fetcher", "without the key"):
            self.assertIn(words, row)

    def test_the_money_rules_are_the_codes(self):
        spend = read("src-tauri/src/studio/spend.rs")
        for name, value in (("CONFIRM_AT_USD", "1.0"), ("DEFAULT_DAILY_LIMIT_USD", "10.0"), ("UNKNOWN_PRICE_RESERVE_USD", "5.0")):
            self.assertRegex(spend, rf"pub const {name}: f64 = {re.escape(value)};")
        for doc in ("SECURITY.md", "README.md"):
            self.assertIn("$10", read(doc), doc)
            self.assertIn("$1 or more", read(doc), doc)

    def test_the_audit_row_keeps_the_prompt_length_only(self):
        bullet = paragraph(section("### 13.7 Audit log"), "- *Studio* (0.1.0)")
        self.assertIn("length", bullet)
        self.assertIn("not the prompt", bullet)

    def test_the_spec_says_the_scope_changed(self):
        self.assertIn("*(Changed in 0.1.0 at the owner's request: image and video generation on this computer, with cloud models "
                      "as the backup; see [docs/dev/0.1.0/README.md](dev/0.1.0/README.md).)*", read("docs/SPEC.md"))

    def test_the_csp_the_docs_describe_is_the_apps(self):
        csp = json.loads(read("src-tauri/tauri.conf.json"))["app"]["security"]["csp"]
        self.assertIn("media-src blob:", csp)
        self.assertIn(f"**CSP:** `{csp}`", paragraph(section("### 13.2 Webview lockdown"), "- **CSP:**"))
        self.assertIn("media-src blob:", read("SECURITY.md"))

    def test_the_offline_test_visits_studio(self):
        self.assertIn('"Studio"', read("e2e/m8_offline.py"))

    def test_13_6_has_a_row_for_studio_on_this_computer(self):
        row = paragraph(section("### 13.6 Network use"), "| Studio on this computer |")
        for words in ("`huggingface.co`", "GitHub", "on a click", "nothing while it runs"):
            self.assertIn(words, row)

    def test_the_engine_pin_the_docs_name_is_the_locks(self):
        tag = json.loads(read("sd.lock.json"))["sd_cpp"]["tag"]
        pin = "\n".join(section("### 7.1 Pinning and layout"))
        self.assertIn(f"`{tag}`", pin)
        self.assertIn("`sd.lock.json`", pin)
        self.assertIn("re-recorded", pin)
        self.assertEqual(read("crates/runtime/tests/fixtures/sd_fixtures_build.txt").strip(), tag)

    def test_7_9_states_the_local_rules_the_code_keeps(self):
        local = "\n".join(section("### 7.9 Local generation (0.1.0)"))
        for words in ("one `sd-cli` process per job", "no `sd-server`", "`--prompt-file`", "4,000 characters", "`.ckpt`",
                      "`--disable-image-metadata`", "never `resources/llama/main`", "Unload the chat model"):
            self.assertIn(words, local)
        self.assertRegex(read("crates/runtime/src/sd/args.rs"), r"pub const PROMPT_MAX_CHARS: usize = 4_000;")

    def test_security_says_a_local_run_sends_nothing(self):
        self.assertIn("A local Studio run sends nothing", read("SECURITY.md"))

    def test_the_readme_leads_studio_with_this_computer(self):
        studio = doc_paragraph("README.md", "Studio makes pictures and short videos")
        self.assertLess(studio.index("on this computer"), studio.index("OpenRouter"))

    def test_the_notices_name_the_studio_engine(self):
        notices = read("THIRD-PARTY-NOTICES.md")
        self.assertIn("stable-diffusion.cpp", notices)
        self.assertIn("The Studio engine is not part of the Windows installer.", notices)

    def test_no_document_promises_a_local_time_before_the_benchmark(self):
        if any(e.get("speed") for e in json.loads(read("catalog/models.json")).get("generators", {}).values()):
            return                                                # measured: times may be written
        for doc in ("README.md", "CHANGELOG.md"):
            self.assertNotRegex(read(doc), r"\b(s|seconds?|min|minutes?) (a|per|for a) (picture|clip)", doc)
```
(`import json` at the top of the script.)

- [ ] **Step 2: Run to see them fail**
Run: `python3 -I scripts/docs_test.py`
Expected: FAIL (`expected one line starting with '| Studio |', found 0`, …).

- [ ] **Step 3: Write the docs.** The local 13.6 row: `| Studio on this computer | The user presses Search in Get local models, opens a search result, or clicks Download | \`huggingface.co\` (search, listings and model files, with the stored token for gated repositories) and GitHub (the Windows engine pack), each on a click; nothing while it runs |`. README's Studio section opens "Studio makes pictures and short videos on this computer, free and private, with cloud models (OpenRouter, OpenAI) as the backup for computers without a good graphics card." and says how to get local models, what fits, the licence screen, then the cloud rules ($10 daily limit, confirmation for every cloud video and any run of $1 or more); its development notes add `python3 scripts/fetch_sd.py macos` before a Mac `tauri build` and `APP_DEV_SD_DIR` on Linux. Run `python3 scripts/notices.py` after editing it. The cloud 13.6 row: `| Studio | The user clicks Create, Get the list, Update model lists or Test, or picks an OpenRouter picture model (its price) | the chosen endpoint: \`openrouter.ai\` or \`api.openai.com\`; a result on the company's file host through the guarded fetcher, without the key |`. README: a short "Studio" section in plain words (what it does, the $10 daily limit, confirmation for every video and any run of $1 or more, where files go). The SPEC line gets the annotation in the test, in the same form as the 0.0.8 annotations (italic, a semicolon, a link). ARCHITECTURE 17 names `crates/agent/tests/media_endpoints.rs`, `media_redirects.rs`, `src-tauri/src/studio/manager_tests.rs`, `wire_tests.rs` and `e2e/m12_studio.py` (by hand). **Separate website task (other repository):** add the Studio rows to the harnessui.site privacy policy page added in 0.0.8 (a local run sends nothing; Get local models talks to huggingface.co and the engine comes from GitHub, each only on a click; cloud runs send the prompt and added pictures to OpenRouter or OpenAI; results stay on the computer; no telemetry), and the terms page section "Pictures and videos you make" (each model's licence and use policy apply; the core prohibitions; outputs carry an AI label that must not be removed where the law requires it; local generation has no company filter).

- [ ] **Step 4: Run to see them pass**
Run: `python3 -I scripts/docs_test.py`
Expected: PASS (`OK`).

- [ ] **Step 5: Commit**
```bash
git add README.md ARCHITECTURE.md SECURITY.md docs/SPEC.md docs/dev/0.1.0/README.md scripts/docs_test.py scripts/notices.py THIRD-PARTY-NOTICES.md third_party/licenses
git commit -m "Studio: architecture, security, readme and notices for local and cloud Studio, checked against the code"
```

### Task C2: CHANGELOG and version 0.1.0

**Files:**
- Modify: `CHANGELOG.md` (new `## 0.1.0 (<date>)` entry on top), `Cargo.toml` (`[workspace.package] version`), `Cargo.lock`, `package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`, `src-tauri/src/commands.rs` (version test), `scripts/docs_test.py`

**Interfaces:** none.

- [ ] **Step 1: Write the failing tests**
```rust
// src-tauri/src/commands.rs
#[test] fn the_version_is_the_same_everywhere() {
    let v = env!("CARGO_PKG_VERSION");
    assert_eq!(v, "0.1.0");
    let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    let pkg: serde_json::Value = serde_json::from_str(include_str!("../../package.json")).unwrap();
    let lock: serde_json::Value = serde_json::from_str(include_str!("../../package-lock.json")).unwrap();
    assert_eq!(conf["version"], v); assert_eq!(pkg["version"], v);
    assert_eq!(lock["version"], v); assert_eq!(lock["packages"][""]["version"], v);
}
```
```python
# scripts/docs_test.py, in StudioDocs
def test_the_changelog_tells_users_what_0_1_0_does(self):
    entry = read("CHANGELOG.md").split("\n## ")[1]
    self.assertTrue(entry.startswith("0.1.0 ("))
    for words in ("Studio", "On this computer", "stable-diffusion.cpp", "Hugging Face", "$10", "$1 or more", "### Added", "### Known limits"):
        self.assertIn(words, entry)
    self.assertLess(entry.index("On this computer"), entry.index("OpenRouter"))
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib commands::tests::the_version_is_the_same_everywhere; python3 -I scripts/docs_test.py`
Expected: FAIL (`left: "0.0.8"`; the changelog's first entry is 0.0.8).

- [ ] **Step 3: Write the entry and bump the version.** The entry opens with a paragraph for users that quotes the owner's requests (both, verbatim), then *Added* (Studio and how to open it; **On this computer** first: local pictures and video with the Studio engine (stable-diffusion.cpp), the recommended free models, Hugging Face search with "Not supported yet" for unknown families, the licence screen and the one-time notice, fit and time measured on this computer, Stop, the AI label in local files; then cloud as the backup: the companies, controls that follow the model with Advanced and chips, prices before you run, confirmation and the $10 daily limit, videos that keep going after you close the app, the gallery folder, Send to chat / Open in Studio, the audit row without the prompt), *Changed* (Test on an OpenRouter or OpenAI endpoint also fetches its picture and video lists; endpoint cards say "Chat · Pictures · Video"; a local Studio job unloads the chat model when it needs the graphics card, with *Load it again*, as Settings → Studio says; the Mac app includes the Studio engine; the Windows installer adds one Visual C++ runtime file the engine needs; importing a picture or video GGUF in Models says to add it in Studio; deleting a chat model keeps files a picture or video model uses; installing an update waits while a local Studio job runs), *Known limits* (on Windows the Studio engine uses Vulkan or the processor, a faster NVIDIA engine comes later; one local job at a time, and the model loads again for every run; local video is short, 480p by default and WebM; video on this computer needs a graphics card or an Apple-silicon Mac; only the families harnessUI knows can be downloaded from search; times are first guesses until a model has run here once; OpenAI publishes no per-picture price for GPT Image 2.6, and on OpenRouter GPT Image and Nano Banana are priced per token, so those runs say "price unknown", ask every time and hold $5 of the daily limit until they finish, when the real cost replaces it; extra settings for video models are not sent yet; how long OpenRouter keeps a finished video is not documented; a video over 256 MiB opens from its folder instead of playing in the app; going back to 0.0.8 after 0.1.0 has opened the database is refused, because 0.0.8 cannot read schema 3; built and tested on Linux, Windows and macOS from CI). Then:
```bash
sed -i 's/^version = "0.0.8"$/version = "0.1.0"/' Cargo.toml
npm version 0.1.0 --no-git-tag-version
# src-tauri/tauri.conf.json: "version": "0.1.0"
cargo check -p app            # rewrites the workspace crates' versions in Cargo.lock
```

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib commands && python3 -I scripts/docs_test.py && python3 -I scripts/release_notes_test.py`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add CHANGELOG.md Cargo.toml Cargo.lock package.json package-lock.json src-tauri/tauri.conf.json src-tauri/src/commands.rs scripts/docs_test.py
git commit -m "Release 0.1.0: Studio"
```

---

## Phase D: End-to-end check against a mock provider and the fake engine

### Task D1: A whole run against a local OpenRouter mock, and a local run against the fake engine

**Files:**
- Create: `src-tauri/src/studio/wire_tests.rs` (`#[cfg(test)] mod wire_tests;` in `studio/mod.rs`)

**Interfaces:**
- Consumes: the real `OpenRouterMedia`, `StudioManager`, `StoreProviders`, `Gallery`, `catalog`, `store::Store::open_in_memory()`, `store::MemorySecretStore`; for the local run the real `LocalMediaProvider` (Task L14) with the fake engine (`studio::local::tests::fake_engine`, made `pub(crate)` for this module), the real `GpuLease` over a `FakeChat` with a chat model loaded, `FixedHardware` (an 8 GB Vulkan card) and an installed FLUX.2 klein 4B whose parts are tiny files (the diffusion part `models::gguf::testing::diffusion_gguf("klein4b-tensors.txt")`: the 4B's tensor table and no metadata keys, as leejet's real file).
- Produces (test-only): `struct MockOpenRouter { fn start() -> Self; fn url(&self) -> String; fn count(&self, method: &str, path: &str) -> usize; fn requests(&self) -> Vec<(String, String)> }` on a `tokio::net::TcpListener`, serving the catalogue fixtures, flux-3-image's recorded `/endpoints` answer, `POST /api/v1/images` (one PNG, `usage.cost` 0.048), `POST /api/v1/videos` (202 `vid_1`), `GET /api/v1/videos/vid_1` (pending twice, then completed with `usage.cost` 0.36) and `GET /api/v1/videos/vid_1/content?index=0` (a small MP4 starting with an `ftyp` box).

- [ ] **Step 1: Write the tests**
```rust
#[tokio::test] async fn a_picture_and_a_video_through_a_mock_openrouter() {
    let mock = MockOpenRouter::start().await;
    let h = WireHarness::new(&mock).await;          // provider kind "openrouter" at mock.url() + "/api/v1", key in MemorySecretStore
    h.refresh_lists().await;
    assert_eq!(h.models().await.len(), 101);
    h.refresh_price("black-forest-labs/flux-3-image").await;   // the pick that fetches a picture price
    let pic = h.manager.start(h.picture("black-forest-labs/flux-3-image", json!({"resolution": "1K"})), false).await.unwrap();  // $0.048: no confirmation
    let pic = h.wait(&pic.id, "done").await;
    assert_eq!(pic.outputs.len(), 1);
    assert!(h.gallery_file_exists(&pic.outputs[0].path) && h.sidecar_exists(&pic));
    let vid = h.manager.start(h.video("x-ai/grok-imagine-video-1.5-lite", json!({"duration": 4, "resolution": "480p"})), true).await.unwrap();
    h.wait(&vid.id, "waiting").await;
    let h = h.restart().await;                      // first.shutdown(), then a new StudioManager on the same store and gallery
    h.manager.resume_interrupted().await;
    let vid = h.wait(&vid.id, "done").await;
    assert_eq!(vid.outputs.len(), 1);
    assert_eq!(vid.outputs[0].mime, "video/mp4");
    assert_eq!(mock.count("POST", "/api/v1/images"), 1);
    assert_eq!(mock.count("POST", "/api/v1/videos"), 1);
    assert_eq!(mock.count("GET", "/api/v1/videos/vid_1/content"), 1);
    assert!((h.manager.usage().await.unwrap().spent_today_usd - 0.408).abs() < 1e-9);
    assert!(h.audit_rows().await.iter().all(|r| !r.arguments.to_string().contains(&h.prompt)));
}
#[tokio::test] async fn opening_studio_makes_no_request() {
    // saved lists in the store; catalog::models_for, list runs, usage, prefs::load → mock.requests().is_empty()
}
#[tokio::test] async fn a_video_past_the_daily_limit_never_reaches_the_mock() {
    // spent 9.95 today; a 4 s 480p Grok Imagine Video 1.5 Lite run with no start frame (estimate $0.08) → "studio_daily_limit";
    // no POST /api/v1/videos
}
#[tokio::test] async fn a_local_picture_through_the_fake_engine_sends_nothing_and_costs_nothing() {
    let mock = MockOpenRouter::start().await;                 // running, so any request would be counted
    let h = WireHarness::with_local(&mock, "ok").await;
    let run = h.manager.start(h.local("flux2-klein-4b-q4", json!({"aspect_ratio": "1:1", "n": 2})), false).await.unwrap();
    let run = h.wait(&run.id, "done").await;
    assert_eq!((run.runs_on.as_str(), run.cost_usd, run.outputs.len()), ("local", Some(0.0), 2));
    for o in &run.outputs { assert!(label::read_label(Path::new(&o.path), "image/png").unwrap().contains(label::IPTC_TRAINED)); }
    assert!(mock.requests().is_empty());
    assert_eq!(h.manager.usage().await.unwrap().spent_today_usd, 0.0);
    let row = h.audit_rows().await.pop().unwrap();
    assert_eq!(row.arguments["runsOn"], "local"); assert!(!row.arguments.to_string().contains(&h.prompt));
}
#[tokio::test] async fn a_local_run_and_a_cloud_run_go_side_by_side_and_stop_works() {
    // fake engine mode "hang" for the local run; a cloud picture finishes meanwhile (one POST /api/v1/images);
    // cancel the local run → cancelled within 1 s, "Nothing was charged", .partial empty; the FakeChat was unloaded once (it did not fit beside)
}
#[tokio::test] async fn export_then_delete_leaves_the_audit_rows_and_the_spending() {
    // after the picture run: studio export path gets the file; delete_outputs removes the file and sidecar and the run leaves the list;
    // the audit row stays; usage().spent_today_usd is still 0.048
}
```

- [ ] **Step 2: Run them**
Run: `cargo test -p app --lib studio::wire_tests`
Expected: PASS once Phases A, L, B and C are in. A failure names the owning task (A4/A5 adapter, A13/A14 manager, A9 gallery, L5 runner, L13 label, L14 provider, L15 lane); fix it there, with a test in that task.

- [ ] **Step 3: Implement** only the mock and the harness (no product code). The harness builds `StudioDeps` with `StoreProviders`, a recording `StudioSink`, `GuardedFetch` (unused here), `SystemClock`, a `Scratch` gallery and `PollPlan { first: 20 ms, second: 20 ms, third: 20 ms, .. }`; `restart()` calls `shutdown()` on the old manager before building the new one, so the old tasks cannot finish the video themselves.

- [ ] **Step 4: Run the whole workspace**
Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio/wire_tests.rs src-tauri/src/studio/mod.rs
git commit -m "Studio: a whole picture and video run against a mock OpenRouter"
```

### Task D2: The real app against the mock, offline check, owner checklist

**Files:**
- Create: `e2e/m12_studio.py`, `e2e/fake_sd/sd-cli` (a Python script with a shebang that behaves like `runtime::sd::testing::fake_main` in mode `ok`: refuses an argument it does not know (as the pinned parser does), reads `--prompt-file`, prints the klein progress fixture with `\r` redraws and writes a tiny PNG to `-o`'s path)
- Modify: `docs/dev/0.1.0/owner-checklist.md` (adds the cloud and the local real-run sections after Task L1's benchmark), `e2e/m8_offline.py` (open Studio from the sidebar, both tabs, Get local models and Settings → Studio; then make a local picture with the fake engine; still zero requests), `e2e/README.md`

**Interfaces:** `e2e/m12_studio.py` reuses `e2e/webdriver.py` and a `ThreadingHTTPServer` mock with the same answers as Task D1, on `http://127.0.0.1:<port>/api/v1`, plus `GET /api/v1/models` with a small `data` list holding one text model (Test lists the chat models first and stops when that fails, before the Studio lists are fetched).

- [ ] **Step 1: Write the scenarios.** In `m8_offline.py`, after the existing checks: start the app with `APP_DEV_SD_DIR=e2e/fake_sd`; insert one `gen_models` row with `sqlite3` (as the script already does for `models`) whose parts are files the script writes (the diffusion part a minimal GGUF v3 file with no metadata keys and the tensor table of `crates/models/tests/fixtures/gguf/klein4b-tensors.txt`, 32 bytes of data per tensor); open Studio (it leads with "On this computer" and picks that model); create a picture; one tile appears; the recording proxy saw no request. In `m12_studio.py`, checks, each PASS or FAIL like `scripted_agent.py`: Studio opens with "On this computer" first and the empty state offering *Get local models*; add an OpenRouter-kind endpoint at the mock (Test fetches the lists) → open Studio with Ctrl+G → pick FLUX 3 Image (its price is fetched) → create a 1K picture (no dialog: $0.05) → one tile appears → start a 4 s video (dialog "Make this video?") → restart the app while it is waiting → the card finishes by itself and the mock saw one `POST /api/v1/videos` → Save as with settings off → Delete → the file is gone and Settings → Audit log still shows two Studio rows without the prompt → set the daily limit to $1, choose an 8 s 1080p video (~ $1.12) and see "Daily limit reached: $0.41 of $1 spent today" in place of Create (the deleted result still counts).

- [ ] **Step 2: Run them**
Run: `python3 -I e2e/m12_studio.py && python3 -I e2e/m8_offline.py`
Expected: every check PASS; m8_offline sees no request while Studio and Settings → Studio are open. Both run by hand on Linux with tauri-driver, as the other e2e scripts do.

- [ ] **Step 3: Write the owner checklist** (`docs/dev/0.1.0/owner-checklist.md`, after the benchmark section). **Local, on the MacBook Air and the Windows PC:** from a fresh install, Get local models shows the right hardware line and the free recommended list; download FLUX.2 klein 4B (on Windows the engine comes with it; SmartScreen or the antivirus does not block `sd-cli.exe`, or the block is named and the download offered again; on the Mac nothing asks for permission and Gatekeeper is silent); make a picture with a chat model loaded (unloaded with *Load it again* when it does not fit, the chat waits while Studio works); Stop a 2 s video halfway (the card says nothing was charged, the computer goes quiet); one Hugging Face search that finds a recognised quantization (parts, total and licence shown) and one that shows "Not supported yet"; quit while a video is being made, by closing the window and, on the Mac, with ⌘Q (the question appears both ways), and try *Install update* meanwhile (it waits); on a Windows PC without the Visual C++ redistributable installed, the engine starts from its own folder; `sd-cli -M metadata --image <result>` shows the AI label and no prompt. **Cloud:** one real run on each company, under $1 in total: a low-quality GPT Image 2.6 Flare picture on OpenAI; a picture and a 4 s 480p Grok Imagine Video 1.5 Lite video on OpenRouter (about $0.08 without a start frame); one safety refusal on each. Compare each shown cost with the company's activity page. Save the real answers (OpenAI generation, its `moderation_blocked` error, OpenRouter's picture answer, video status and content answers, and any redirect the content endpoint gives) and the request bodies the companies accepted (with a reference picture, a start frame and an extra setting) over the hand-written fixtures in `crates/agent/tests/fixtures/media/`, and re-run the tests, so the shapes Tasks A4 and A5 check come from real traffic. Note how long OpenRouter keeps a finished video. Play a video on Windows and on a Mac (blob playback and seeking).

- [ ] **Step 4: Run the full check list**
Run: `npm run lint && npm run typecheck && npx vitest run && cargo test --workspace && python3 -I scripts/docs_test.py`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add e2e/m12_studio.py e2e/m8_offline.py e2e/fake_sd e2e/README.md docs/dev/0.1.0/owner-checklist.md
git commit -m "Studio: app-level scenarios against a mock and a fake engine, offline check, owner checklist"
```

---

## Spec coverage

| Design section | Tasks |
|---|---|
| §1 What and where (navigation, shortcut, event, badge, ready notice, name, the viewer of Wireframe 4 with "OpenRouter (runs at …)") | B1, B2, B8, B9 |
| §2 Companies, keys, endpoint badges, lists fetched only on request (Test, Get the list, Update model lists with dates, a pick), greyed models, Video empty for OpenAI-only | A1, A2, A4, A7, A11, B2, B3, B10 |
| §3 Easy but not dumbed down (simple layer, Advanced, chips, Extra fields, request preview, seeds, setups, references, Animate, Edit, carrying values) | A1, A4, B4, B5, B6, B9 |
| §4 Jobs, cost, confirmation, daily limit, queue, never pay twice, resume, downloads, prices | A2, A3, A4, A5, A6, A7, A13, A14, B7, B8 |
| §5 Gallery, viewer, actions (incl. select several to delete or save), filters by kind, star, model, date and text, Settings → Studio, files on disk, V3, showing media, export, folder | A8, A9, A10, A15, A16, B9, B10, B12 |
| §6 Send to chat, Open in Studio | A16, B11 |
| §7 Privacy, Cloud mark, the company that ran it, no automatic requests, safety messages, audit row, Esc and the stop hotkey, docs | A13, A14, A15, B1, B2, B3, B7, B9, C1, D1, D2 |
| §8 Architecture mapped to files | File Structure, all tasks |
| §9 Scope, risks, tests, owner checklist | Review Focus, D1, D2 |
| **Local addendum: the owner's decisions** | |
| 1. Local is the main thing, cloud the backup; local pictures and video in 0.1.0; "On this computer" first; local default when it runs well; empty state and first-run suggestion lead with local; cloud suggested when nothing fits or a run is very slow | Global Constraints, B2 (`defaultModel`, `emptyState`, the suggestion), B3, B7 (`cloudBackup`), L8 (`VERY_SLOW_MS`), L16 (`very_slow`) |
| 2. Picking local models like chat models: the recommended list and Hugging Face search; recognised families get their companions from a known-good table with size, licence and fit; "Not supported yet" otherwise; gated repositories with the existing token | L6 (`generatorFamilies`), L7 (`recognise`, `family_from_tensors`, `entry_for`, `search_generators`), L11 (`entry_from_source` refuses an unrecognised source; `check_diffusion_file` before a searched file is added), L14 (the tensor check before each run), L16 (`studio_local_search`, `studio_local_repo`), B13 |
| 3. The Windows engine downloads with the first local model, pinned by SHA-256; the Mac bundles it built from source | L1, L2, L10, L11 (`needs_engine`), B14 (the engine line) |
| 4. The recommended list free for any use only (FLUX.2 klein 4B, Z-Image Turbo, Wan 2.2 TI2V-5B fast, plus Wan 2.1 1.3B if the benchmark calls for it); searched models show their licence and it must be accepted | L1 (the benchmark's video decision), L6 (licence hashes from bundled texts), L7 (`the_recommended_list_is_free_for_any_use_and_ungated`, `searched_licence`), L11 (`check_acceptance`), L16 (`licences`), L17 (applies the decision), B13 (badges, non-commercial hidden), B14 (every licence sent) |
| **Local addendum sections** | |
| L0 Summary; L1 What you see (picker group, Get local models, download and licence dialog, fit and time, local run card, Create line, the notice) | B2, B3, B5, B7, B8, B9, B13, B14, L16 |
| L2 The engine (choice, delivery, one process per job, progress, Stop, out of memory, failures, quitting, sharing the graphics card, "Chat replies may be slower", processor only with physical cores minus one) | L1–L5 (threads in L4), L10, L12, L14 (`LocalJobOptions`), L15 (`job_options`, `local.alongside`, `local.on_processor`, the quit question for the window and ⌘Q), B8, B10 |
| L3 Starter catalog (entries, never-in-catalog list, the catalog entry's shape, allowed file types, revisions, licences from upstream, Apache decoder copies, `gen_models`) | L6, L7, L9 |
| L4 Fitting the Studio design (same cards, seeds, Advanced for local, time instead of price, privacy, content, labels in files, the type changes) | A1, A2, A3, A8, A12, A13, L13, L14, L15, B4, B6, B9 |
| L5 Architecture (`LocalMediaProvider`, the local lane, fit, time estimate, downloads and store, Models dialog, commands, tests, docs) | L6–L17, B13, C1 |
| L6 Scope, effort and risks | Global Constraints ("Not in 0.1.0"), Review Focus 6–11 |
| L7 The one-day benchmark | L1 (owner checklist and its Results table: the video entry, Wan 2.1 as a fourth entry, the WebP quirk, the shared encoder, `memory[]`, `speed[]` and the recorded fixtures), L17 (applies them; the release waits for it), Global Constraints ("Time figures"), L8 (`no_measured_point_and_no_run_here_means_no_time`), C1 (`test_no_document_promises_a_local_time_before_the_benchmark`) |
| L8 Open decisions | Settled by the owner's four decisions at the addendum's top; the "treated as decided" items are in Global Constraints |

## Decisions this plan makes where the design was open

- **Raw lists are cached, not parsed models:** `providerMediaModels:<id>` keeps OpenRouter's (or OpenAI's) answers, and the controls are built when read, so a parser fix applies to lists saved earlier. The trait therefore has `fetch_catalog`/`fetch_image_price` instead of `list_models`/`model_price`, plus `preview` for *See the exact request*.
- **`MediaError` says whether the company may have the request** (`NotSent`, `Answered`, `Unknown`) or the failure was writing bytes here (`Local`: disk full, over a cap), which drives the never-retry rule, the charge line and *Get it anyway*.
- **`send_json` itself does not retry** (the 408/5xx/529 retries live in `send_stream_request`); `post_once` is still needed for the streamed body and to make "at most once" explicit and tested.
- **Polling:** 5 s for the first minute, 10 s until 5 minutes, then 20 s; after 30 minutes the run stays `waiting`, unwatched, with *Check again*.
- **A queued run is never sent after a restart**; it becomes "Not sent … Nothing was charged."
- **Spending:** a run counts on the day it was created; queued and running runs count by their estimate; an unknown price reserves $5 until the real cost arrives; each ended run's `error.charged` decides the rest through `spend::charge_of` (yes: its cost; unknown: its estimate; no: 0). A video whose job reported done has its cost recorded before the download, so a failed download still counts it.
- **Deleting keeps a spending stub:** a run deleted on the day it was made keeps a row with no prompt, settings or references (`deleted_at` set) so the daily limit cannot be undone from the gallery; yesterday's stubs are purged at start-up. Two columns (`deleted_at`, `upstream`) are added to the design's V3 table, which has not shipped.
- **What OpenRouter bills for pictures:** its image docs say a picture that failed, was cancelled or was cut off is not billed, so a stopped or timed-out OpenRouter picture is recorded as not charged ("OpenRouter does not charge for a picture you did not receive."); an OpenAI picture stopped the same way "may still charge" and counts its estimate.
- **Prices that are not per picture ask every time:** on 2026-11-17 GPT Image 2.6 and Nano Banana 2.1 on OpenRouter were priced per token, and OpenAI states no per-picture price for 2.6, so those runs have no estimate, confirm, and hold $5 of the limit while running; OpenAI's real cost comes from `usage` with the recorded text, image-input and image-output rates. A picture estimate uses the dearest allowed company unless routing pins one, and the dearest row when a price tier matches no offered resolution, so it is never too low.
- **Extra settings follow OpenRouter's docs:** passthrough keys go under `provider.options[<provider_slug>]` per company that lists them (the names come from the saved `/endpoints` answer, fetched when a picture model is picked); video extras are not sent in 0.1.0 because the video catalogue does not name the company a job is routed to, and Veo nests them differently.
- **Audit rows stay write-once:** *Get it anyway* on an ended run appends a follow-up row linked by `parent_audit_id` rather than changing the completed one; a stale (unwatched) `waiting` run keeps its row open.
- **The manager holds the prefs:** Settings → Studio changes (limit, confirmations, folder) apply to the next run without a restart, and `studio_set_prefs` takes a patch so a stale copy in another view cannot overwrite them.
- **Where it ran:** `upstream` is recorded when routing pins one company or the saved `/endpoints` answer lists only one; otherwise the viewer says "OpenRouter" (no extra request is made to find out).
- **Opening Studio restores the last model without asking for its price;** the dock offers *Get the price*, and the picker fetches it on a pick.
- **Large videos:** the viewer plays a video from a `blob:` up to 256 MiB; above that it offers *Show in folder*.
- **The webview fixture is a generated, typed `.ts` file,** not `.json`, so a serde/TS mismatch fails `npm run typecheck`; `ConfirmReason` is a unit enum (the threshold travels as `confirm_at_usd`) and `ParamKind` renames its fields too.
- **Duplicate presses:** a `clientToken` per Create press (kept in memory) plus a start lock, no schema change.
- **At 20,000 runs Studio refuses new runs** (`studio_gallery_full`) instead of deleting old results.
- **`studio_attach_to_chat` is replaced** by `studio_output_for_chat` plus the composer's `app:compose-images` event, so the picture goes through the normal send path (which already copies into `attachments/`); ChatView stays mounted, hidden, while Studio is shown, so drafts survive.
- **Added commands the design implied:** `studio_preview`, `studio_live`, `studio_add_ref`, `studio_ref_from_output`, `studio_ref_bytes`, `studio_delete_unfinished`, `studio_open_folder`.
- **Video models that need a video input** (4 on 2026-11-17) are listed greyed with the reason; `-1` (automatic length) is not offered; `size` on non-OpenAI picture models is an Extra key.
- **OpenAI edits use a hand-built multipart body** (no new dependency). OpenAI per-picture prices fill only what OpenAI states; the rest is "price unknown" and asks.
- **Copy to clipboard uses the webview's clipboard API** (no new Tauri permission), with a Save as fallback.
- **Changing the gallery folder affects new results only;** nothing is moved in 0.1.0.
- **Routing in Advanced is offered for OpenRouter pictures only** (their `/endpoints` answer names the companies); OpenRouter chooses for video.

**Local decisions (addendum and the owner's four decisions):**
- **Phase order A → L → B.** The local backend comes before the webview so every Studio screen is built local-first against real DTOs; the shared types the addendum changes are defined with their local parts in the Phase A tasks that introduce them (A1 `Company::Local`, the local `FailureKind`s, `MediaProvider::cancel`; A2 `Price::Free`; A3 `Billing`; A8 the V3 columns, `'running'`, `gen_models`, `license_acceptances`; A12 `RunTarget`, `GpuPolicy` and the local prefs; A13 `StartRequest.target`, `RunDto.runs_on`/`eta`/`local`, `EstimateDto.eta`/`local`). Phase L adds only new modules, plus `StudioDeps.local` (as A14 adds `poll`).
- **`RunTarget`, not `Target`:** `StartRequest.target` is `RunTarget { Endpoint(i64) | ThisComputer }`, because `models::fit::Target` (the hardware) is used beside it in the same files.
- **`Billing { Free, Paid(Option<f64>) }` in the spending rules:** `confirm_reasons` and `check_limit` take it, so the pure functions themselves never ask about or limit a local run; a local run is stored at $0 estimate and cost.
- **The local provider reaches the manager through `StudioDeps.local: Option<LocalLane>`** rather than the addendum's `ProviderSource::local()`, because the lane also needs the graphics-card lease and the hardware; until Task L15, `RunTarget::ThisComputer` is refused with `studio_no_local_engine`.
- **Local runs use the status `running`,** never `sending` or `waiting`; at start-up a local `queued` or `running` row becomes `interrupted` ("Run again", free).
- **Local controls are built in code from typed catalog fields** (`LocalMediaProvider::params_for`), not read as `ParamSpec`s from the catalog: every local control becomes an engine flag, so the set of flags stays closed; their `canonical` roles match the cloud ones, so settings carry between local and cloud models.
- **`parse_catalog(Company::Local, …)` reads back the `MediaModel`s the local provider serialised,** so local models go through the same validation, pricing (`Price::Free`) and DTOs as cloud ones.
- **The family table is catalog data, the recognition is code:** `generatorFamilies` in the signed catalog holds each family's companions, licences, defaults, limits, fit fallbacks and speed points (correctable by a signed catalog update, no release); the family ids, their name tokens and their tensor signatures are compiled in, because they decide engine flags and quirks. The 0.1.0 table has the four families the benchmark measures; anything else is "Not supported yet".
- **Searched models:** only a `.gguf` diffusion file with a published SHA-256; companions only from the table; the licence is the stricter of the family's upstream licence and the repository's own label (the strictest with any part's), and a label that differs is pointed out; red (non-commercial) results are hidden unless *Show non-commercial models* is on; region-limited, uncensored and not-for-all-audiences repositories are never offered; the backend recognises the repository again at the pinned commit before downloading.
- **Generator downloads take each part's own revision** (`row_commit`): the `downloads` table has no commit column and a group's parts come from several repositories; registration matches rows by destination path.
- **The fake engine is `runtime::sd::testing`,** compiled in and `#[doc(hidden)]` like `models::gguf::testing`, so `crates/runtime/tests/sd_cli.rs` (a `harness = false` binary that is the engine when `RUNTIME_FAKE_SD` is set, as `reaper_process.rs` is the reaper) and the app's tests (the libtest binary started with a test filter and `--`, as `attachments::tests::picture_worker_process` is the picture worker) share one fake. `Engine.leading_args` and `Engine.extra_env` exist only for it.
- **No model download in any test:** CI's contract test runs an `sd-cli` built from the pinned commit (CPU only; the upstream Linux zip needs glibc 2.43) for `-h`, `--list-devices` and the exact argv Studio builds, parsed in metadata mode without a model; the progress, failure and device fixtures are written now in the pinned build's exact format (from its source) and recorded over by the benchmark (Task L17), as the cloud plan does with the owner's real run; a pin change must re-record them (`sd_fixtures_build.txt`).
- **The prompt goes by file:** the pinned CLI has no `--flag=value` form, so the prompt and negative prompt are written into the run's partial folder and passed with `--prompt-file` / `--negative-prompt-file`; nothing the user typed reaches argv, the Windows command-line limit no longer bounds it (4,000 characters stays as a sanity cap, refused before a run is recorded), and the folder goes with the run.
- **Families are told apart by tensor tables:** leejet's and Green-Sky's GGUFs carry no metadata keys, and `general.architecture` cannot tell Wan 2.1 from Wan 2.2 or a 4B from a 9B, so a downloaded file's tensor table (names, width, block count) decides, after download and before every run; a search result is judged by its names (a distilled marker for the fast Wan) and a stated parameter count, which can only rule out.
- **The fast Wan 2.2 family is FastVideo's FastWan** (Apache-2.0); the full Wan 2.2 5B (30–50 steps, cfg 5) and other distillations whose base states no licence are "Not supported yet".
- **Licence hashes are those of bundled canonical texts,** because several upstream repositories state Apache-2.0 only in their README; the link goes to the upstream page at a pinned revision. A searched repository gets the stricter of its family's licence and its own label; an unknown or `other` label is "Check the licence" (red).
- **Search sends three queries and merges them** (text-to- and image-to- for the kind, and one untagged query kept only for family names), because the fast Wan repositories are tagged image-to-video or not tagged at all; the merged list is one page, with no "more".
- **Quitting while a local job works is asked in Rust:** the main window's `CloseRequested` and the app's `ExitRequested` without a code (the app menu's Quit, ⌘Q) go through one `quit::ask_before_quit`, are prevented and emit `studio-quit-requested`; *Quit and stop* calls `studio_quit_now`, whose `app.exit(0)` has a code and is not asked again; installing an update is refused while a local job runs. No window permission is added to the capabilities.
- **The benchmark is a gated task (L17),** not a step inside another: the plan ships a provisional video entry (FastWan, pinned from its listing) and hand-written fixtures, nothing waits for the owner's measurements, and the release waits for L17.
- **The content notice is accepted through `studio_accept_local_notice`,** which records the time; it is not a prefs patch field, so no other view can set it.
- **The Windows engine starts downloading inside `studio_local_download`** when it is missing, so the first model is one button; Settings → Studio can also download or remove it on its own.
- **The default model:** the last model used still wins; when there is none (or it is gone), an installed local model whose fit word is "Fits" or "Fits after the chat model is unloaded" (not "Slow here") is the default, else the cloud default. Studio never moves a draft between local and cloud by itself; the cloud backup is a suggestion with a button.
- **Speed figures are gated on the benchmark:** a first guess needs a `speed[]` point with `measuredOn`; with none and no run here, the UI says "Time shown after your first run here"; the docs test refuses a time figure in README or CHANGELOG while the catalog has no speed points. "Slow here" is above 2 min for a picture or 3 min for the default clip; "very slow" (amber line, cloud suggested) above 15 min.
- **The 1.3× video pad applies to every Apple-silicon Mac** until measured, because a fanless MacBook Air cannot be told apart reliably from the hardware probe.
- **"Fits beside the chat model" means Green or one part at a time** with the chat model loaded; partly on the processor counts as not fitting, so Studio unloads (or asks) rather than run very slowly. **The fit word follows one order** (`gen_eta::fit_word`): beside → "Fits"; fits after an unload → "Fits after the chat model is unloaded" before any red or slow rule; partly on the processor → "Slow here"; else "Not for this computer". The verdict colour, the ETA and memory saving follow the fit the run will have (`running_fit`).
- **What the lane decides reaches the engine** through `LocalJobOptions` (processor only and its threads, memory saving resolved from Automatic, the probed device), passed to `LocalProvider::start_local`; `MediaProvider::start` stays the same for cloud.
- **The lease never deadlocks a chat load:** a job holds the card only from its grant; while it waits for a reply or a Model check to end, chat loads go first. "Keep in memory" forces an unload only on shared memory (Macs), where mlock competes with the job; on a graphics card it pins system memory only. "Reload it after Studio" reloads once, when the local queue empties.
- **The Studio engine's version is a build number (1106)** compared with each entry's `minBuild`; an entry for a newer engine is hidden, never an error.
