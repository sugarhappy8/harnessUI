# Build [harnessUI]: a local-first agentic desktop app for local AI models (owner's spec, verbatim)

## Goal
Build a cross-platform desktop app (Windows .exe installer, macOS .dmg) that gives local AI models the agentic capabilities of the Claude, ChatGPT and Kimi desktop apps: chat, tools, skills and MCP. It is for people who run models on their own hardware. Keep the app name in one config constant so it's easy to change.

Non-negotiable:
- Works fully offline once a model is downloaded. Network use happens only when the user starts it: Hugging Face search and downloads, git clone/pull, the browser tool, remote endpoints and MCP servers the user adds, and an opt-in update check.
- No account, no telemetry, no remote crash reporting. Logs stay on disk.
- All user data (chats, settings, audit log) is stored locally.

## How to work
1. Read this whole spec first. Ask me up to 5 clarifying questions where a wrong guess would be expensive. Then write ARCHITECTURE.md (components, data flow, process model, security model) and wait for my approval before writing app code.
2. Build in the milestones listed at the end. After each one, stop and give me: what works, how to test it manually, and known issues.
3. Never stub silently. If something is incomplete, mark it with a TODO and list it in the milestone summary.
4. Justify every new dependency in one line. Prefer well-maintained, widely used crates and packages.

## Platforms and test hardware
- Windows 10/11 x64 → NSIS .exe installer. macOS 13+ on Apple Silicon → .dmg. Intel Macs are out of scope for v1.
- GPU acceleration: Metal on macOS. On Windows, ship the llama.cpp Vulkan build as the default (it works on AMD, NVIDIA and Intel), an optional CUDA build for NVIDIA, and a CPU fallback. Auto-detect the right one, and let the user override it in settings.
- My test machines: [Windows: Ryzen 9 7900X, 32 GB RAM, AMD Radeon RX 9070 XT 16 GB. There is no NVIDIA GPU, so the AMD path must work] and [MacBook Air M5, 16 GB RAM].

## Tech stack
- Shell: Tauri 2 with a Rust backend and a React + TypeScript (Vite) frontend, using Tailwind plus a headless component library. Lock down Tauri capabilities and the CSP. The webview never loads remote content.
- Inference: bundle llama.cpp's llama-server as a Tauri sidecar.
  - Pin it to a specific release and start it with --jinja so tool calling works.
  - The backend owns its lifecycle: start, health check, restart on crash, and no orphaned processes after quit.
  - Load one local model at a time. Switching models shows load progress.
- Remote providers: connect to any OpenAI-compatible endpoint (Ollama, LM Studio, vLLM, a remote server) with a base URL and an optional API key.
- Storage:
  - SQLite for chats, settings and the audit log.
  - Secrets (API keys, HF token, MCP env vars) go in the OS keychain, never in plain files.
  - The models folder is user-configurable.
- Tools: everything goes through MCP, using the official Rust MCP SDK (rmcp). The built-in tools are MCP servers too, so they share one permission and UI path with servers the user adds.

## Agent loop (the hardest part; get this right)
- Capability detection: read GGUF metadata to set per-model flags for tool calling, vision (an mmproj file is present) and context length. Show these as badges. If a model can't call tools, disable tools for it and explain why in plain language.
- Tool calls:
  - Use OpenAI-style tool calling from llama-server.
  - Add a fallback parser that recovers tool calls the model emits as JSON in plain text.
  - Validate arguments against the tool's JSON schema. On a bad call, return the error to the model and allow a limited number of retries.
- Limits: cap the number of steps per turn (default 25, configurable). A cancellation token reaches the inference stream and every running tool.
- Context management:
  - Count tokens and show context usage in the UI.
  - Truncate long tool output, with a note saying how much was cut.
  - Summarize older turns as the context window fills.
  - Only send schemas for enabled tools, because small models get worse when given many tools.
- UI: each tool call appears in the chat as a collapsible card with the tool name, arguments, result and duration.

## Features
1. Chat:
   - Streaming responses, with stop, regenerate, and edit-and-resend.
   - Conversation history with search, rename and delete.
   - Per-chat model and system prompt, plus a system prompt editor.
   - Attachments: images for vision models only. PDFs and text files have their text extracted locally.
   - Tokens/sec and context usage display.
2. Model manager:
   - Search Hugging Face from inside the app, showing GGUF files only. Show size, quantization, license and whether the repo is gated (with an optional HF token for gated models).
   - Fit check against VRAM, unified memory or RAM. Estimate model weights + KV cache for the chosen context length + runtime overhead. Show green, yellow or red with a one-line plain explanation.
   - Download with progress, pause/resume and SHA-256 verification. Handle split GGUFs, and fetch the mmproj file alongside vision models.
   - Delete models, show disk usage, and import a GGUF that's already on disk.
