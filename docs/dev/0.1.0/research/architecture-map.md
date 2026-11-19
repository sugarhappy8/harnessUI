ARCHITECTURE BRIEF: where an image and video generation section ("Studio") fits in harnessUI 0.1.0
I only read files under /home/user/harnessUI (origin/main at f67d49e) and edited nothing. Paths are relative to that folder.

## 0. Things that decide the design
- **The spec rules it out today.** docs/SPEC.md:121, "Out of scope for v1", lists "image generation". 0.1 has to record that the owner changed this, the same way 0.0.8 annotated the updater line in SPEC.
- **The chat code removes media models on purpose.**
  - `cloud::openai_can_chat` (crates/agent/src/cloud.rs:991) excludes `dall-e`, `gpt-image`, `chatgpt-image` and `sora`.
  - `OpenAiClient::list_model_entries` (crates/agent/src/client.rs:660) drops OpenRouter and abliteration.ai models whose `architecture.output_modalities` or `output_modalities` have no "text".
  - So a media model list is the reverse of these filters. It must be a separate list, so media models never show up in the chat ModelPicker.
- **Which companies apply.** Anthropic has no image or video generation API, so the section only applies to OpenAI, OpenRouter (images come back from chat completions with output modalities) and possibly abliteration.ai (not checked). For Generic endpoints it should be opt-in. The API details need separate research; nothing in this repo describes them.
- **"Generation" is already a name.** `state::Generation` (src-tauri/src/state.rs:149) and `Generation` / `generation` in src/store/app.ts mean the chat reply being written. Use another word, such as Studio, Creation or MediaJob.

## 1. Endpoints and providers
- **The form.** Settings → Endpoints is src/components/settings/Endpoints.tsx (425 lines). It has a form, a Test button, Edit, *Remove the saved key*, a context size per endpoint and the Cloud mark per endpoint.
- **Presets.** src/components/settings/endpointPresets.ts holds them: `COMPANY_CHOICES` (kind, name, baseUrl, `company{keyUrl, keyPage, defaultContext, contextNote}`) and `SERVER_CHOICES`. It also has `choiceFor`, `originOf` and `hostOf`. This is the natural place for a per-company flag such as `media?: {image, video}`.
- **Storage.**
  - The `providers` table holds id, name, kind, base_url, secret_ref (unused) and enabled (crates/store/src/schema.rs:202). Accessors are in crates/store/src/config.rs.
  - The context size per endpoint is the setting `endpointContext:<id>`.
  - Cached chat models are the setting `providerModels:<id>` (src-tauri/src/settings.rs:94). It is written by `test_provider` (src-tauri/src/providers.rs about line 400) so the picker works offline.
  - A media cache would follow the same pattern (`providerMediaModels:<id>`), and `delete_provider` would need to clear it.
- **Commands** (src-tauri/src/providers.rs): `list_providers`, `save_provider`, `delete_provider`, `test_provider`. The webview wrappers are in src/lib/ipc.ts:233-246.
  - `kind_of()` maps the kind string to `agent::ProviderKind`.
  - `client_for(state, id)` reads the key and builds `OpenAiClient`. A media client should be built the same way.
  - `marks_for` and `chat_marks` produce the Cloud host for the chat header.
- **Keychain.**
  - The `store::secrets::SecretStore` trait and its implementations are in crates/store/src/secrets.rs: OS keychain, chunked entries for Windows' 2,560-byte limit, one serialised writer, and a memory store for tests and Linux.
  - The key name is `provider-api-key:<id>`.
  - `prepare_key`, `may_hold_key` and `key_may_stay` keep a key with its scheme, host and port only. A keychain that cannot be read counts as holding a key.
  - The media feature should reuse the endpoint's key, so it needs no new secret.
