# harnessUI

A local-first desktop app that gives AI models running on your own hardware the agentic features of the big desktop assistants: chat, tools, skills and MCP. Windows 10 and 11 (x64) and macOS 13.3 or later, on Apple silicon and Intel Macs.

- Works offline once a model is downloaded. Network use happens only when you start it.
- No account, no telemetry, no remote crash reporting. Logs stay on disk.
- Chats, settings and the audit log are stored locally.

**Status:** all milestones of ARCHITECTURE.md section 19 are implemented (M0–M8): local and OpenAI-compatible chat, the model manager, the agent loop with approvals and an audit log, MCP servers, files, terminal, git (with a built-in fallback), skills, a browser tool and computer use. It has been tested automatically on Linux and built and unit-tested on Windows and macOS in CI; testing on real Windows and Mac hardware is the next step (docs/MILESTONES.md has the checklists). macOS code signing and in-app updates wait for the signing keys; Windows code signing is not set up yet. See [CHANGELOG.md](CHANGELOG.md).

## What works today

- **Chat with a local model.** Pick a `.gguf` file (model picker → *Open a GGUF file…*, or *Models → Installed → Import*). The app starts llama.cpp's `llama-server` on 127.0.0.1 with a random port and key, shows load progress, and streams replies with tokens/s. Stop, regenerate (with version arrows), edit-and-resend, per-chat model and system prompt, history search, rename and delete.
- **Backends.** Windows ships Vulkan and CPU; macOS ships Metal (with CPU as the fallback). A Mac with an Intel processor runs models on the CPU by default, and Metal can be picked in *Settings → Local models*. A backend that fails to load steps down (ROCm/CUDA → Vulkan → CPU) and a crashed server restarts up to three times in five minutes. *Settings → Diagnostics → Run backend check* lists every device per backend and, when a model is loaded, times a short generation on each, and saves the report to the logs folder (*Open logs folder*).
- **OpenAI-compatible endpoints.** *Settings → Endpoints* adds Ollama, LM Studio, vLLM or any OpenAI-compatible server; API keys go to the system keychain.
- **Model manager.** *Models* (Ctrl+M) searches Hugging Face for GGUF chat models, shows every quantization with a fit verdict for your hardware, downloads with resume and SHA-256 verification, and lists installed models with badges read from the GGUF header. First run suggests a model from the bundled catalog. Vision models take image attachments; PDFs and text files can be attached too. ROCm and CUDA backends are one-click downloads in *Settings → Local models*, verified by SHA-256.
- **Tools and approvals.** Approve a folder (*Settings → Permissions*, or for one chat under *Folders for this chat* in its *Tools* panel), then ask the model to read, search or edit files in it. Every tool call shows a card; actions ask first (*Allow once*, *Allow for this chat*, *Deny*), and every call is recorded in *Settings → Audit log*. The chat's *Tools* panel switches tool groups on and off and shows what they cost in context. The terminal and git tools are off by default; git works without Git installed through a built-in library (*Settings → Git*).
- **Allow all (0.0.4).** The shield button in a chat's header switches that chat from asking before every action to *Allow all*. Deleting files (and emptying them), throwing away work, system commands, paying, sending and publishing still ask, and so does sending data out after the model has read a web page or a file in the same turn. *Settings → Permissions → Approvals* has the switch for every chat.
- **Artifacts (0.0.4).** Documents, pages, pictures and diagrams the model makes open in a panel beside the chat, with preview, source editing, versions, restore and export. Previews run in a sandbox that cannot reach the app, your files or the network, and errors in a preview go back to the model so it can fix them. A page written after the model read outside content shows without its scripts until you press *Run scripts*.
- **Web search (0.0.4).** The model searches the web and reads pages (it asks first), and cites its sources as numbered chips you can open and check against the text it was given. *Settings → Search* chooses DuckDuckGo, Brave, Tavily or your own SearXNG. Pages on this computer or a private network are never read.
- **A short tool list per task (0.0.4).** Each turn shows the model only the tools the task is about, sized to the model's context; the rest stay one lookup away. The Tools panel shows which are "shown" and which "on request".
- **MCP servers.** *Settings → MCP servers → Add servers* takes a Claude Desktop, Cursor or VS Code config. Secrets go to the system keychain; a new server's tools start off until you turn them on. See [docs/mcp-guide.md](docs/mcp-guide.md).
- **Skills.** *Settings → Skills* imports skill folders or zips (SKILL.md, as used by other agent apps), creates and edits them; imported skills start turned off. The model loads a skill when it needs it. How to write one: [docs/skills-guide.md](docs/skills-guide.md).
- **Browser.** The model drives a browser window of its own with a separate profile (never your own cookies or logins). It uses Chrome, Edge, Brave, Vivaldi or Chromium if one is installed; on a Mac with none of these, *Settings → Browser* can download one for the agent (Google's Chrome for Testing, about 200 MB, checked against a checksum built into the app). The Browser tools are on in a new chat when a browser is available and the model's context is at least 8,192 tokens; otherwise they are one switch away in the chat's *Tools* panel. Leaving a site and submitting forms always ask; this computer's own addresses and, by default, private networks are blocked.
- **Computer use (Windows and macOS).** Switch on *Computer* under *Tools* in a chat and a model that can see images can take screenshots and use the mouse and keyboard. There is nothing to set up per model: the app works out from the model's name how it writes screen positions, and *Settings → Computer use* lets you choose another way if clicks land in the wrong place. Every kind of input asks first for each application, input to harnessUI itself is always refused, and a red border with a Stop button stays on screen while it acts. On a Mac it needs the Screen Recording and Accessibility permissions.
- **Subagents.** Switch on *Subagents* under *Tools* in a chat and the model can hand a self-contained task (reading many files, checking several pages) to a helper that works in its own fresh context and returns only its answer, which keeps the main chat's context small. The helper has the same tools and asks for the same approvals; its cards start with "Subagent:". Off in new chats.
- **Stop.** The Stop button, Esc, or the global hotkey (Ctrl+Alt+Backspace on Windows, Ctrl+Option+Delete on macOS) stops the reply and cancels any running tool.

## Download

Open the repository's **Releases** page and download the file for your computer from the latest release: the `-setup.exe` for Windows, or the `_universal.dmg` for any Mac. Versions 0.0.x are early test builds.

Builds of work in progress: every push also builds both installers in GitHub Actions. Open the **Actions** tab, pick the latest green **CI** run, and under **Artifacts** download `windows-installer` or `macos-dmg` (a zip that contains the `-setup.exe` or the `.dmg`).

What you need:

- **Windows:** Windows 10 or 11, 64-bit (x64). The app needs Microsoft's WebView2, which is part of Windows 11 and of up-to-date Windows 10; if it is missing, the installer downloads it (this needs an internet connection once).
- **macOS:** macOS 13.3 or later. There is one disk image for every Mac, with Apple silicon or an Intel processor, so you do not need to know which one you have. Macs with an Intel processor run models on the CPU by default, which is slower; Metal can be picked in *Settings → Local models* and is worth trying on a Mac with a Radeon graphics card.

### Opening unsigned builds

Builds are not code-signed yet (see [Signing](#signing)).

- **Windows:** SmartScreen shows "Windows protected your PC". Click **More info → Run anyway**. If **Smart App Control** is on (Windows 11; Windows Security → App & browser control), unsigned apps are blocked with no way through. Turning it off is permanent unless you reinstall Windows.
- **macOS:** the build is ad-hoc signed. Right-click the app → **Open**, or go to **System Settings → Privacy & Security → Open Anyway**. If macOS says the app is damaged, run `xattr -dr com.apple.quarantine /Applications/harnessUI.app`.

## Build from source

Prerequisites:

- Rust stable (1.95 or newer) and Node.js 22.
- **Windows:** Visual Studio Build Tools with the C++ workload. WebView2 is part of Windows 11 and of up-to-date Windows 10.
- **macOS:** Xcode Command Line Tools and CMake (for the llama.cpp build below).
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

On a Mac, the installer build is universal (Apple silicon and Intel in one app). Add both Rust targets, build llama.cpp for both processors from the pinned commit (into `src-tauri/binaries/`), then build:

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
```

```bash
python3 scripts/fetch_llama.py macos
```

```bash
npx tauri build --target universal-apple-darwin
```

Tests and checks, the same ones CI runs:

```bash
npm run lint && npm run typecheck && npm test
```

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

For development without an installer on Linux, unpack the pinned CPU build of `llama-server` (it goes to `.cache/llama-linux/`, and the script prints that path), then point the app at it:

```bash
python3 scripts/fetch_llama.py linux
```

```bash
APP_DEV_LLAMA_DIR="$PWD/.cache/llama-linux" npx tauri dev
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
| `crates/tools` | Built-in tools (files, terminal, git, skills, PDF text extraction worker) |
| `crates/browser` | Built-in browser tool: the agent's own Chrome or Edge over CDP, with request checks |
| `crates/computer` | Built-in computer-use tool: screen capture, input injection and the input guard |
| `crates/agent` | Providers, agent loop, tool-call parsing, context management |

The app's display name is `productName` in `src-tauri/tauri.conf.json`; everything else reads it from there.

## Signing

Release builds (`.github/workflows/release.yml`, on `v*` tags) fetch the pinned llama.cpp runtime, build both installers into a draft GitHub release, and are prepared for signing. Add these repository secrets when you have them; until then builds are unsigned.

| Secret | For |
|---|---|
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` | macOS Developer ID signing |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | macOS notarization |
| `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Update and catalog signatures (generate once with `npx tauri signer generate`, and back the key up; losing it means installed copies can never update) |

**In-app updates** need the updater key pair:

1. On your own machine run `npx tauri signer generate -w ~/.tauri/harnessui.key`, choose a password, and keep both the key file and the password in your password manager.
2. Add the private key's contents and its password as the `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets.
3. Commit the public key (the `.pub` file's contents) as `plugins.updater.pubkey` in `src-tauri/tauri.updater.conf.json`.

Release builds then include signed update files and `latest.json`. The release workflow also signs `catalog/models.json` and attaches it with `models.json.minisig`, which *Settings → General → Model catalog → Refresh catalog* downloads. A last job fails the run if the draft release is missing an installer or, with the key set, a platform in `latest.json` or the catalog. *Settings → General → Updates* offers *Check now* and an opt-in automatic check. Builds without the key show that they cannot update themselves. The app never checks for updates unless you click *Check now* or turn the automatic check on.

Windows code signing is not set up yet, so the Windows installer is unsigned. Once you have chosen a certificate route, add a separate config file that holds only `bundle.windows.signCommand` (for example `src-tauri/tauri.winsign.conf.json`), and pass it with `--config` in the Windows build of `release.yml`, only when that route's secrets are set.

## License

MIT. Third-party notices: [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
