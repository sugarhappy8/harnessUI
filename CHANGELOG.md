# Changelog

All notable changes, by milestone. Dates are UTC.

## M8 — Polish (in progress)

### Added
- Opt-in updates: the updater plugin is registered only in builds that carry the updater public key (release builds with `src-tauri/tauri.updater.conf.json`, merged when the signing secret exists). *Settings → General → Updates*: *Check now*, *Install and restart* (stops llama-server and every tool child first), and an automatic check at start that is off by default. The check runs in Rust; the webview has no updater permission.
- Keyboard shortcut sheet (Ctrl+/ or Cmd+/) and a toggle for the chat list (Ctrl+B or Cmd+B).
- Models whose template has no tool support: the Tools panel says so (8.1).
- After every load (7.4): a 24-token timed generation; a GPU backend below 5 tokens/s shows "Loaded but slow", and a partial offload shows "N of M layers on the GPU" next to the backend.
- The backend list says "No Vulkan driver was found" when the Vulkan loader is missing, instead of listing no GPU (7.2).
- Streamed text reaches the webview in batches of about 30 ms (14).
- Long chats render their latest 80 messages, with "Show earlier messages".
- Version arrows also show on a regenerated turn that starts with a tool call.
- *Settings → Permissions → Terminal environment*: the user-editable list of variables the terminal passes through despite secret-looking names (9.3).
- MCP servers whose runtime is missing (Node.js, uv, Docker) link to its installer.

### Fixed
- macOS: the terminal and git tools now get the login shell's environment captured at start (only the MCP host had it).
- `e2e/m8_offline.py`: the app behind a recording proxy that refuses everything starts, loads a local model, answers and shows every settings page without one outbound request.
- Frontend tests for the streamed-turn reducer and approval bookkeeping.

### Changed
- Dev builds keep debug info only for the workspace's own crates.

## M3 — Agent loop, MCP host, approvals (in progress)