- **The Rust provider layer** is crates/agent.
  - client.rs: the `ProviderKind` enum (LlamaServer, Ollama, LmStudio, Vllm, Generic, Anthropic, OpenAi, OpenRouter, Abliteration), `ClientConfig` (10 s connect limit, 300 s request or silence limit) and `OpenAiClient::new`.
    - A loopback endpoint gets `no_proxy` and no redirects.
    - Everything else gets `same_origin_redirects()` (private, line 280).
    - A local network endpoint gets `no_proxy`.
    - `with_http_client` runs `check_key_transport`.
    - `authorize()` sends Bearer, or `x-api-key` plus `anthropic-version` for Anthropic.
    - `get_json`, `post_json` and `send_json` call `cloud::scrub_key`.
    - Body caps: `MAX_JSON_BODY` 8 MiB, `MAX_COMPLETION_BODY` 32 MiB, `MAX_ERROR_BODY` 64 KiB.
    - Retries: up to 2, on 429 only when the server says how long (at most 30 s), and on 408, 5xx and 529 (`cloud::retry_delay` and `parse_retry_after`).
  - crates/agent/src/anthropic/: the Messages API (mod.rs, request.rs, stream.rs, tests.rs). It has no media support.
  - cloud.rs: `endpoint_url` (one parse shared by all three uses), `endpoint_place` (ThisComputer / LocalNetwork / Internet; any host name but `localhost` counts as Internet), `endpoint_host`, `check_key_transport`, `endpoint_origin`, `default_context`, `is_cloud_company`, `scrub_key`, and `remember_facts` / `known_facts` (`ModelFacts{context, max_output, thinking}`, kept in memory per endpoint and model).
  - The media client belongs here, as a new crates/agent/src/media/ module, so it inherits the redirect, proxy, key-transport and key-scrubbing rules. That needs `same_origin_redirects`, `authorize`, `read_capped` and `network_error` to become pub(crate).
- **Can an endpoint list its models?** Yes. `list_model_entries` returns `ListedModel{id, display_name, context_length}`: Anthropic from its own `/v1/models`, everyone else from `/models`.
- **How capabilities are represented.** Only lightly.
  - `agent::ModelCaps` (crates/agent/src/turn/mod.rs:400) has tools, vision, n_ctx, local, thinking, preserve_reasoning and images_in_tool_results.
  - Remote models are simply assumed to see images: `vision: if target.local {…} else { true }` in src-tauri/src/chat.rs:872.
  - `ModelOptionDto` (src-tauri/src/dto.rs:347) is `{ref, label, source, contextLength}`, and model refs look like `remote:<providerId>:<model>`.
  - The media feature needs a new type, for example `MediaModel{id, display_name, outputs:{image, video}, inputs:{text, image}}`. OpenRouter's listing has `architecture.input_modalities` and `output_modalities`. For OpenAI it would come from a table matched on model ids.
- **The Cloud mark.** src/components/CloudMark.tsx draws it from `ChatDto.cloudHost`. The backend decides through `providers::marks_for`, which uses `agent::endpoint_place`. Endpoints.tsx has `cloudTitle()`. The Studio needs the same mark next to its model picker, with text like "Prompts and pictures you add go to <host>".
- **Network documentation.** ARCHITECTURE.md 13.6 says all outbound HTTP goes through one module with a reason from a fixed list. In practice reqwest clients are built in several crates (agent, search, models/hf, mcp-host, reverse, runtime, hub.rs), and no lint enforces it; clippy.toml only bans `Command::new`. The "Remote endpoint" row of the 13.6 table needs the new paths and any CDN hosts added.

## 2. App shell and navigation
- **No router.** src/App.tsx (170 lines) holds `useState` flags: `settingsOpen` and `settingsTab`, `modelsOpen`, `shortcutsOpen`, `sidebarOpen`, `firstRunDone`.
  - The layout is `<AppNotices/>` + `[Sidebar] <ChatView/> <Workbench/>` + dialogs.
  - Deep links are window events: `app:open-models` (detail = tab name), `app:open-settings` (detail = tab), `app:focus-search`, `app:focus-input`.
