# Changelog

All notable changes, by milestone. Dates are UTC.

## 0.0.3 (2026-11-11)

Three things reported from a real Windows PC on 0.0.2: a model that seemed not to load, a task that stopped at the step limit, and "the app looks too plain and is not smooth".

### Changed
- **A new look.** The colours have layers now (page, sidebar, floating surfaces) with one accent gradient, soft shadows and rounder corners, in dark and in light. What changed on screen:
  - *With no chat open* there is a welcome screen instead of one line of grey text: the loaded model (or the recent ones, one click to load), and four things to try that write a request into the message box.
  - *Sidebar:* the app's mark, a prominent New chat button, chats under Today, Yesterday, This week and Earlier, and at the bottom what is loaded and on which backend (it opens the model manager).
  - *Header:* nothing wraps onto a second line any more at narrow widths (labels give way to icons), and context use is a small meter (`40.9k / 65.5k tokens (62%)`) that turns amber, then red, as it fills.
  - *Messages:* your messages are tinted bubbles; code blocks have a header with the language and a Copy button; tables, tool cards, approval cards, banners and dialogs share the same surfaces.
  - *Message box:* floats above the page and shows focus as a glow.
- **Motion.** Buttons, menus and fields ease between states; dialogs and menus open and close with a short scale and fade; messages, tool cards and notices rise in; a moving bar shows while a model loads without a known percentage; three dots show while the model works. All of it is switched off when the system asks for reduced motion.
- **Smoother while a reply streams.** Messages above the one being written are no longer rendered again for every chunk of text; a long reply with code is rendered as fast as the machine can without holding up typing or scrolling; off-screen messages are skipped by the layout engine; and the chat only follows new text while you are at the bottom. Scroll up to read and it stays put, with a button to jump back to the latest message.
- With no chat open and a model loaded, typing a message starts a chat with that model, and New chat uses the loaded model.
- **No white flash at start on a dark system.** The main window was created white and stayed so until the page had painted. It is now created hidden, given the page's background for the system theme, and then shown. Checked on Linux that the window still appears; the flash itself could only be seen on Windows or macOS, so this part is not verified.
- **The step limit is a setting, and reaching it is no longer a dead end.** Reported on 0.0.2: a task (installing Rust) stopped at "The step limit for one turn was reached", shown as a red error. A turn could make 25 model calls and that number could not be changed. The default is now 50, *Settings → General → Steps per turn* offers 25, 50, 100 and 200, and when a turn stops at the limit the chat shows a notice with a **Continue** button instead of an error.

### Fixed
- **Browser: the first navigation in a new tab could skip the request checks.** A tab counted as usable as soon as the browser reported it attached, while its requests were put under the app's checks by a separate task a moment later. A navigation sent in that moment was not paused at all: its redirects were not checked against the private-network and loopback rules, and the tab got no granted site, so a pop-up it opened later waited for an approval nobody was asked for. In practice the moment is microseconds long; it showed as a browser test that hung on two CI runs in a row (Windows, then Linux). Now no browser call touches a tab until `Fetch.enable` has been answered for it. A new test holds the moment open for 1.5 seconds: before the fix the browser followed a redirect to a loopback address, with it the redirect is blocked.
- **A model picked with no chat open seemed not to load, and picking it again said "Loading was cancelled."** Reported on 0.0.2. With no chat open, the picked model had no chat to belong to: it loaded in the background, the header kept saying "Choose a model" and showed no progress. Picking the file again then restarted the load, and the first attempt reported "Loading was cancelled" although the second went on to finish. Now picking a model with no chat open opens a new chat for it, so its name and loading progress show at once; picking the same model again while it loads waits for that load instead of restarting it; and a load that is replaced because another model was picked ends without an error.
- A load that was replaced could take the newer load's server for its own and wait on it for ever, which also left that server running after an unload. Each load now touches only the server it started, and two loads never replace the server at the same time. Found while testing the fix above (it showed as a load request that never returned).
- If the task that runs llama-server ended by itself, the app said "Loading was cancelled". It now says that the loader stopped unexpectedly and keeps the details.

## 0.0.2 (2026-11-11)

Fixes for the first two problems reported from a real Windows PC. The version number is new so the build can be told from 0.0.1 in *Settings → General*.

