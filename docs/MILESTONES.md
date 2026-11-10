# Milestone summaries

One section per milestone of ARCHITECTURE.md section 19: what works, how to test it by
hand on your Windows PC and your Mac, known issues, open TODOs and the dependencies the
milestone added. The CHANGELOG has the full list of changes.

Builds come from the latest green **CI** run on the branch (*Actions → CI → Artifacts*:
`windows-installer`, `macos-dmg`). See the README for opening unsigned builds.

Linux is used for development and automated end-to-end runs only (`e2e/`). Everything
that depends on a real GPU, the keychain, window behaviour or an installer has to be
checked on your machines with the steps below.

---

## M0 — Scaffold

**Works:** Cargo workspace; Tauri 2 shell with navigation locked to the app's origin,
two capability files (main, overlay), every command explicitly granted; strict CSP; CI
on Linux, Windows (NSIS installer) and macOS (dmg); release workflow with signing
placeholders; cargo-deny.

**Manual test (both machines):**
1. Install the build. Windows: SmartScreen → *More info → Run anyway*. Mac: right-click → *Open*.
2. The window opens with the sidebar and an empty chat. *Settings* (gear, or Ctrl+, / Cmd+,) opens; switch Light / Dark / System and check the whole UI follows.
3. Close and reopen: a second launch focuses the first window instead of opening another.

**Known issues:** builds are unsigned (SmartScreen, Gatekeeper).

**Dependencies:** the workspace set of ARCHITECTURE.md section 18 (Tauri 2.12 and its
plugins: dialog, opener, clipboard-manager, global-shortcut, single-instance).

---

## M1 — Runtime and chat

**Works:** llama-server as a managed child (Job Object on Windows, process group plus a
reaper on macOS) with load progress, restart and backend step-down; streaming chat with
stop, regenerate (version arrows), edit-and-resend, search, rename, delete, per-chat
model and system prompt, tokens/s; OpenAI-compatible endpoints with keys in the
keychain; *Settings → Diagnostics → Run backend check*.