- **Sidebar** (src/components/Sidebar.tsx, 320 lines). The header has icon buttons for Models (Boxes, Ctrl+M) and Settings, then "New chat", search and chat history, and `RuntimeChip` at the bottom. A Studio entry fits as a third header icon or a row under "New chat".
- **Workbench** (src/components/Workbench.tsx). A `TABS` array has Artifacts, Task, Sources and Context, each `view: ComponentType<{chatId}>`. It only shows while a chat is open. `WorkbenchTab` and `openWorkbench` are in src/store/app.ts:200. It is tied to a chat, so it does not fit a section that works across chats.
- **Dialogs** use the Radix Dialog and Tabs pattern.
  - ModelsDialog.tsx: 52rem × 38rem. Each tab is a file in ./models/, and a new tab is one trigger plus one content line.
  - SettingsDialog.tsx:676-707 has 15 tabs.
- **Recommendation.** A top-level view switch in App.tsx (`view: "chat" | "studio"`) that renders `<StudioView/>` in place of ChatView and Workbench. A dialog is too small for a gallery, and jobs need to keep running while the user chats. The store slice would go in a new src/store/studio.ts; src/store/compare.ts is a precedent for a separate zustand store.
- **Shortcuts.** The keydown handler is in App.tsx. Ctrl/⌘ N, K, L, M, B, `,` and `/` are taken, and Esc stops the open chat's reply. Esc must not cancel a paid video job. Add a row to the ROWS list in src/components/ShortcutsDialog.tsx.
- **IPC wrappers.** Either the shared src/lib/ipc.ts or a feature-local api.ts like src/components/artifacts/api.ts. Types go in src/lib/types.ts or a feature types.ts. Plain-language copy goes in src/lib/words.ts.

## 3. Media handling
- **Attachments** (src-tauri/src/attachments.rs).
  - Limits: `MAX_IMAGE_BYTES` 20 MiB and `MAX_IMAGES` 8 per message.
  - Magic-number check (`sniff`) for PNG, JPEG, GIF and WebP.
  - WebP is converted to JPEG or PNG by the picture worker (`--internal-picture-worker`, 1 GiB memory, 60 s, at most 8,192 px a side).
  - Bytes are stored by SHA-256 in `<app_local_data_dir>/attachments/<sha>.<ext>` (`store_dir`, line 611), with a row in `attachments` (chat_id, sha256, mime, file_name, size_bytes, bytes_path).
  - There is no cleanup of orphaned files.
  - `get_attachment` returns a `data:` URI. MessageItem.tsx and Composer.tsx show images with `<img>`.
  - These validators can be reused for reference images (image-to-image, image-to-video) and for checking returned bytes.
- **Artifacts "pictures"** are SVG only. The kinds are `markdown | html | svg | mermaid` (schema V2, crates/store/src/schema.rs:262).
  - Previews are sandboxed frames served by the `artifact` URI scheme (src-tauri/src/artifacts/protocol.rs; `register_asynchronous_uri_scheme_protocol` at src-tauri/src/lib.rs:107). They use one-use tickets, `CSP sandbox`, and serve the main window only, GET only.
  - Raster images and video are not artifacts today.
- **CSP** (src-tauri/tauri.conf.json):
  - `img-src 'self' data: blob:` and no `media-src`, so `<video>` falls back to `default-src 'self'`.
  - Playing video needs `media-src blob:` (bytes from IPC) or a new main-window-only `media` scheme.
  - A test pins the whole CSP string: src-tauri/src/commands.rs:210.
  - On Windows a custom scheme is `http://<scheme>.localhost`.
  - WebKit (macOS) wants Range support to seek in a video served by a custom scheme.
  - No raw-bytes IPC (`tauri::ipc::Response`) is used yet.
