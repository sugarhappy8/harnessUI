# harnessUI

A local-first desktop app that gives AI models running on your own hardware the agentic features of the big desktop assistants: chat, tools, skills and MCP. Windows 10 and 11 (x64) and macOS 13.3 or later, on Apple silicon and Intel Macs.

- Works offline once a model is downloaded. Network use happens only when you start it, apart from the check for updates: shortly after it starts and every 6 hours, harnessUI asks github.com whether a newer version is out. *Settings → General → Updates* turns that off.
- No account, no telemetry, no remote crash reporting. Logs stay on disk.
- Chats, settings and the audit log are stored locally.

**Status:** all milestones of ARCHITECTURE.md section 19 are implemented (M0–M8): local and OpenAI-compatible chat, the model manager, the agent loop with approvals and an audit log, MCP servers, files, terminal, git (with a built-in fallback), skills, a browser tool and computer use. It has been tested automatically on Linux and built and unit-tested on Windows and macOS in CI; testing on real Windows and Mac hardware is the next step (docs/MILESTONES.md has the checklists). From 0.0.8 harnessUI updates itself; the first release needs the owner's updater key, made once (see [Signing](#signing)). macOS code signing waits for Apple's signing keys, and Windows code signing is not set up yet. See [CHANGELOG.md](CHANGELOG.md).

## What works today