**Manual test — Windows PC:**
1. *Settings → Diagnostics → Run backend check*. Expect Vulkan listing your RX 9070 XT and CPU, each with a timed generation; note both tokens/s numbers (this decides D3's Auto order).
2. Model picker → *Open a GGUF file…* → pick any GGUF. The indicator goes from loading (with progress) to ready; the header shows backend and context.
3. Send a message; tokens stream, tokens/s appears. Press Stop mid-reply: "Stopped". Regenerate: "2 / 2" arrows switch versions. Edit your message and resend: the old branch stays reachable.
4. Task Manager: one `llama-server.exe` under the app. End the app's process tree from Task Manager → `llama-server.exe` disappears too.
5. *Settings → Endpoints*: add LM Studio or Ollama if you have one; pick its model in a new chat.

**Manual test — Mac:**
1. Backend check: Metal (Apple M5) and CPU.
2. Steps 2, 3 and 5 as above.
3. Activity Monitor: one `llama-server`. Force-quit the app → `llama-server` exits within a few seconds (the reaper).
4. Keychain: adding an endpoint key may raise a keychain prompt on this unsigned build (see Known issues).

**Known issues:** WebView2 crash reporting is disabled by command-line switches only (wry
does not expose the documented setting; unverified on real Windows). (Since M8: the
header warns "Loaded but slow" or "N of M layers on the GPU" after a load.) Unsigned macOS builds can trigger repeated keychain prompts.

---

## M2 — Model manager and first run (in progress)

**Works:** Hugging Face search (GGUF chat models) with every quantization and a
green/yellow/red fit verdict for your hardware; downloads with pause/resume, resume after
restart, SHA-256 verification; installed models with badges from the GGUF header;
import; delete; models folder setting; first-run screen suggesting a catalog model;
image attachments for vision models (with their projector); PDF and text attachments;
ROCm and CUDA backend packs downloaded on demand and verified by SHA-256.

**Manual test — Windows PC:**
1. Fresh install: the first-run screen suggests a model for your RAM/VRAM. *Skip for now* also works.
2. *Models* (Ctrl+M) → *Browse* → search "qwen3". Open a repo: each file has a fit badge; your 16 GB card should show 8B Q4 as green.
3. Download a small model; pause, resume, quit the app mid-download, restart: it resumes. When done it appears under *Installed* and loads.
4. *Settings → Local models → Backend downloads*: ROCm is offered only on a new enough Windows build and RX 9000 series; CUDA only with an NVIDIA GPU. If offered, download ROCm, accept the license, and load a model with backend ROCm.
5. Attach an image to a chat with a vision model (e.g. SmolVLM, Qwen2.5-VL): the model describes it. Attach a PDF: the model can quote it.

**Manual test — Mac:**
1. First run: the suggestion should fit 16 GB (about an 8B-class model at ~16k context).
2. Steps 2, 3 and 5 as above (no backend packs on macOS).

**Known issues:** the catalog's entries are not yet marked tested on your machines, so
first run never preselects one (run `scripts/catalog_test.py`, see below). Catalog
Refresh is disabled until the updater key exists.

**TODO for you:** `python3 scripts/catalog_test.py <entry>` on each machine records a
tested entry.

---

## M3 — Agent loop, MCP host, approvals, audit, files tool

**Works:**
- Every reply is an agent turn: the model can call the tools switched on for the chat. Tool calls show as cards (arguments, live output, result, duration); the header shows context used.
- Approval cards in the chat: *Allow once*, *Allow for this chat*, *Deny* (with a note the model sees). Input is ignored for 700 ms after a card appears, Enter never approves. Stop (button, Esc or the global hotkey) closes an open card and cancels the call.
- One audit row per tool call (*Settings → Audit log*, filter and export).
- Per-chat *Tools* panel with per-tool token estimates; files and skills are on in a new chat.
- *Settings → MCP servers*: paste a Claude Desktop / Cursor / VS Code config; secrets move to the keychain; a new server's tools start off; per-tool "allow any arguments" and "always ask"; status, restart, log tail.
- Built-in servers: files, terminal, git, skills (terminal and git off by default).

Automated: `e2e/m3_agent.py` passes against the real llama-server with Qwen3 0.6B
(Allow once, Allow for this chat then a call under the grant, Deny, Stop with a card
open, audit rows).

**Manual test — both machines** (use a tool-capable model, e.g. Qwen3 8B):
1. *Settings → Permissions → Approve a folder* → pick a test folder with a text file in it. Approving your home folder shows a warning; the app's own folders are refused.
2. New chat: "Read notes.txt in my folder and summarise it." A `read_file` card appears with the full path. Click *Allow once*: the result card shows the file, the model answers.
3. Ask again with *Allow for this chat*; a third read in the same chat runs without a card. Check *Settings → Audit log*: allow_once, allow_for_chat, allowed_by_grant.
4. "Create hello.txt containing hi." The card shows a diff. *Deny* with a note: nothing is written, the model mentions your note.
5. Ask for a write, and while the card is up press the global stop hotkey (Windows Ctrl+Alt+Backspace, Mac Ctrl+Option+Delete) with another app in front: the card closes, the call shows Cancelled.
6. *Tools* panel: switch on Terminal; ask "what is in this folder? use run_command". Windows runs PowerShell 5.1, Mac runs zsh. `dir`/`ls` can be granted; `rm`/`Remove-Item` always asks.
7. MCP: *Settings → MCP servers → Add servers*, paste e.g. `{"mcpServers":{"everything":{"command":"npx","args":["-y","@modelcontextprotocol/server-everything"]}}}` (needs Node.js). Confirm the warning. The server goes Running; open it, switch on `echo`; in a chat ask the model to echo something: card shows server, tool and arguments.
8. Paste a config with `"env": {"API_KEY": "abc"}`: after adding, the stored config shows a `${secret:…}` placeholder (Windows Credential Manager / macOS Keychain Access holds the value under the app's name).

Also: a per-chat *Thinking on/off* switch in the header for local thinking models.

**Known issues:** MCP servers that need OAuth
sign-in are not supported yet.

**TODOs:** count image tokens against the per-result cap; verify Ollama's thinking-off
field on a real Ollama; replace the bounded echo look-back in the text-call parser with a
linear scan; check tool-call id formats against Mistral templates.

**Dependencies:** none new in the workspace (rmcp, jsonschema, process-wrap, async-trait,
sha2 were already listed in section 18 and are now used by `mcp-host` and `agent`).

---

## M4 — Terminal and git (in progress)

**Works:** terminal (`run_command`): PowerShell 5.1 via `-EncodedCommand` on Windows, `zsh
-lc` with the login environment captured at start on macOS; secret-looking variables
removed; 1 MiB output cap; timeout and Stop kill the process tree; the fail-closed
command classifier decides what a grant can cover. Git tools on system git with hooks,
fsmonitor, external diff, pager and repository credential helpers off; repositories
whose own config defines filter/merge/diff drivers are refused; push always asks and
re-checks its URL.

**Manual test:** M3 step 6; for git: switch on Git, ask for `git status` in a repository
folder, then a commit (card), then a push (always asks, card shows the push URL).

**Known issues / TODOs:** the libgit2 fallback for machines without git is not built
yet; on such machines the git tools say "Git is not installed" with install steps.

---

## M5 — Skills

**Works:** *Settings → Skills*: import a folder or a zip (picked in a native dialog;
absolute paths, `..`, symlinks, more than 512 files or 16 MiB are refused), create and
edit SKILL.md with the strict validator, enable/disable, delete, open the folder, and
optionally read `~/.agents/skills`. Enabled skills appear in the prompt's skills index;
`load_skill` and `read_skill_file` never ask (D11) and are audited as "allowed without
prompt".

Automated: `e2e/m5_skills.py` (create in the editor, the model loads it, audit row,
disable, delete).

**Manual test — both machines:**
1. *Settings → Skills → New skill*, name `magic-number`, body "The magic number is 4242". *Validate* → no problems → *Save*.
2. New chat: "What is the magic number? Load the skill." The `load_skill` card runs without asking; the model answers 4242.
3. *Import zip…* with a skill zip from another agent app; it appears enabled. Disable it: it disappears from new chats' prompts.

**Dependencies:** none new.

---

## M8 — Polish (in progress)

**Works:** opt-in updater (only in builds with the updater key; *Check now*, *Install and
restart*, automatic check off by default); keyboard shortcut sheet (Ctrl+/ or Cmd+/) and
chat-list toggle (Ctrl+B or Cmd+B); the 8.1 message for models without tool support;
third-party notices generated and checked in CI; an offline gate (`e2e/m8_offline.py`:
no outbound request through any proxy during start, a local chat and every settings
page).

**Manual test — both machines:**
1. *Settings → General → Updates* says this build cannot update itself (no key yet). After you set up the key (README, "Signing") and tag a release, a release build offers *Check now*.
2. Press Ctrl+/ (Cmd+/): the shortcut list opens, including your stop hotkey. Ctrl+B hides and shows the chat list.
3. Offline: turn off Wi-Fi / unplug the network, start the app, load a downloaded model and chat; everything works and nothing reports a network error except Hugging Face search.

**Known issues:** the offline gate sees proxied traffic only (not direct DNS lookups); long
chats show their latest 80 messages with a "Show earlier" button rather than a
virtualised list; Rust types
are mirrored by hand in `src/lib/types.ts` rather than generated.

---

## M6 — Browser tool

**Works:** the `browser` tools (navigate, snapshot, find, read text, click, type, fill a
form, press a key, screenshot, back) drive the agent's own Chrome or Edge window with a
separate profile in the app's data folder, over a pipe (no listening port) or, as a
setting, a loopback port. Every document request on every tab and frame is checked in
the browser itself: `http`/`https` only, and this computer's own addresses (including the
local model server) and, unless allowed, private networks are blocked, including after
redirects. Form submissions and anything that sends data with a navigation are held until
you approve them; leaving a site asks; file choosers and downloads are refused. Settings →
Browser chooses the browser, the port mode, private networks and strict sites.

Automated: the browser crate's tests run against a real Chromium (blocking, held
submissions released or failed, pop-ups, Stop releasing held requests); `e2e/m6_browser.py`
checks the integration in the app.