- **Saving and export.**
  - The dialog plugin's `save`/`open` are allowed for the main window only (src-tauri/capabilities/main.json).
  - The pattern is ArtifactsTab.tsx:511: `saveDialog({defaultPath, filters})`, then `export_artifact` (src-tauri/src/artifacts.rs:666).
  - `export_target` (line 629) requires an absolute path and the right extension, canonicalises the folder, and refuses the app's own folders (`permissions::forbidden_for_tools`, src-tauri/src/permissions.rs:161).
- **App data** is `app.path().app_local_data_dir()`: attachments, browser profile, backends/, reverse/, benchmarks/, logs. Models live in a visible `<home>/harnessUI/models`, because the Windows uninstaller may delete app data. A large gallery may deserve the same kind of visible folder.
- **SQLite store** (crates/store).
  - One actor thread (actor.rs) and an async API per module (chats.rs, artifacts.rs, outputs.rs and so on), re-exported from lib.rs.
  - Migrations are a list in schema.rs (`MIGRATIONS = [V1, V2]`, kept in `PRAGMA user_version`). Rule: never edit a shipped migration; append V3.
  - Each step runs in a transaction with a foreign-key check, and there are tests for upgrading from older versions.
  - 0.0.6 and 0.0.7 added no schema and kept new data in `settings` JSON. A gallery of jobs and creations (id, provider_id, model, kind image|video, prompt, params JSON, status, remote_job_id, file path, mime, size, w/h/duration, created/finished, error) justifies a V3 table, in a new crates/store/src/creations.rs.
  - Existing per-table caps to copy: `MAX_ARTIFACT_BYTES`, `MAX_OUTPUTS_PER_CHAT` = 200.

## 4. Background jobs, progress and cancellation
- **Downloads** (src-tauri/src/downloads.rs, `DownloadManager`) are the best template.
  - `running: HashMap<group, Running{token, paused, ended, id}>`.
  - `TaskEnd` cleans up on every exit, panic included.
  - Progress goes out with `app.emit_to(MAIN_WINDOW, "download-progress", dto)`, throttled through `Live{last_emit, last_persist}`.
  - State is saved in the DB plus `downloadGroup:<id>` settings.
  - `resume_interrupted` runs at start (lib.rs about line 408).
  - The "cancelling" marker `downloadCancelling:<id>` is finished at the next start.
  - The webview listens with `ipc.onDownloadProgress` through `listenInEffect` (src/lib/listen.ts), which handles listeners that start after the cleanup. src/components/models/Downloads.tsx shows the list.
- **Updates** (src-tauri/src/updates.rs) show time limits for connecting, each read and the whole check, a `busy` flag so one runs at a time, and a status that is both pushed (`update-state` event) and readable (`update_status`), because events are not replayed.
- **Others.** Long external runs: outside_agent.rs `MAX_RUN` of 30 min with progress ticks. Pack downloads: packs_cmd.rs, "pack-progress". Polling hook: src/lib/usePoll.ts, which pauses while the window is hidden.
- **Video jobs** (submit, then poll for minutes):
  - Save the remote job id before polling, and resume polling at start like `resume_interrupted`.
  - Cancelling locally stops polling. It may not stop the company's job or its charge.
  - Stream the result to a temp file, then rename. Never buffer it through `read_capped`.
  - Do not auto-retry the POST that creates a job: the existing retry rules (5xx and timeouts) could pay twice.

## 5. Safety and privacy hooks
- **Audit log** (crates/store/src/audit.rs: `append_audit` and `complete_audit`; ARCHITECTURE 13.7).
  - Append-only triggers.
  - Decision CHECK values: allow_once, allow_for_chat, allowed_by_grant, allowed_without_prompt, allowed_by_mode, denied, cancelled, refused_by_policy.
  - The sharing proxy's `audit_entry` (src-tauri/src/share/proxy.rs:1956) shows how to log a non-tool request with no chat (`chat_id: None`, its own `server`, a preview, `allowed_without_prompt`) without a schema change.
  - A user-started generation would be server "studio", tool "image" or "video", arguments with the model and parameters (and the prompt, as a decision to make), decided by "user".