### Fixed
- **The app did not open again after its window was closed.** Closing the main window left the app running with no window: its two small helper windows (the computer-use overlay and indicator) are created hidden at startup and kept the process alive. Opening the app again only woke that invisible copy, which had no window to show. The app now quits when its main window closes, and a copy that is asked to show a main window it no longer has exits, so the next start works. Until this build is installed: end "harnessUI" in Task Manager, then open it again.
- **No GPU on a Windows PC whose graphics driver came without the Vulkan runtime.** Reported on a PC with a Radeon RX 9070 XT: the driver was installed, but `vulkan-1.dll` (the Vulkan runtime every Vulkan program loads first) was not in `System32`. Why is not known: AMD's full Adrenalin package was installed and up to date on that PC. llama.cpp's Vulkan library then cannot be loaded, and it says nothing: the device list is just empty. The Windows installer now carries its own copy of the Vulkan runtime (the Khronos loader, 1.4.363.0 from LunarG, pinned by SHA-256) in `llama\main\vulkan`, and llama-server gets that folder as the last entry of its `PATH`. Windows looks there last, so a runtime installed with the driver is always used when there is one.
- **A graphics card that Vulkan did not list was treated as the wrong card.** The app knew a PC's graphics card only from the Vulkan backend's device list, and counted the CPU as a card. When Vulkan listed nothing, the ROCm download said "supports only AMD Radeon RX 9000-series cards" and the CUDA download said "needs an NVIDIA graphics card", whatever card the PC had. The app now also asks Windows which display adapters are installed, and the downloads are decided from that.
- The Vulkan backend's "no Vulkan device found" is replaced by a message that names the cards Windows sees and says what to do (or says that only a virtual adapter is visible, as in a virtual machine).

### Added
- *Settings → Diagnostics → Run backend check* now lists the graphics adapters Windows reports with their driver versions, includes what llama-server printed when a backend found no device (the check now asks for the verbose listing, which names the backend libraries that were loaded), and repeats that check with the system's full environment to show whether a driver needs a variable the app does not pass on.
- CI loads llama.cpp's Vulkan library on the Windows runner with and without the app's copy of the Vulkan runtime. The runner has a Vulkan runtime of its own, so this shows only that the app's copy does no harm; the missing-runtime case was confirmed by hand on the reported PC (with the same runtime file last on `PATH`, llama-server listed the RX 9070 XT), not in CI.
- `e2e/m0_close.py`: closes the main window the way its close button does and checks that the app quits and starts again. It fails on 0.0.1.

## 0.0.1 (2026-11-11)

The first release: everything below, M0 to M9, under the app's corrected name, **harnessUI** (test builds before this were called "harnissUI"). An early test build. The installers are not code-signed (see the README for opening them), there are no in-app updates yet, and nothing has been tested on a real Windows PC or Mac; computer use is the least proven part.

### Changed in 0.0.1
- The app is named harnessUI. Its identifier is `io.github.sugarhappy8.harnessui`, so it keeps its data, settings and keychain entries in new places: chats, settings and saved keys from an earlier "harnissUI" test build are not carried over (that build can be uninstalled). The GitHub repository is still called `harnissUI`; the app's download links point there.
- A release is published automatically once the release workflow has checked that both installers are attached.

## M9 — Works out of the box on more machines (2026-11-11)

Added at the owner's request after the first test builds: models could not browse or use the computer without setup, and only Apple Silicon Macs could run the app.

### Added
- **Subagents.** A built-in group "Subagents" (one tool, `run_subagent`), off in new chats and switched on per chat in the Tools panel. The model can hand a self-contained task to a helper that works in a fresh context with the same model and the chat's tools, and returns only its answer. The helper's actions go through the same policy, grants, approval cards (marked "Subagent:") and audit log; it inherits the turn's untrusted-content and recovered-call state; it cannot start another helper; Stop ends it at once; nothing of it is stored in the chat except the result, and its token counts do not reach the chat's context numbers.
- **A browser of the agent's own on Macs.** With no Chrome, Edge, Brave, Vivaldi or Chromium installed, *Settings → Browser* downloads Google's Chrome for Testing (157.0.8103.50; pinned by size and SHA-256 in `browser.lock.json`, from storage.googleapis.com only) into the app's data folder. Links in the archive are checked to stay inside the folder. An installed Chrome or Edge is preferred.
- **One Mac download for Intel and Apple Silicon.** CI and releases build a universal app; llama.cpp is built from the pinned commit for both processors and joined; a CI job runs the Intel half on an Intel Mac runner.
- Brave and Vivaldi are detected on Windows, macOS and Linux.
- The Tools panel shows why the Browser group cannot work when no browser is found, with *Set up the browser*.