**Manual test — both machines** (a vision model is not required):
1. Tools panel → switch on Browser. Ask: "Open https://example.com and tell me the page's heading." A navigate card shows the host; allow it. A Chrome window opens (not your own profile: no bookmarks, not signed in); the model reads the heading.
2. Ask it to search something on a site with a search box: typing asks per site; pressing Enter in the search field shows a submission card (D8). Deny: the page does not change.
3. Ask it to open `http://127.0.0.1:8080` or your router's address: refused without a card (private networks are off by default).
4. Press Stop while a card is open: the card closes and the held request is cancelled.

**Known issues:** see CHANGELOG (M6).

---

## M7 — Computer use

**Works (Windows and macOS only):** screenshots of one monitor with this app's windows
left out and a cursor marker drawn in, resized exactly as llama.cpp resizes them for the
model; clicks, moves, drags, scrolls, typing and key combinations through one injector
that refuses any input that would land on this app (including WebView2's processes),
re-checks the target app before every event, refuses elevated windows on Windows, and
releases every held key and button on Stop, a panic or exit. Grants are per kind of input
and per application (D7); terminals, system settings and password managers always ask.
While it acts, a red border and an "AI is controlling this computer" bar with a Stop
button stay on screen. Settings → Computer use shows the macOS permissions and sets each
model's coordinate convention.

**Manual test — Windows PC** (needs a vision model with known coordinates, e.g. a
Qwen2.5-VL-based model such as UI-TARS-1.5-7B):
1. Settings → Computer use → pick the model, choose "Pixels of the image the model saw", tick the confirmation, Save. Tools panel → switch on Computer.
2. Open Notepad. Ask: "Take a screenshot, then click in the Notepad window and type hello." Approve the screenshot card, then the pointer card (it names notepad.exe) and the keyboard card.
3. While it works the red border and the Stop bar are visible; press Ctrl+Alt+Backspace: everything stops, no key stays pressed.
4. Ask it to click on harnissUI's own window: refused ("covered by this app's window").
5. Run an admin Command Prompt and ask it to type there: refused ("running as administrator").

**Manual test — Mac:**
1. Settings → Computer use: Screen Recording and Accessibility show "not allowed"; *Open System Settings*, allow harnissUI, come back, *Check again* (restart the app if macOS asks).
2. Steps 1–4 as above with TextEdit; the stop hotkey is Ctrl+Option+Delete.
3. Put TextEdit in full screen: the border and Stop bar still show on that Space.

**Known issues:** unsigned macOS builds lose the two permissions on every new build (a
Developer ID fixes that). The automated calibration test of 9.3 is replaced by a
confirmed per-model convention for now (see the 4pm report).