### Added
- `crates/agent` (agent loop, ARCHITECTURE.md 8): `run_turn` drives a turn from the user's message to the answer: native tool calls with schema-guided argument repair, a fallback parser for calls written as text (Hermes, Qwen, Llama, Mistral, Gemma, GLM, gpt-oss and other marker formats; a recovered call always asks), validation against the full schema with one-line error messages back to the model, a step cap with a wrap-up, a mistake budget and repeat detection. One system message (chat prompt, preamble with approved folders and OS, skills index, loaded skills, summary); tool results and attachments in random-delimited untrusted blocks with the model's own control tokens broken. Context budget per 8.4 with result caps, stubbing of old results, image pruning, summarisation and truncation, exact counts from llama-server. 212 tests, including runs against the real llama-server.
- `crates/mcp-host`: one registry for built-in and external tools; stdio servers through the runtime spawn module (scrubbed environment, process tree killed on Stop 500 ms later and restarted on next use) and Streamable HTTP through rmcp, both the 2025-11-25 handshake and the 2026-07-28 lifecycle with the working one remembered; config import from Claude Desktop, Cursor and VS Code JSON; credential scanning of arguments and URLs; `${secret:…}` placeholders resolved from the keychain at start; schemas compiled offline (no remote `$ref`) and simplified for llama-server; a generic describer that binds each approval to the exact arguments; tool fingerprints; Windows `.cmd` quoting; per-server stderr logs. 76 unit tests and 14 end-to-end tests against real rmcp servers.
- App:
  - Every reply is an agent turn. Tool calls show as cards with arguments, live output and the result; thinking shows while it streams; the header shows context used.
  - Approval cards in the chat (Allow once / Allow for this chat / Deny with a note), sent only to the main window with a random id; input ignored for 700 ms; Enter never approves. Stop closes an open card and cancels the call. "Allow for this chat" grants apply at once and last until the app quits (kept in memory only, D12); *Tools → Forget everything I allowed in this chat* clears them.
  - One audit row per tool call (decision, who decided, result, duration).
  - Per-chat Tools panel: tools by server with token estimates and the total share of the context. A new chat starts with files (and the skill tools while a skill is enabled); after that, with the tools you last chose. Terminal and git start off.
  - *Settings → MCP servers*: paste a config, see each server's command line with found credentials replaced, move credentials and every `env`/header value to the keychain, confirm the "runs with your permissions" warning; per-server on/off, restart, remove (deletes its secrets), status and log tail; per-tool "offer to the model", "allow any arguments" and "always ask". Tools of a new server start off; a tool that changed since it was reviewed is off again and its grants are removed.
  - Built-in servers registered: files, terminal, git (system git with the user's own credential helpers), skills.
  - A per-chat *Thinking on/off* switch in the header for local models whose template can think (read from the GGUF header); off sends `enable_thinking: false`.
- `e2e/m3_agent.py`: a real turn with Qwen3 0.6B: Allow once, Allow for this chat then a call under the grant, Deny (nothing written, the denial reaches the model), Stop with a card open, and the audit rows for each.

### Changed
- `regenerate` answers the user message a turn started from, so a turn with tool calls is regenerated whole.
- The llama-server device probe waits up to 45 s (macOS scans a new binary on its first start; the CI runner took longer than 15 s).
- Skill file listings use `/` on every OS.

### Known issues
- MCP OAuth (servers that answer 401 with a sign-in challenge) is reported as "needs sign-in" and not supported yet.

## M2 — Model manager (in progress)

### Added
- `crates/models`:
  - GGUF header reader that treats every length as hostile (no allocation from a length the file cannot hold; vocabularies skipped, not kept), plus a prefix reader for headers fetched with a ranged request before download.
  - Model summary: architecture, quantization, trained context, per-layer KV heads, template badges (tools, thinking), projector detection; KV-cache size per token by llama.cpp's formula.
  - Hugging Face file classification in the documented order (non-GGUF, projector, sidecars, shards, quantization label) using the regex from the 34,052-file research corpus.
  - Fit check (green / yellow / red with one sentence), with a stated compute allowance and a smaller-context suggestion.
  - Hugging Face client: search with cursor paging, repository detail with per-file SHA-256, token check, file resolution through two clients (the CDN client never carries the token; redirect hosts limited to `huggingface.co`, `*.hf.co` and a configured mirror), path validation that cannot escape the models folder.
  - Resumable downloads: `Range` resume validated against the published hash and size, re-resolved signed URL on every attempt, SHA-256 checked before the file is renamed into place.
  - System memory detection (Windows `GlobalMemoryStatusEx`, macOS `sysctl` + `host_statistics64`).
  - The curated catalog (`catalog/models.json`, 14 entries with pinned commits, sizes and SHA-256) and first-run selection (never auto-picks an untested entry), plus catalog Refresh verification with minisign against the updater key (tested with a locally signed fixture).
  - The list of architectures the pinned llama.cpp knows (`catalog/llama-architectures.txt`, generated by `scripts/update_llama_archs.py`), so the app warns before downloading a model it cannot load.
- Models dialog (Installed / Browse / Downloads), fit badges, and the first-run screen.
- App commands for search, repository detail with fit verdicts, downloads (pause, resume, cancel, resume after restart), installed models (import, delete, models folder), hardware info, first run and catalog refresh.

## M1 — Runtime and chat (2026-11-10)

### Added
- `crates/runtime`: the only place processes are created (Job Object with kill-on-close on Windows, own process group on Unix, scrubbed environment, no console window); the stale-child registry and the macOS reaper process; backend discovery and probing (`--list-devices`), launch arguments, the managed-child protocol, load-failure classification, `/props`; and the supervisor with restart (3 in 5 minutes), step-down (ROCm/CUDA → Vulkan → CPU) and new-port policies. Contract tests run against the real pinned llama-server.
- `crates/agent` (provider layer): OpenAI-compatible client with SSE parsing hardened against split chunks, CRLF, BOM and vendor quirks (Ollama, LM Studio, vLLM), tool-call accumulation, context-overflow and parse-error mapping, token counting, and a loopback client that never uses a proxy.
- `crates/store`: SQLite on one dedicated thread (WAL, foreign keys, FTS5 trigram search), chats with a message tree for regenerate and edit-and-resend, settings, providers, models, downloads, MCP and skills configuration, approved folders, and the append-only audit log with JSON Lines and CSV export; OS keychain secrets with chunking for Windows' 2,560-byte limit.
- App: model loading with progress, streaming chat, stop, regenerate, branch switching, edit-and-resend, history search, rename, delete, per-chat model and system prompt, tokens/s, OpenAI-compatible endpoints with keychain-stored keys, and *Settings → Diagnostics → Run backend check* (the in-app probe build: devices per backend and a timed generation on each, saved to the logs folder).
- `e2e/`: a standard-library WebDriver client and an M1 scenario that drives the real app through `tauri-driver`.

### Changed
- macOS llama.cpp source build: OpenSSL and the web UI download are turned off (the build had linked Homebrew's OpenSSL, which the bundling check rejected); the macOS installer lists CPU next to Metal so a failed Metal load steps down.
- `rusqlite_migration` replaced by a small built-in migration runner so rusqlite can move to 0.41 (its bundled SQLite fixes RUSTSEC-2026-0459).
- Windows WebView2 is started with its crash reporter disabled (see Known issues).

### Known issues
- WebView2's documented switch for not uploading crash dumps (`IsCustomCrashReportingEnabled`) is not exposed by the pinned wry 0.57; the app passes `--disable-breakpad --disable-crash-reporter` instead, which is unverified on real Windows.
- Probe results are not yet cached per driver version across restarts (7.2); they are cached for the app session. (The Vulkan-loader check and the "loaded but slow" check were added in M8.)
- Keychain code paths have only been type-checked; the first real runs are on your Windows PC and Mac.

## M0 — Scaffold (2026-11-10)

### Added
- Cargo workspace with the crates from ARCHITECTURE.md section 5 (empty except the app crate).
- Tauri 2.12 app shell:
  - Windows are created from config in Rust so navigation away from the app's own origin and `window.open` are blocked (unit-tested).
  - Two explicit capability files: `main` (core defaults, dialogs, opener, clipboard write, app commands) and `overlay` (event listen only). No shell, filesystem or HTTP plugin.
  - App commands are declared in `src-tauri/commands.list`; Tauri then requires a per-window permission for each. A test checks every command is granted to `main` and not to `overlay`.
  - Strict CSP with Trusted Types, frozen prototypes, DNS prefetch off.
  - Windows manifest declares per-monitor DPI awareness.
  - Single-instance lock; global-shortcut plugin registered (the stop hotkey arrives in M3).
- React 19 + TypeScript + Vite + Tailwind 4 frontend with Radix primitives: app shell, settings dialog with light / dark / system theme and an About section. ESLint bans HTML-string sinks (`innerHTML`, `dangerouslySetInnerHTML`, `eval`).
- CI (`.github/workflows/ci.yml`): lint, typecheck, frontend tests, rustfmt, clippy, workspace tests and cargo-deny on Linux; tests and an installer build on Windows (NSIS, unsigned) and macOS (dmg, ad-hoc signed); installers uploaded as artifacts; Windows installer size budget of 150 MB.
- Release workflow (`.github/workflows/release.yml`) on `v*` tags with tauri-action, signing secrets as placeholders that only apply when set.
- cargo-deny policy: advisories, license allow-list, banned crates.
- README, LICENSE (MIT), notices placeholder.