- **Chat with a local model.** Pick a `.gguf` file (model picker → *Open a GGUF file…*, or *Models → Installed → Import*). The chat's model list shows every installed model, and a new download appears in it at once. The app starts llama.cpp's `llama-server` on 127.0.0.1 with a random port and key, shows load progress, and streams replies with tokens/s. Stop, regenerate (with version arrows), edit-and-resend, per-chat model and system prompt, history search, rename and delete.
- **Backends.** Windows ships Vulkan and CPU; macOS ships Metal (with CPU as the fallback). A Mac with an Intel processor runs models on the CPU by default, and Metal can be picked in *Settings → Local models*. When a graphics card runs out of memory, the app first tries again on the same card (without *Save memory*, then with part of the model on the processor); a backend that still fails steps down (ROCm/CUDA → Vulkan → CPU), and a crashed server restarts up to three times in five minutes. *Settings → Diagnostics → Run backend check* lists every device per backend and, when a model is loaded, times a short generation on each, and saves the report to the logs folder (*Open logs folder*).
- **Endpoints and cloud AI companies.** *Settings → Endpoints* adds Ollama, LM Studio, vLLM or any OpenAI-compatible server, and since 0.0.7 has ready-made choices for Anthropic (Claude, through Anthropic's own Messages API), OpenAI, OpenRouter and abliteration.ai: pick one and paste your API key. Keys go to the system keychain and only to their own endpoint's address, over plain http:// only to this computer or a local network IP address. A *Cloud* mark beside the model's name says when a chat's messages and files go to a server on the internet (any address but this computer and local network IP addresses). On *Automatic*, a long chat with a cloud model sends at most the company's size with each message (200,000 tokens for Claude); more is a choice under *Edit*.
- **Model manager.** *Models* (Ctrl+M) searches Hugging Face for GGUF chat models, shows every quantization with a fit verdict for your hardware, downloads with resume and SHA-256 verification, and lists installed models with badges read from the GGUF header. First run suggests a model from the bundled catalog. Vision models take image attachments; PDFs and text files can be attached too. Since 0.0.7 the memory a model needs is counted the way llama.cpp sets it up (models that look back only a short way in most layers, such as Gemma, need far less than before), an image file put next to a model is used, a broken one no longer stops the model from loading, and *Installed* offers *Download the image file* for a model downloaded without one. ROCm and CUDA backends are one-click downloads in *Settings → Local models*, verified by SHA-256.
- **Settings for each model (0.0.6).** The gear beside a model in the chat's model list or in *Models → Installed* opens *Settings for this model*, kept per model file: *How much it can remember* (*Automatic* picks the largest size that fits this computer), *Use the graphics card*, *Save memory*, *Keep the model in memory*, and under *More options* the graphics card and *How long it may think*. *Settings → Local models* holds *Defaults for all models*. The header says how much the loaded model remembers and has *Reload*, *Model settings…* and *Unload to free memory*. When a chat gets longer than the model can remember, older messages are summarised automatically, and the chat shows where.
- **Fine control and computer usage (0.0.6).** A line under the model name shows how busy the graphics card and the processor are and how much memory is in use, read on this computer only. *Settings for this model* shows *Where the model goes* (graphics card and computer memory) before you load, and a closed *Fine control* section chooses the layers on the graphics card, expert parts on the processor, where the conversation memory lives, processor threads, fast attention and the memory to leave free.
- **Compare models (0.0.6).** *Models → Compare* puts up to 4 models side by side, with plain questions ("What is it good at?", "Will it fit on this computer?") and where each answer comes from, for example a quote from the publisher's model card, what the uploader lists on Hugging Face, or your own test results.
- **Tools and approvals.** Approve a folder (*Settings → Permissions*, or for one chat under *Folders for this chat* in its *Tools* panel), then ask the model to read, search or edit files in it. Every tool call shows a card; actions ask first (*Allow once*, *Allow for this chat*, *Deny*), and every call is recorded in *Settings → Audit log*. The chat's *Tools* panel switches tool groups on and off and shows what they cost in context. The terminal and git tools are off by default; git works without Git installed through a built-in library (*Settings → Git*). Since 0.0.6 a chip in the chat header shows the folder the chat works in ("No folder · Choose" when there is none, "Whole computer" in Full access), with a menu to add, remove or switch folders.
- **Allow all (0.0.4).** The shield button in a chat's header switches that chat from asking before every action to *Allow all*. Deleting files (and emptying them), throwing away work, system commands, paying, sending and publishing still ask, and so does sending data out after the model has read a web page or a file in the same turn. Since 0.0.6, starting Claude Code, Codex or a similar coding assistant asks too. *Settings → Permissions → Approvals* has the switch for every chat.
- **Full access (0.0.5).** The same header button has a third mode: nothing asks. The model can run any command and read, create, move and delete files and folders anywhere on the computer, with tools to organise folders. The app's own data and the system's key stores stay out of reach of its file tools (a program it runs has your full rights). If every chat is set to start in Full access, the app asks again at each start. It asks for confirmation with a plain warning first, and *Settings → Permissions → Approvals* can make it the mode every chat starts in. See [SECURITY.md](SECURITY.md) for what it means.
- **Artifacts (0.0.4).** Documents, pages, pictures and diagrams the model makes open in a panel beside the chat, with preview, source editing, versions, restore and export. Previews run in a sandbox that cannot reach the app, your files or the network, and errors in a preview go back to the model so it can fix them. A page written after the model read outside content shows without its scripts until you press *Run scripts*.
- **Web search (0.0.4).** The model searches the web and reads pages (it asks first), and cites its sources as numbered chips you can open and check against the text it was given. *Settings → Search* chooses DuckDuckGo, Brave, Tavily or your own SearXNG. Pages on this computer or a private network are never read.
- **A short tool list per task (0.0.4).** Each turn shows the model only the tools the task is about, sized to the model's context; the rest stay one lookup away. The Tools panel shows which are "shown" and which "on request". Since 0.0.7 the model always gets a list of every tool that is on in the chat and how to run the ones a turn doesn't show, plus a short guide for writing tool calls and the shell commands of your system (Windows PowerShell 5.1 or zsh), and a failed command made with a common mistake ends with a one-line hint. *Settings → General → Same call in a row* sets how many identical calls in a row end a turn (5 by default).
- **Tool errors and long output (0.0.4).** A failed tool call tells the model why and what to call instead; an edit whose text appears twice lists both places. Long results are shortened for the model and saved, the model can read more by line range or by word, and the tool card's *Full output* opens the whole text.
- **Task (0.0.4).** The *Task* tab beside the chat shows the goal, steps, next action, changed files and the checks that ran. You can correct it, pause, resume after a restart, mark it done or clear it.
- **Mentions (0.0.4).** Type `@` in the message box to send a file or a function, class or type from your approved folders with your message (eleven languages). Mentions read only what the file tools could read.
- **Pinned instructions and context (0.0.4).** The *Context* tab keeps up to 20 instructions in front of the model for one chat or every chat, and shows exactly what the next message would send, with token counts, without sending it.
- **Tested settings and benchmarks (0.0.4).** *Settings → Local models → Tested settings* tries a model on three short file tasks and keeps the settings that work best for it. *Settings → Diagnostics → Coding benchmarks* runs eleven small coding tasks and compares runs. A benchmark runs the model's code on your computer with your rights and is not sandboxed: use it only with a model you trust.
- **MCP servers.** *Settings → MCP servers → Add servers* takes a Claude Desktop, Cursor or VS Code config. Secrets go to the system keychain; a new server's tools start off until you turn them on. See [docs/mcp-guide.md](docs/mcp-guide.md).
- **Skills.** *Settings → Skills* imports skill folders or zips (SKILL.md, as used by other agent apps), creates and edits them; imported skills start turned off. The model loads a skill when it needs it. How to write one: [docs/skills-guide.md](docs/skills-guide.md).
- **Browser.** The model drives a browser window of its own with a separate profile (never your own cookies or logins). It uses Chrome, Edge, Brave, Vivaldi or Chromium if one is installed; on a Mac with none of these, *Settings → Browser* can download one for the agent (Google's Chrome for Testing, about 200 MB, checked against a checksum built into the app). The Browser tools are on in a new chat when a browser is available and the model's context is at least 8,192 tokens; otherwise they are one switch away in the chat's *Tools* panel. Leaving a site and submitting forms always ask; this computer's own addresses and, by default, private networks are blocked. Since 0.0.7 the browser tool *browser_capture_site* saves a copy of a website's look (the page with its scripts removed, style sheets with their animations, pictures, fonts, screenshots and a report) into a folder you approved, so the model can study the design and rebuild it with your own content; it asks first, and the copy runs nothing when you open it.
- **Computer use (Windows and macOS).** Switch on *Computer* under *Tools* in a chat and a model that can see images can take screenshots and use the mouse and keyboard. There is nothing to set up per model: the app works out from the model's name how it writes screen positions, and *Settings → Computer use* lets you choose another way if clicks land in the wrong place. Every kind of input asks first for each application, input to harnessUI itself is always refused, and a red border with a Stop button stays on screen while it acts. Since 0.0.7 harnessUI minimises its own window when it is in the way of approved input and comes back for each approval card, at the end of the turn and on Stop. On a Mac it needs the Screen Recording and Accessibility permissions.
- **Other apps can use your model (0.0.6, experimental).** *Settings → Other apps → Let other apps on this computer use the loaded model* lets Claude Code, Codex and the Claude app use the model loaded here, through a small proxy on 127.0.0.1 that needs a key kept in the system keychain. The page has setup steps for Windows and Mac with Copy buttons; the Claude app's are "Not tried yet". Claude Code and Codex send long instructions, so the model should remember at least 32,768 tokens (the page offers to set it). Off by default.
- **Claude Code and Codex from a chat (0.0.6, experimental).** Switch on *Coding apps (experimental)* under *Tools* in a chat and the local model can hand a task to Claude Code or Codex installed on this computer. Each hand-off asks first in Ask first and Allow all; in Full access it runs without asking. It runs online with your account and plan, and what the program reads is sent to Anthropic (Claude Code) or OpenAI (Codex). Off in every new chat.
- **Reverse engineering (0.0.6).** *Settings → Reverse engineering* installs rea, a free tool that looks inside program files, with a decompiler: Ghidra (with Java) on Windows, Kuna on a Mac. Nothing is downloaded until you click *Download* (Windows: about 80 MB for rea and 775 MB more for Ghidra and Java; Mac: about 120 MB for rea and Kuna). On a Mac, install Apple's command line tools first with the tab's *Install* button. Then switch on *Reverse engineering* under *Tools* in a chat and ask about a program in one of its folders, or drop the file on the message box. rea reads files and never runs them. Ghidra's download is checked against the checksum GitHub publishes for it, and every release build checks that checksum again. See [SECURITY.md](SECURITY.md) for what it can reach.
- **Subagents.** Switch on *Subagents* under *Tools* in a chat and the model can hand a self-contained task (reading many files, checking several pages) to a helper that works in its own fresh context and returns only its answer, which keeps the main chat's context small. The helper has the same tools and asks for the same approvals; its cards start with "Subagent:". Off in new chats.
- **Stop.** The Stop button, Esc, or the global hotkey (Ctrl+Alt+Backspace on Windows, Ctrl+Option+Delete on macOS) stops the reply and cancels any running tool.

## Studio: pictures and short videos (0.1.0)

Studio makes pictures and short videos on this computer, free and private, with cloud models (OpenRouter, OpenAI) as the backup for computers without a good graphics card. Open it with the sparkle button in the sidebar or Ctrl+G (⌘G on a Mac), type what you want to see and click *Create*.

- **Getting local models.** *Get local models* in Studio lists the models recommended for this computer, under Pictures and Video: FLUX.2 klein 4B and Z-Image Turbo for pictures and a fast version of Wan 2.2 5B for video, all free for any use, including commercial. You can also search Hugging Face for other picture and video models: Studio offers the ones it knows how to run, adds the extra files they need, and says "Not supported yet" for the rest. On Windows the Studio engine (about 29 MB) downloads with your first model; on a Mac it is built in. Making a picture or video here sends nothing anywhere.
- **What fits.** Each model says whether it fits this computer: *Fits*, *Fits after the chat model is unloaded*, *Slow here* or *Not for this computer*. When a picture or video model needs the graphics card the chat model is using, Studio unloads the chat model to make room and offers *Load it again*; *Settings → Studio* can make it ask first, or make pictures on the processor instead. Video needs a graphics card or a Mac with Apple silicon. How long a run takes is shown after your first run here. When nothing fits, or a run here would be very slow, Studio suggests a cloud model.
- **Model downloads.** Studio shows the files, sizes and publisher licence links for compatible models, including non-commercial models and models tagged for mature audiences. Click *Download* to confirm. The first download also shows a one-time note about local models. For a gated repository, request access on Hugging Face and use a token with access in harnessUI.
- **Cloud models.** With an OpenRouter or OpenAI key saved in *Settings → Endpoints*, Studio can also use their picture models and OpenRouter's video models. Your prompt and the pictures you add then go to that company, the line above *Create* says where, and *Create* shows the price. Studio asks before every cloud video and before any run of $1 or more or with an unknown price, and it stops at a daily limit of $10 of cloud spending, which is on by default and can be changed in *Settings → Studio*. Runs on this computer cost nothing and never count.
- **Where files go.** What you make is saved in the `harnessUI/studio` folder in your home folder (*Settings → Studio* can change it), by month, with a small record of each run's settings beside the files. Pictures and videos made on this computer carry a label in the file saying they were made by AI, and never your prompt. *Send to chat* puts a picture into a chat, and a picture in a chat has *Open in Studio*. Esc and the stop hotkey never stop a Studio run; the buttons on its card do.

## Download

Go to **[harnessui.site](https://harnessui.site)** and click **Download**. It offers the installer for your computer first, from the latest release. Or pick the link for your computer; both always give the newest version:

- **Windows:** [harnessUI-setup.exe](https://github.com/sugarhappy8/harnessUI/releases/latest/download/harnessUI-setup.exe)
- **Mac (Apple silicon or Intel):** [harnessUI-universal.dmg](https://github.com/sugarhappy8/harnessUI/releases/latest/download/harnessUI-universal.dmg)

The repository's **Releases** page has the same files, also under names with the version in them (`harnessUI_<version>_x64-setup.exe` and `harnessUI_<version>_universal.dmg`). Releases before 0.0.8 have only those, so until 0.0.8 is published the two links above lead to GitHub's "Not Found" page and harnessui.site says *Coming soon*; take the installer from the Releases page instead. Versions 0.0.x are early test builds.

**Updates.** From 0.0.8 on, harnessUI updates itself. Shortly after it starts, and every 6 hours while it runs, it asks github.com whether a newer version is out. If there is one, it downloads it in the background and checks its signature. When it is ready, a small box appears in the window: "Update ready", with *Restart and update*, *Later* and *What's new*. *Restart and update* closes the model and any running tools, installs the new version and opens harnessUI again. *Later* hides the box until the next time you open harnessUI. Nothing is installed until you choose to. On a Mac, drag harnessUI into the Applications folder and open it from there: a copy run from the disk image or straight from Downloads cannot update itself, and *Restart and update* says so. To stop the automatic check, untick *Check for updates automatically* in *Settings → General → Updates*; *Check now* there still works.

**Have 0.0.7 or older?** Those versions cannot update themselves. Download 0.0.8 as above and install it once by hand. After that, updates come by themselves.

Builds of work in progress: every push also builds both installers in GitHub Actions. Open the **Actions** tab, pick the latest green **CI** run, and under **Artifacts** download `windows-installer` or `macos-dmg` (a zip that contains the `-setup.exe` or the `.dmg`). These builds cannot update themselves.

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
- **macOS:** Xcode Command Line Tools and CMake (for the llama.cpp and Studio engine builds below).
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

On a Mac, the installer build is universal (Apple silicon and Intel in one app). Add both Rust targets, build llama.cpp and the Studio engine (`sd-cli`) for both processors from their pinned commits (into `src-tauri/binaries/`), then build:

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
```

```bash
python3 scripts/fetch_llama.py macos
```

```bash
python3 scripts/fetch_sd.py macos
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

For Studio's local pictures and videos on Linux, build a processor-only Studio engine from the pinned commit (it goes to `.cache/sd-linux/`; this needs CMake and a C++ compiler) and point the app at it with `APP_DEV_SD_DIR` as well:

```bash
python3 scripts/fetch_sd.py linux
```

```bash
APP_DEV_SD_DIR="$PWD/.cache/sd-linux" APP_DEV_LLAMA_DIR="$PWD/.cache/llama-linux" npx tauri dev
```

Contract tests against the real llama-server run when `LLAMA_SERVER_BIN` and `TEST_GGUF` are set (`python3 scripts/fetch_test_model.py` downloads the pinned test model), and against the Studio engine when `SD_CLI_BIN` names the `sd-cli` built above. Tests against the real rea, Kuna and Ghidra run when `REVERSE_PACK_CACHE` names a folder filled by `python3 -I scripts/pin_reverse.py --download-to DIR`. End-to-end tests that drive the real app are described in [e2e/README.md](e2e/README.md).

## Architecture in one paragraph

A Tauri 2 app: a Rust core and a React + TypeScript webview with no privileges. The core runs llama.cpp's `llama-server` as a child process bound to 127.0.0.1, talks to it (or to any OpenAI-compatible endpoint you add) over HTTP, and exposes every tool, built-in or user-added, through MCP. Every tool call passes one policy engine that shows an approval card before anything runs. The details, the security model and the decisions behind them are in [ARCHITECTURE.md](ARCHITECTURE.md); the research behind it is in [docs/research/](docs/research/README.md).

| Path | What |
|---|---|
| `src/` | React frontend |
| `src-tauri/` | Tauri app: windows, capabilities, commands |
| `crates/policy` | Path sandbox, permission rules, command classifier |
| `crates/store` | SQLite storage, audit log, keychain |
| `crates/runtime` | Process spawning, llama-server lifecycle, and the Studio engine (`sd-cli`) |
| `crates/models` | GGUF reader, hardware detection, Hugging Face downloads |
| `crates/mcp-host` | MCP connections and tool registry |
| `crates/tools` | Built-in tools (files, terminal, git, skills, PDF text extraction worker) |
| `crates/browser` | Built-in browser tool: the agent's own Chrome or Edge over CDP, with request checks |
| `crates/computer` | Built-in computer-use tool: screen capture, input injection and the input guard |
| `crates/agent` | Providers, agent loop, tool-call parsing, context management, and Studio's cloud picture and video companies |
| `crates/reverse` | Reverse engineering: rea, Kuna and Ghidra downloads, the allow-listed `reverse` tools and their path checks |
| `crates/usage` | How busy the graphics card and processor are, read on this computer |

The app's display name is `productName` in `src-tauri/tauri.conf.json`; everything else reads it from there, except the download names `harnessUI-setup.exe` and `harnessUI-universal.dmg` (`STABLE_WINDOWS` and `STABLE_MAC` in `release.yml`), which harnessui.site and the links under [Download](#download) use. They stay the same on purpose when the app is renamed.

## Signing

Release builds (`.github/workflows/release.yml`, on `v*` tags) fetch the pinned llama.cpp runtime, build the Studio engine for the Mac, build both installers into a draft GitHub release, and are prepared for code signing. The updater key is required for every release (see *In-app updates* below). Add the Apple secrets when you have them; until then builds are not code-signed.

| Secret | For |
|---|---|
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` | macOS Developer ID signing |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | macOS notarization |
| `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Update and catalog signatures. Required: no release is built without them. Made once with `scripts/setup_updater.py` (below); back the key up, because losing it means installed copies can never update |

**In-app updates** need the updater key pair, and every release needs it: the release workflow stops before it builds anything when the `TAURI_SIGNING_PRIVATE_KEY` secret is missing or `src-tauri/tauri.updater.conf.json` has no public key. Make the key once, on your own computer:

1. In the repository folder run `npm ci`, then `python3 -I scripts/setup_updater.py` (on Windows: `py scripts\setup_updater.py` or `python scripts\setup_updater.py`). It runs the Tauri CLI's `tauri signer generate`, which asks you to choose a password, saves the key as `~/.tauri/harnessui.key` (`--key PATH` puts it somewhere else) and writes the public key into `src-tauri/tauri.updater.conf.json`. If the GitHub CLI (`gh`) is installed and signed in, it offers to save the two secrets for you; otherwise it says where to add them.
2. Keep the key file and its password in your password manager. If you lose either one, installed copies can never update again.
3. Commit `src-tauri/tauri.updater.conf.json` and push it. Then release: *Actions → Release → Run workflow* on the commit to release, or push the tag `v<version>`.

The script never overwrites a key file, never prints the private key, and does nothing when a public key is already committed, because a new key would leave every installed copy unable to update. Without the script: run `npx tauri signer generate -w ~/.tauri/harnessui.key` and choose a password; add the key file's contents and the password as the `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets (*Settings → Secrets and variables → Actions* on GitHub); and put the `.pub` file's contents, unchanged, into `plugins.updater.pubkey` in `src-tauri/tauri.updater.conf.json`.

Every release then includes signed update files, `latest.json`, and a copy of each installer under the name without the version (`harnessUI-setup.exe`, `harnessUI-universal.dmg`), which harnessui.site and the links under [Download](#download) use. The release page shows the version's entry from `CHANGELOG.md`, followed by where to download and how to open the unsigned builds; the app's update box shows the same entry under *What's new*, as plain text and without the download part. A version without an entry is not built. The entry is published as it is, so write it for the people who download harnessUI. The release workflow also signs `catalog/models.json` and attaches it with `models.json.minisig`, which *Settings → General → Model catalog → Refresh catalog* downloads. A last job keeps the release a draft if an installer or its copy is missing, if a copy is not the same file as its installer, if `latest.json` lacks Windows, Apple silicon or Intel Macs, or if the catalog is missing.

In the app, the automatic check is on unless you turn it off: *Settings → General → Updates* has *Check for updates automatically* (it contacts github.com), *Check now*, a line that says what the last check found, and *Restart and update* when an update is ready. A build made without the key, such as a build from source or a CI build, cannot update itself and says so.

**Reverse-engineering downloads** are pinned in `reverse.lock.json`, written and checked by `scripts/pin_reverse.py`. The release workflow runs `python3 -I scripts/pin_reverse.py --check --release` and reads Ghidra's checksum on GitHub again (`github-digest --part ghidra`, with the workflow's own token) before it builds anything, and stops if either fails; CI runs `--check` and the same comparison. Ghidra's checksum was recorded from what CI run 43380365172 read: CI keeps GitHub's answer as the artifact `ghidra-release-record`, and `python3 -I scripts/pin_reverse.py github-digest --part ghidra --from-release <that file> --record` records it, with the run's address, only when the file names a CI run of this repository.

Windows code signing is not set up yet, so the Windows installer is unsigned. Once you have chosen a certificate route, add a separate config file that holds only `bundle.windows.signCommand` (for example `src-tauri/tauri.winsign.conf.json`), and pass it with `--config` in the Windows build of `release.yml`, only when that route's secrets are set.

## License

MIT. Third-party notices: [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