- **Approvals.** Only tool calls from the model go through the policy engine (crates/policy/src/engine.rs `Scope`) and approval cards. A user-started Studio request needs no card, as with Endpoints → Test. A later `generate_image` tool for chat models would need a policy scope with a cost warning, and should ask even in Allow all.
- **Privacy.**
  - SECURITY.md "Secrets and privacy" (line 487) and its "Cloud AI companies (0.0.7)" bullet list everything sent to a company. Prompts, reference images and returned media must be added, along with "OpenRouter passes it on".
  - README.md:6 promises no telemetry. ARCHITECTURE 13.6 has no new automatic requests, so opening Studio must make none.
  - e2e/m8_offline.py opens every Settings page (its `SETTINGS_PAGES` list) behind a proxy that refuses everything. The media model list must therefore come from cache, not be fetched on open.
- **Address checks.** A result URL the server chose, such as a CDN link, is "an address someone else chose". The guarded fetcher `search::Fetcher` (crates/search/src/fetch.rs; addresses resolved and pinned, private and loopback refused, redirects followed by hand, no proxy) only returns text (`Accept::Page|Json`, 2 MiB). It needs a binary, stream-to-disk variant, and the API key must never go to a host other than the endpoint's.
- **Settings persistence.** The generic `set_setting` only accepts the `ALLOWED` list (src-tauri/src/models_cmd.rs:489); features add their own typed commands and settings keys. Studio defaults (last model, aspect ratio, advanced settings open or closed) need such a command, or localStorage on the webview side.
- **New commands** must be added in three places: src-tauri/commands.list, an `allow-<cmd>` entry in src-tauri/capabilities/main.json, and `generate_handler!` in src-tauri/src/lib.rs:108. The build script turns these into a per-window ACL.
- **Errors** are the four-part `AppError{code, message, next_step, details}` (src-tauri/src/error.rs), with `From<ProviderError>` at line 80. `ProviderError::Refused` exists for a moderation stop.