3. Agent tools, each toggleable per chat:
   - Files:
     - Read, list, search and write only inside folders the user approved.
     - Canonicalize paths and block ../ and symlink escapes.
     - Writes show a diff before they are applied.
   - Terminal (off by default):
     - Run commands in an approved working directory: PowerShell on Windows, zsh on macOS.
     - Stream the output, with a timeout and a kill option.
   - Git:
     - Clone, pull, status, diff and commit. Push asks for approval every time.
     - Use the system git if it's installed, and libgit2 if it isn't.
   - Browser:
     - Drive the user's installed Chrome or Edge over CDP, in a separate app-owned profile that never touches their logged-in sessions.
     - Navigate, read the page as text or an accessibility tree, click, type, fill forms and take screenshots.
     - Form submissions always need approval.
   - Computer use (vision models only, off by default):
     - Take screenshots and control the mouse and keyboard.
     - Guide the user through the macOS Screen Recording and Accessibility permissions step by step.
     - Handle HiDPI scaling so model coordinates map to real screen coordinates, and downscale screenshots to the model's input size.
     - Show a visible on-screen indicator while it is active.
4. Skills:
   - Follow the open Agent Skills format (agentskills.io) so existing skills work unchanged. A skill is a folder containing SKILL.md (YAML frontmatter with name and description, then Markdown instructions), plus optional scripts/, references/ and assets/ folders.
   - Load skills progressively. Only names and descriptions go into the system prompt. The model loads the full SKILL.md with a load_skill tool, and referenced files with a read_skill_file tool.
   - Include a UI to import (folder or zip), create, edit with frontmatter validation, enable and disable skills.
   - Skill scripts run only through the terminal tool, with the same approvals.
5. MCP:
   - Add servers through a form, or by pasting JSON in the same "mcpServers" format Claude Desktop uses.
   - Support the stdio and Streamable HTTP transports.
   - Show each server's tools with per-tool toggles, plus server status and logs.
   - If a server needs Node (npx) or Python (uvx) and it's missing, say so in plain language.
   - When a user adds a server, warn them that it runs with their own permissions.

## Safety
- Approval cards: every tool call shows the exact action before it runs: the full command, the full file path, a diff for writes, or the URL for navigation.
  - Options: Allow once, Allow for this chat, or Deny (with an optional note sent back to the model).
  - "Allow for this chat" never covers destructive actions. These always ask: deleting files, rm -rf or del /s style commands, sudo or admin elevation, git push, and form submissions.
- Defaults: computer use and the terminal are off by default.
- Stopping the agent: provide a global stop hotkey (configurable) and a Stop button.
  - Stopping cancels inference, kills child processes, releases the mouse and keyboard, and halts browser actions within 1 second.
  - The default hotkey must not collide with OS shortcuts. Avoid Ctrl+Shift+Esc (opens Task Manager on Windows) and Cmd+Option+Esc (Force Quit on macOS).
- Untrusted content: content from web pages, files, tool output and MCP servers is data, never instructions. Nothing the model outputs can change approval settings.
- Audit log: keep a local log of every tool call (arguments, decision, result summary) that can be viewed and exported in the app.

## UX
- First run:
  - Flow: detect hardware → recommend one model that fits → one click to download → chatting. That's at most 5 clicks, and the download is the only wait.
  - Recommended models come from a curated JSON file bundled with the app, which can be refreshed when online. No model names are hardcoded.
  - Also offer "Use a model I already have" and "Connect Ollama / LM Studio".
- Look and feel: a clean, minimal UI with light, dark and system themes, and keyboard shortcuts for common actions.
- Errors:
  - Show a plain-language message plus what to do next, with a "Copy details" button for the technical info. Never show a raw stack trace.
  - Cover at least: model too big for memory, port already in use, download interrupted, GPU driver missing or outdated, and MCP server failed to start.

## Packaging and release
- GitHub Actions builds the Windows installer and the macOS .dmg on every tag, using tauri-action.
- Prepare code signing: macOS Developer ID signing plus notarization, and Windows code signing. Leave the secrets as placeholders for me to fill in later, and document how to open unsigned builds in the meantime.
- Updates: opt-in check only, using Tauri's updater with signature verification.
- Include third-party license notices for llama.cpp and every other bundled binary.

## Out of scope for v1
Linux, mobile, Intel Macs, accounts, cloud sync, voice, image generation, and running multiple local models at once.

## Milestones
- M0: ARCHITECTURE.md approved. Repo scaffold. CI produces an installable empty app on both OSes.
- M1: Bundled llama-server, and chat with a GGUF loaded from disk. OpenAI-compatible endpoints.
- M2: Model manager and the first-run flow.
- M3: Agent loop, MCP host, approval system, stop hotkey, audit log and the Files tool.
- M4: Terminal and Git tools.
- M5: Skills.
- M6: Browser tool.
- M7: Computer use.
- M8: Polish: error coverage, themes, accessibility, signing and the release build.

A milestone is done when:
- CI builds it on both OSes.
- Tests pass. Unit tests are mandatory for path sandboxing, permission rules and tool-call parsing.
- The manual test steps you give me work on my machines.
- The README and CHANGELOG are updated.

## Deliverables
- README (build, run, architecture overview)
- ARCHITECTURE.md
- SECURITY.md (threat model and permission design)
- Short guides for writing a skill and adding an MCP server

# Owner's answers to the clarifying questions

1. Windows installer: "bundle all three + ROCm" (Vulkan, CUDA, CPU and a ROCm build, all inside the installer).
2. "Allow for this chat" is scoped to tool + scope (not the whole tool).
3. Stop after each milestone and wait for the owner to test.
4. License: MIT.
5. The MacBook Air M5 has 16 GB RAM.
