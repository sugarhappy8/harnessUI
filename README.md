# harnissUI

A local-first desktop app that gives AI models running on your own hardware the agentic features of the big desktop assistants: chat, tools, skills and MCP. Windows 10/11 x64 and macOS 13.3+ on Apple Silicon.

- Works offline once a model is downloaded. Network use happens only when you start it.
- No account, no telemetry, no remote crash reporting. Logs stay on disk.
- Chats, settings and the audit log are stored locally.

**Status:** under construction. Chat with a local GGUF model or an OpenAI-compatible endpoint works (M1); the model manager and first-run flow are in progress (M2); the agent loop with tools, approvals, MCP servers and the audit log works (M3). See [CHANGELOG.md](CHANGELOG.md) for what each milestone delivered and [ARCHITECTURE.md](ARCHITECTURE.md) section 19 for the plan.

## What works today

- **Chat with a local model.** Pick a `.gguf` file (model picker → *Open a GGUF file…*, or *Models → Installed → Import*). The app starts llama.cpp's `llama-server` on 127.0.0.1 with a random port and key, shows load progress, and streams replies with tokens/s. Stop, regenerate (with version arrows), edit-and-resend, per-chat model and system prompt, history search, rename and delete.
- **Backends.** Windows ships Vulkan and CPU; macOS ships Metal (with CPU as the fallback). A backend that fails to load steps down (ROCm/CUDA → Vulkan → CPU) and a crashed server restarts up to three times in five minutes. *Settings → Diagnostics → Run backend check* lists every device per backend and times a short generation on each, and saves the report to the logs folder.
- **OpenAI-compatible endpoints.** *Settings → Endpoints* adds Ollama, LM Studio, vLLM or any OpenAI-compatible server; API keys go to the system keychain.
- **Model manager (in progress).** *Models* (Ctrl+M) searches Hugging Face for GGUF chat models, shows every quantization with a fit verdict for your hardware, downloads with resume and SHA-256 verification, and lists installed models with badges read from the GGUF header. First run suggests a model from the bundled catalog.

- **Tools and approvals.** Approve a folder (*Settings → Permissions*), then ask the model to read, search or edit files in it. Every tool call shows a card; actions ask first (*Allow once*, *Allow for this chat*, *Deny*), and every call is recorded in *Settings → Audit log*. The chat's *Tools* panel switches tool groups on and off and shows what they cost in context. The terminal and git tools exist but are off by default.
- **MCP servers.** *Settings → MCP servers → Add servers* takes a Claude Desktop, Cursor or VS Code config. Secrets go to the system keychain; a new server's tools start off until you turn them on.
- **Stop.** The Stop button, Esc, or the global hotkey (Ctrl+Alt+Backspace on Windows, Ctrl+Option+Delete on macOS) stops the reply and cancels any running tool.

## Download a test build

Every push builds a Windows installer and a macOS disk image in GitHub Actions.

1. Open the repository's **Actions** tab and pick the latest green **CI** run for the branch.
2. Under **Artifacts**, download `windows-installer` or `macos-dmg`.
3. Unzip the artifact; it contains the `-setup.exe` or the `.dmg`.

### Opening unsigned builds

Builds are not code-signed yet (see [Signing](#signing)).

- **Windows:** SmartScreen shows "Windows protected your PC". Click **More info → Run anyway**. If **Smart App Control** is on (Windows 11; Windows Security → App & browser control), unsigned apps are blocked with no way through. Turning it off is permanent unless you reinstall Windows.
- **macOS:** the build is ad-hoc signed. Right-click the app → **Open**, or go to **System Settings → Privacy & Security → Open Anyway**. If macOS says the app is damaged, run `xattr -dr com.apple.quarantine /Applications/harnissUI.app`.

## Build from source

Prerequisites:

- Rust stable (1.95 or newer) and Node.js 22.
- **Windows:** Visual Studio Build Tools with the C++ workload. WebView2 ships with Windows 10/11.
- **macOS:** Xcode Command Line Tools.
- **Linux (development only, not a release target):** `sudo apt-get install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev`.

```bash
npm ci
```

```bash
npx tauri dev
```

```bash
npx tauri build
```

Tests and checks, the same ones CI runs:

```bash
npm run lint && npm run typecheck && npm test
```

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

For development without an installer, point the app at a folder that holds `llama-server` (for example one unpacked by `python3 scripts/fetch_llama.py`):

```bash
APP_DEV_LLAMA_DIR=/path/to/llama-folder npx tauri dev
```

Contract tests against the real llama-server run when `LLAMA_SERVER_BIN` and `TEST_GGUF` are set (`python3 scripts/fetch_test_model.py` downloads the pinned test model). End-to-end tests that drive the real app are described in [e2e/README.md](e2e/README.md).

## Architecture in one paragraph

A Tauri 2 app: a Rust core and a React + TypeScript webview with no privileges. The core runs llama.cpp's `llama-server` as a child process bound to 127.0.0.1, talks to it (or to any OpenAI-compatible endpoint you add) over HTTP, and exposes every tool, built-in or user-added, through MCP. Every tool call passes one policy engine that shows an approval card before anything runs. The details, the security model and the decisions behind them are in [ARCHITECTURE.md](ARCHITECTURE.md); the research behind it is in [docs/research/](docs/research/README.md).

| Path | What |
|---|---|
| `src/` | React frontend |
| `src-tauri/` | Tauri app: windows, capabilities, commands |
| `crates/policy` | Path sandbox, permission rules, command classifier |
| `crates/store` | SQLite storage, audit log, keychain |
| `crates/runtime` | Process spawning and llama-server lifecycle |
| `crates/models` | GGUF reader, hardware detection, Hugging Face downloads |
| `crates/mcp-host` | MCP connections and tool registry |
| `crates/tools` | Built-in tools (files, terminal, git, browser, computer use, skills) |
| `crates/agent` | Providers, agent loop, tool-call parsing, context management |

The app's display name is `productName` in `src-tauri/tauri.conf.json`; everything else reads it from there.

## Signing

Release builds (`.github/workflows/release.yml`, on `v*` tags) are prepared for signing. Add these repository secrets when you have them; until then builds are unsigned.

| Secret | For |
|---|---|
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` | macOS Developer ID signing |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | macOS notarization |
| `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Update and catalog signatures (generate once with `npx tauri signer generate`, and back the key up; losing it means installed copies can never update) |

Windows code signing uses a `signCommand` added in M8 once you have chosen a certificate route.

## License

MIT. Third-party notices: [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