## 6. Tests
- **Rust, agent crate.** crates/agent/tests/ has `common/mod.rs`, a hand-written HTTP/1.1 `MockServer` that records requests (`Recorded`) and serves canned `Reply`/`StreamEnd`. Tests built on it include cloud_endpoints.rs (fake Anthropic and OpenAI-compatible servers, no key or network), redirects.rs, proxy.rs, proxy_this_computer.rs and client_mock.rs. A new crates/agent/tests/media_endpoints.rs fits here: create, poll and download, a redirect to a foreign host, the key scrubbed from errors, oversized bodies, no retry of the create request.
- **Rust, elsewhere.** Inline `#[cfg(test)]` in each module. Store migration tests are in schema.rs (start from a V2 database). Download task tests are in downloads.rs (`fake_task`). Settings commands check their window.
- **Frontend.** Vitest with jsdom covers `src/**/*.test.{ts,tsx}` (vite.config.ts), with `vi.mock("../lib/ipc")` and mocks per component (see src/components/ChatView.cloud.test.tsx and settings/Endpoints.test.tsx). Store tests are src/store/app.test.ts and compare.test.ts.
- **e2e.** e2e/*.py use tauri-driver and WebKitGTK on Linux. scripted_agent.py holds the scenarios and m8_offline.py the offline check. Both are run by hand, not in CI.
- **CI** (.github/workflows/ci.yml): lint, typecheck, vitest, build, `cargo fmt`, check-spawn.sh, notices.py --check (needed if a dependency is added; see deny.toml), python -I scripts/docs_test.py (keeps some ARCHITECTURE sentences in step with the code), `clippy -D warnings`, `cargo test --workspace`.

## 7. Files that would change (about 15, plus new ones)
1. crates/agent/src/media/{mod.rs, openai.rs, openrouter.rs} (new) and crates/agent/src/lib.rs: create, poll and download clients; `MediaModel`; `list_media_models`.
2. crates/agent/src/client.rs: make the redirect, authorize and read helpers pub(crate); a larger cap for media JSON; a streamed download to a file.
3. crates/agent/src/cloud.rs: the reverse of `openai_can_chat` (which media a model makes); modalities in `ModelFacts`.
4. crates/store/src/schema.rs (V3), a new crates/store/src/creations.rs, and crates/store/src/lib.rs.
5. src-tauri/src/studio.rs (new): the job manager modelled on `DownloadManager`; the commands list_media_models, start_creation, cancel_creation, list_creations, creation_bytes, delete_creation and export_creation; a `studio-progress` event; audit rows.
6. src-tauri/src/lib.rs: manage the manager, register the commands, resume jobs at start, and register a media scheme if that route is chosen.
7. src-tauri/commands.list and src-tauri/capabilities/main.json.
8. src-tauri/tauri.conf.json and src-tauri/src/commands.rs (the CSP pin test): `media-src`.
9. src-tauri/src/providers.rs: cache media models when testing (`providerMediaModels:<id>`) and clear them on delete. src-tauri/src/dto.rs: the DTOs.
10. src/App.tsx: the view state, the shortcut, an `app:open-studio` event.
11. src/components/Sidebar.tsx: the Studio entry.
12. src/components/studio/* (new): StudioView, PromptPanel (simple fields with an Advanced section), ModelPicker with the Cloud mark, Gallery, JobRow, api.ts, types.ts. Optionally src/store/studio.ts.
13. src/components/settings/endpointPresets.ts: media flags per company. Optionally Endpoints.tsx shows "Images · Video".
14. src/components/ShortcutsDialog.tsx and src/lib/words.ts.
15. Docs and version.
    - Docs: ARCHITECTURE.md (7.5, 12, 13.6, 13.7, 14, 17), SECURITY.md (Secrets and privacy), CHANGELOG.md, docs/SPEC.md (scope), README.md, docs/dev/0.1.0/README.md.
    - Version to 0.1.0 in Cargo.toml `[workspace.package]`, package.json and src-tauri/tauri.conf.json. Tests that hard-code "0.0.8" (AppNotices, UpdateBox, SettingsDialog.updates) use fixtures and can stay.
16. Tests: crates/agent/tests/media_endpoints.rs, a store V2→V3 test, src/components/studio/*.test.tsx, and a Studio visit in e2e/m8_offline.py.

## 8. Risks
- **Scope and spec.** The scope change, and that each company's API has to be researched; nothing in the repo describes them. Anthropic has none.
- **Body sizes.** Base64 images can pass the 8 MiB JSON cap. Video has to stream to disk.
- **Result URLs on other hosts.** They need guarded-fetch rules and the key must never go to them. The 0.0.7 promise "a key goes only to its endpoint's address" must hold.
- **Cost and paying twice.** No retry of job creation. Show the model, size, duration and number before Generate, and add an explicit confirmation for video.
- **Restarts and cancel.** Polling must survive a restart, and a cancel may still be charged.
- **CSP and playback.** The pin test, and WebKit needing Range for video from a custom scheme.
- **Disk growth.** There is no orphan cleanup for attachments today. The gallery needs delete and possibly a visible folder, because the uninstaller removes app data.
- **Privacy and network docs.** Copy must stay accurate, opening the section must make no request (m8_offline), and docs_test.py and 13.6 must stay in step.
- **Naming.** The clash with chat "Generation".
- **Thin model metadata.** OpenAI's list says nothing about modalities, so a table matched on ids will go out of date.
- **Interface.** "Easy but not dumbed down" suggests presets (aspect ratio, quality, count, duration) with an Advanced section (seed, size, negative prompt, reference image, fps, raw parameters for that model), and the Cloud mark always shown.