### Changed
- **Browser tools are on in new chats** when a browser is available and the model's context is at least 8,192 tokens (D10 was "files only"). A chat's own choice and the saved last choice are untouched.
- **Computer use needs no per-model setup.** The way a model writes screen positions is picked from its name (pixels for the Qwen2.5-VL family such as UI-TARS-1.5, Fara and Holo1.5; a 0–1000 grid for Qwen3-VL, Qwen3.5 and newer Holo; y-first boxes for Gemma; a 0–1000 grid as the guess for unknown models). *Settings → Computer use* shows "Automatic" and lets the user choose another. The confirmation tick box is gone. Computer use is still off in every chat until switched on, and every safeguard is unchanged.
- **Computer use and Subagents are switched on for one chat at a time.** The "last choice becomes the default for new chats" rule no longer carries them over. Tool defaults saved before this version are reset once, so the new Browser default takes effect.
- **Intel Macs run models on the CPU by default** (llama.cpp's Metal backend is unreliable on Intel graphics); Metal can be chosen by hand. First run says so.

### Reviewed
An independent read of the new code found no way around the approval rules and eight defects, fixed here: Qwen2-VL was given Qwen2.5-VL's pixel convention (it answers on a 0–1000 grid, so clicks would have landed in the wrong place silently); a convention saved but never switched on in an older version overrode the automatic one; the model could write a line starting with "[App note]" into a helper's task; the Tools panel and a turn could disagree about a remote model's context; Cancel was ignored while a download waited to retry; and a link in a downloaded archive could have been created through an earlier link.

### Known issues
- None of this has run on a real Windows PC or Mac yet; see docs/MILESTONES.md (M9).

## Debugging pass (2026-11-11)

The app was driven end to end against a fake model server with scripted replies (text, thinking, tool calls, HTTP errors, dropped streams), which makes the agent loop testable without a model. The script is now `e2e/scripted_agent.py` (16 checks).

### Fixed
- A failed reply (server error, rejected key, dropped connection) was stored as two messages: an empty "Stopped" one, then the error. The text streamed before a dropped connection was lost. A failed reply is now one message that keeps the text written so far.
- A tool call that did not run (invalid arguments, not allowed, denied) showed "Failed" with an empty result. The card now shows the reason.
- Deleting a chat while its reply was being written left the model, and any tool it called, running for a chat that no longer existed, and blocked new messages until it finished. Deleting a chat now stops its reply first.
- The chat could stay stuck in "generating" (Stop button, disabled composer) when the chat could not be reloaded at the end of a turn.
- Stopping a reply and sending a new message at once could leave the new reply unreachable by Stop (the old reply's end cleared the new one's handle).
- Advice under errors fits the error and the endpoint: a rejected API key points to *Settings → Endpoints* instead of "check that the model is still running"; a remote endpoint's context size is a setting there, not something to "load the model" with.
- A reply whose stream ended without the server saying it was finished is kept, with a note that it may be cut short.
- Notes from a turn (a withheld tool, a summarised history, a cut-short reply) vanished the moment the turn ended. They stay under the reply until the next message.

## Final audit (2026-11-10)

A read-only audit of the whole app against ARCHITECTURE.md, the security rules and the docs (five independent reviewers, each finding checked by a second one that tried to refute it) confirmed 41 distinct problems. 33 are fixed below; the other 8 are larger features, listed under known issues in docs/MILESTONES.md.

### Security
- Credentials written into a git push URL no longer reach the approval card, the audit log, the chat database or the model's prompt (both git backends); every URL in git's output and errors is masked. A URL's host is read as git and curl read it (the authority ends at the first `/`, `?` or `#`), so a card never shows a host other than the one git connects to.
- The file and git tools cannot write into skill folders (the app's skills folder and, when used, `~/.agents/skills`): a new skill there would otherwise be loaded without the user adding it. Skills are added in *Settings → Skills* only; the terminal still runs in skill folders.
- An external tool whose input schema puts model control tokens where they cannot be cleaned (property names, enum, const, default or example values) is not offered to the model; the chat says which tool and why.
- *Forget everything I allowed in this chat* and revoked tool grants now apply to a turn that is already running (grants stay in memory only, D12).
- A pasted MCP config that already contains `${secret:…}` placeholders is refused (it could have pulled another server's saved secret); two servers whose names would share saved secrets are refused.
- Every Unix child (llama-server, MCP servers, tool commands, workers, the browser) starts with only stdio and the descriptors meant for it: on macOS, std cannot create pipes and sockets close-on-exec atomically, so one created at the moment of a fork could leak into a child.
- macOS: Apple's `/usr/bin/git` stub is never run when the developer tools are missing (it opened an installer dialog at every start); the git tools use the built-in library or a Homebrew git. The app's logs folder is out of the tools' reach on macOS too.
- Windows: model replies containing HTML entities such as `&amp;` no longer break rendering. The markdown parser's browser build decoded them through `innerHTML`, which the app's Trusted Types policy refuses in WebView2; the table-based build is used instead.

### Fixed
- Auto never picks CUDA or ROCm (D3: Metal, Vulkan or CPU); ROCm launches show only the chosen card to HIP (`HIP_VISIBLE_DEVICES`).
- Remote endpoints that do not report a context size get *Settings → Endpoints → Context size when a server does not report one* (4,096 by default) instead of a fixed 8,192.
- Imported skills start turned off, with a note that a skill's text becomes instructions to the model.
- The automatic update check's result is shown (a notice in the main window and in *Settings → General → Updates*); before, nothing listened for it.
- macOS *Open System Settings* buttons in *Settings → Computer use* work (the opener scope allows the two Privacy panes) and show an error if opening fails.
- First run preselects a model only when a catalog entry has been tested on real hardware; no entry is marked tested yet.
- The release workflow fetches the pinned llama.cpp runtime before building, signs and uploads the catalog when the updater key exists, and fails if the update manifest misses a platform or the catalog is missing. Catalog Refresh reports an up-to-date catalog as such, not as an error.
- The Windows installer stops a stray `llama-server.exe` running from the install folder before copying files.
- The offline gate (`e2e/m8_offline.py`) visits all twelve settings pages.
- Docs: the catalog test command, the backend-check steps, the crate table, the keychain names in the MCP test step, the Windows signing note and the Linux runtime example were wrong and are corrected.

### Added
- Chat *Tools* panel: *Folders for this chat* (approve a folder for this chat only, remove it, choose the chat's working folder) and *Skills loaded in this chat* with *Unload*.
- *Settings → General*: a default system prompt for new chats, and *Model catalog* (version and *Refresh catalog*, disabled with the reason until the updater key exists).
- *Settings → Diagnostics → Open logs folder*.
- A warning bar in the main window while the stop hotkey is not registered.
- Syntax highlighting in fenced code blocks that name a language (`rehype-highlight`: it produces React elements, no HTML strings, so the strict CSP is unaffected).
- docs/skills-guide.md: how to write a skill for harnessUI.

## M8 — Polish (2026-11-10; macOS signing and in-app updates wait for your keys; Windows signing is not set up yet)

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

## M7 — Computer use (2026-11-10, untested on real hardware)

### Added
- `crates/computer`: screenshots of one monitor (xcap on Windows; ScreenCaptureKit on macOS 14+, the older call on 13) with this app's windows left out and a cursor marker drawn in, resized exactly as llama.cpp resizes images for the model (a port of its size calculation, checked bit for bit against the C++); clicks, moves, drags, scrolls, typing and key combinations (SendInput for the mouse on Windows, enigo on the main thread for the keyboard) through one injector that refuses input to this app and its WebView processes, re-checks the target application before every event, refuses elevated windows on Windows, refuses the indicator's Stop button, and releases every held key and button on Stop, panic or exit. Grants per kind of input and per application (D7); terminals, system settings, password managers and OS-level shortcuts always ask. Coordinate conventions per model: pixels of the resized image, a 0–1000 grid, 0–1000 boxes (and Gemma's y-x boxes). 69 tests with fake capture and input.
- App: the computer server on Windows and macOS; the indicator (red border, "AI is controlling this computer" bar with a Stop button whose window may call nothing but stop) shown while it acts and hidden when the turn ends; input refused when the indicator or the stop hotkey is missing, or (macOS) the indicator is not on the active Space; the stop hotkey and the Stop button release held input. Settings → Computer use: macOS Screen Recording and Accessibility checks with links to System Settings, and each model's convention. When a click is refused because harnessUI's own window covers the point, the tool card offers to minimise the window. The tools are offered only for models that can see images and whose convention the user confirmed.

### Known issues
- The one-click calibration test of 9.3 is replaced by a convention the user confirms per model (the app's windows are excluded from screenshots, so the model cannot see targets the app draws). See docs/MILESTONES.md.
- Not run on real Windows or macOS yet; CI compiles and unit-tests both.

## M6 — Browser tool (2026-11-10)

### Added
- `crates/browser`: the built-in browser server with its own CDP client over a pipe (fds 3/4 on macOS; `--remote-debugging-io-pipes` handles on Windows, through a spawn-module change that keeps other children from inheriting them) or, as a setting, a loopback port. Request enforcement in the browser for every tab, pop-up, frame and redirect: http(s) only; loopback, link-local and (unless allowed) private addresses blocked on the resolved IP; leaving a site held for approval; form submissions and non-GET navigations held until approved and failed on Deny, Stop or timeout; file choosers and downloads refused. Accessibility-tree snapshots with element refs sized to the result cap, find, readable text, click (refused when another element covers the target), type, fill a form, keys, screenshots, back. Every read, click and type re-checks the frame's actual origin. 75 unit tests and 18 end-to-end tests against Chromium.
- App: the browser registered as a built-in (off in new chats, D10); held navigations become always-ask cards in the chat with their own audit rows; Settings → Browser (browser path chosen in a native dialog, port mode with its warning, private networks, strict sites); the browser closes on quit and before an update installs. CI runs the browser's end-to-end tests on Windows to check the pipe mode there.

### Known issues
- One browser profile is shared by all chats; DNS rebinding and service-worker cache answers are not re-checked; consequential-verb detection is English only.
- On networks where this computer cannot resolve host names itself (proxy-only), pages are refused (the check fails closed).

## M5 — Skills (2026-11-10)

### Added
- `crates/tools` skills: lenient SKILL.md loading with warnings (skills written for other clients work) and the strict validator for the editor; the `<available_skills>` index with escaped, capped names and descriptions; zip and folder import refusing absolute paths, `..`, symlinks, more than 512 files or 16 MiB; `load_skill` (SKILL.md, the file list and quoted commands for scripts, the 30% context budget) and `read_skill_file` sandboxed to the skill's folder; `allowed-tools` shown, never granting anything.
- Settings → Skills: import a folder or zip (native picker), create and edit with the strict validator, enable or disable, delete, open the folder, and optionally use `~/.agents/skills`. Skill reads never ask (D11) and are audited as "allowed without prompt".

## M4 — Terminal and git (2026-11-10)

### Added
- Terminal (`run_command`): one command per call in an approved folder or an enabled skill's folder; Windows PowerShell 5.1 through `-EncodedCommand` (no quoting can change what runs), `zsh -lc` on macOS with the login environment; secret-looking variables removed, with a user-editable pass-through list; 1 MiB output cap streamed into the tool card; timeout and Stop kill the process tree.
- The fail-closed command classifier in `crates/policy` (tree-sitter bash and PowerShell grammars): only plain sequences of literal-word commands can be covered by a grant; deletes, elevation, disk and system commands, `git push/clean/reset --hard`, `git -c` and aliases, inline interpreter code, wrappers and anything the parser cannot fully break down always ask.
- Git tools on system git from fixed argument templates: hooks, fsmonitor, external diff, textconv and the pager off; `https` and `ssh` only; prompts off; repository credential helpers replaced by the user's own; repositories whose own config defines filter, merge or diff drivers refused; push always asks and re-checks every push URL at run time; URLs git and the app could read differently are refused.

- The libgit2 fallback for the git tools when Git is not installed: the same seven tools and approval cards, no hooks, no LFS, https only; clone, pull and push run in a worker process (the app binary with a hidden flag) that Stop kills; private repositories authenticate with a token per host from the keychain, sent only to the approved remote's host after it asks.
- Settings → Git: which Git the tools use, tokens per host, and the name and email for commits. The Tools panel marks the Git group when the built-in library is in use.
- THIRD-PARTY-NOTICES.md lists the C libraries built into the app (libgit2, zlib, and OpenSSL on macOS); CI checks the macOS app binary links nothing outside the system.

## M3 — Agent loop, MCP host, approvals (2026-11-10)

### Added
- `crates/policy`: the path sandbox (lexical checks first, so a network path causes no I/O; then canonical paths re-checked, case-insensitive where the volume is), the evaluation order of 13.3, grant scopes, always-ask write rules, and the sanitiser that makes control, bidi and zero-width characters visible on cards.
- The files tool (`list_dir`, `read_file` with line paging, `search_files`, `write_file`, `edit_file`, `delete_file`) with diff previews and a hash re-check before every write; a symlink planted after the preview is refused.
- PDF text extraction in a worker process with a timeout and a memory cap.
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

## M2 — Model manager (2026-11-10; catalog entries wait for your test runs)

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
- App: model loading with progress, streaming chat, stop, regenerate, branch switching, edit-and-resend, history search, rename, delete, per-chat model and system prompt, tokens/s, OpenAI-compatible endpoints with keychain-stored keys, and *Settings → Diagnostics → Run backend check* (the in-app probe build: devices per backend and, with a model loaded, a timed generation on each, saved to the logs folder).
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
