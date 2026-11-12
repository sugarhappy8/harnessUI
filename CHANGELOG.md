# Changelog

All notable changes, by milestone. Dates are UTC.

## 0.0.5 (2026-11-12)

The owner asked for the app to "stop asking me to allow everything", named *Full access*, and for models to be able to make their own folders, move around the computer, delete and organise, only when Full access is on. The four things first planned for 0.0.5 (context per model, plain-language loading settings, talking to Claude and Codex, model comparisons) moved to 0.0.6; the plan and the half-built work are in `docs/dev/0.0.6/`.

### Added
- **Full access.** The shield button in a chat's header now opens a menu with three modes: *Ask first*, *Allow all* and *Full access*. In Full access nothing asks: the model runs commands, reads, writes, edits and deletes files, browses and uses its other tools without a card, and the file tools reach the whole computer, not only your approved folders. Only the app's own folders (its database, logs and install folder) and the folders where the system keeps saved keys stay out of reach of its file tools; a program the model runs has your full rights. Turning it on shows a plain warning and asks you to confirm; turning it off takes one click and reaches a reply that is running at once. *Settings → Permissions → Approvals* chooses the mode every chat starts in; if that is Full access, the app asks again each time it starts, because a program could have changed the setting. Calls that ran this way are listed in the audit log as "allowed by Full access". [SECURITY.md](SECURITY.md) says what it means for safety: a web page or a file that tricks the model is no longer stopped by an approval card.
- **Folder tools in Full access.** `make_folder`, `move_path` and `delete_folder` let the model create folders, move and rename files and folders, and delete whole folders. A move never replaces anything, and a move into an existing folder puts the item inside it. A folder that holds the app's own data (your home folder does) or a drive's top is never moved or deleted. The tools come with *write_file* and are offered only while the chat is in Full access.

### Changed
- `list_dir` and `search_files` no longer go into the app's own folders when they lie inside the folder being listed or searched (in every mode); a listing names such a folder and marks it "not reachable".
- A terminal command that names the app's own folders (its database, logs or program files, the app's identifier, or a path that reaches them through `~`, an environment variable, `..` or a wildcard), or that deletes, moves or locks a folder holding them, is refused, in every mode.
- The file tools leave devices, pipes and sockets alone; reading one could hang or never end. A search skips `/proc`, `/sys` and `/dev`.
- The folders where the system keeps saved keys, and on macOS the whole app bundle, are among the folders tools never reach.

## 0.0.4 (2026-11-12)

The owner asked for thirteen things in 0.0.4 and split the work into three steps. Step 1 brought an Allow all button, artifacts beside the chat, web search with sources, and a short tool list per task. Steps 2 and 3 brought helpful tool errors and manageable tool output, a visible and resumable task, file and symbol mentions, pinned instructions and a context inspector, tested settings per model, and coding benchmarks. All of it is built and was reviewed; the developers' reports and the two independent reviews are in `docs/dev/0.0.4/`. *Settings → General* now also shows which build it is.

### Added
- **Allow all.** A button in the chat header (next to Tools) switches a chat from *Ask first* to *Allow all*: the model then reads, writes and edits files in your approved folders, runs commands, commits with git, browses and uses its other tools without a card for each. A confirmation says exactly what still asks, and *Settings → Permissions → Approvals* has the same switch for every chat (off by default). **What still asks, whatever the mode:** deleting files or folders however it is done, and leaving a file empty; throwing away work (`git reset --hard`, checking files out over your changes, a forced push) and disk, system, registry and administrator commands; commands that hide what they run; writing to a `.git` folder, shell start-up files, git and ssh settings or autostart folders; paying, sending and publishing (web buttons that say so, and `git push`); typing or clicking in a terminal, in system settings or in a password manager; tools from added servers that are marked destructive. After the model has read a web page, a file or a tool result in the same turn, sending data out also asks: opening another site, typing into a page, tools from added servers, git pull and clone, and commands that use the network or can hide what they run. Folders you have not approved stay out of reach of the file tools. Calls that ran this way are listed in the audit log as "allowed by Allow all". What it cannot see is said in the confirmation too: the app checks the command it is about to run, not what a script or a program does once it is running.
- **Artifacts beside the chat.** The model can make documents (Markdown), pages (HTML), pictures (SVG) and diagrams (Mermaid, bundled, no network). They open in a panel beside the chat with a preview and a source view. You can edit the source and save it as a new version, step through versions, restore one, export to a file, rename and delete. What goes wrong in a preview (script errors, blocked requests, a diagram that does not parse) is sent back to the model with line numbers so it can fix its own page, and "Ask the model to fix these" writes the problems into the message box.
- **Web search with sources you can check.** The model can search the web and read the pages it finds; each asks first, and "Allow for this chat" covers the rest of the chat. Answers cite sources as numbered chips; a click opens the Sources tab with the page's address, when it was read, the exact text the model was given, and a check of whether a quoted sentence really is in that text. A number with no source behind it is marked. Official documentation is listed first, and when the question names a library your project uses, the version in use is added to the search. *Settings → Search* chooses the service: DuckDuckGo (no key), Brave or Tavily (your key, kept in the system keychain), or your own SearXNG address.
- **A short tool list per task.** Instead of sending every tool description with every request, each turn shows the model the tools the task is about (file tools always; git when you talk about commits; the browser when you name a site), 14 at most. Two small tools let the model look up and run the others when it needs them. The Tools panel still decides which tools are on, and marks each as "shown" or "on request"; one switch there turns the choosing off. On a model with a small context this roughly halves what tools cost on every request.
- **Tool errors that say what to do next.** When a file, terminal, git or skill tool fails, the model is told why and what to call instead. An edit whose text appears twice lists both places with line numbers and offers two new choices, change match number N or change all of them; an edit whose text is not found shows the closest lines and says when only spaces, line endings or capital letters differ; a missing file comes with similarly named files; a failed command shows its exit code and the lines that look like errors. When explaining a failed edit means looking inside the file, the app treats it as reading that file: it asks first exactly as a read would, and changes nothing.
- **Long tool output you can manage.** A long result is shortened for the model (for a command: its start, the lines that look like errors, and its end), the whole text is saved with the chat, and the model is told how to read more of it, by line range or by searching for a word. The tool card has a **Full output** button that opens the saved text.
- **A task you can see, correct and resume.** The **Task** tab beside the chat shows what the model is working on: the goal, constraints, steps, the next action, the files changed with the file tools and the tests, builds and checks that ran, each passed or failed. You can correct any of it, and **Pause**, **Resume**, **Mark done** or **Clear task**. The task is saved, so after a restart Resume picks the work up where it stopped. The model is reminded of the task when its picture may be out of date: after you changed, paused or resumed it, after the app started, after old messages were summarised, and every 8 messages.
- **Mention files and functions with @.** Typing `@` in the message box lists files in your approved folders and the functions, classes and types defined in them (eleven languages). A chosen mention sends that file, or that definition with its path and line numbers, along with your message, and shows as a chip on the message. Only files in folders you approved can be mentioned; the app's own folders, skill folders, `.git`, `.ssh`, shell start-up files, files over 1 MB and binary files never can.
- **Pinned instructions and a view of what the model gets.** The **Context** tab keeps short instructions in front of the model in every message, for one chat or for all chats (up to 20, 1,000 characters each); the pin button on one of your messages pins it. Summarising old messages never drops a pin. Below the pins, "What the model gets next" shows exactly what the next message would send, section by section with token counts, what would be left out next, and a Copy all button. It sends nothing.
- **Tested settings per model.** *Settings → Local models → Tested settings → Check this model* gives a model three short tasks (read a file, make an edit, recover from a failed edit) under a few settings, in a temporary folder, with no commands run. The settings that work best are saved for that model and used in your chats unless you chose a setting yourself; *Forget these settings* removes them.
- **Coding benchmarks.** *Settings → Diagnostics → Coding benchmarks* runs eleven small coding tasks (Python, JavaScript, config files, documentation) and shows per task whether it was solved, broken edits, failed commands (commands that ran and reported a failure, such as failing tests), tool failures (calls that could not run or did not finish), blocked actions, steps, tokens and time. Runs are saved on this computer and two can be compared; a run you end early is kept and marked so. The model's code runs on your computer with your rights and is not sandboxed; the confirmation says what that means. Run benchmarks only with a model you trust.
- *Settings → General* shows the build (the first characters of the commit) next to the version.

### Changed
- New chats start with Artifacts and Web search on, next to Files and Skills.
- In Allow all, a command line that only has a glob, a redirection to a file or a variable as an argument is judged by the programs it names. It does not ask merely because it could not be fully broken down.
- Repeated searches in one turn no longer ask again once you have allowed searching in the chat.
- In Allow all, more commands keep asking: `awk`, `sed`, `tar`, `zip` or an editor told to run another command through its own language or options. After the model has read outside content, `busybox` and `toybox` running another program, and `python -m http.server` (a small web server), ask too. `node --test`, and an interpreter asked only for `--version` or `--help`, now count as plain commands.
- A page the model writes after you attached or mentioned a file, or added a picture, now also waits for **Run scripts**, as one written after it read a file or a web page does.
- `edit_file` refuses a file that is not UTF-8 text and suggests a command instead. Before, it rewrote the characters it could not read, so the file lost them.
- Computer use: the Delete key in a file manager always asks, and a grant for keyboard input to that file manager does not cover it.
- The app's own pages now lock down JavaScript's base object themselves; the framework's switch for it is off, because on Windows it also applied inside preview frames, where the diagram library could not start.

### Fixed
- **A model loaded with a small context (4,096 tokens) could not send a message in a new chat.** With Artifacts and Web search on by default, the tool descriptions took more of such a context than a turn may start with, and every message was refused with "These 14 tools need about 2,165 tokens". The tool list is now also held to the model's context: when it would not fit, it is made again by size, the core file tools first. Found by the end-to-end test that loads a real 0.6B model.

### Safety work behind the previews
A preview is a page the model wrote, shown inside the window that also approves actions, so most of the work on it was isolation (ARCHITECTURE.md 13.8):
- Previews load from an address of their own, in a sandbox with no storage, no access to the app's page and no way to make a request. The first design used an address that, on Windows, the framework would have treated as the app's own: a page the model wrote could have called the app's commands. That was caught by reading the framework's source before anything shipped, and the address changed.
- When the app starts it checks its own sandbox with a page that tries to call the app, make requests and find storage. Scripts run in previews only after that check has passed; otherwise pages are shown without running.
- A preview address works once, for a frame the app has just made. An address copied from anywhere opens nothing, so a preview cannot become the page of the whole window.
- A page the model writes after it has read a file, a web page or a tool result in the chat, or after you attached or mentioned a file or added a picture, is shown without its scripts until you press **Run scripts**. Pictures and diagrams never run scripts of their own.

### Safety work in steps 2 and 3
Two independent reviews read the new code: one the tool errors, saved output, task, mentions and pins; the other the model check and the benchmarks. What they found and what was done:
- **A failing edit could reveal a file's text without approval.** Some answers to an `edit_file` that could not be made (a match number out of range, an edit that changes nothing) came back before any approval and depended on what the file said, so a model could have guessed a secret in a file one character at a time. Now nothing the app says before approval depends on a file's text: every such failure is treated as a read of the file, asks like a read, and is answered afterwards.
- **`edit_file` damaged files that are not UTF-8.** It now refuses them (above).
- **A mentioned file could be swapped for a link between the last check and the read.** The file is now opened without following a link and checked to be the same file that passed the checks. What remains is listed under Known limits.
- **A mention made of one very long line was cut without saying so**, and the "Shortened" line counted a line shown only in part as shown. Both now say what was left out and where to read on.
- **The Full output button trusted the last line of a result,** so a file could make it point at another saved output of the same chat. It now asks the app which output it saved for that call.
- Three commands (`get_task` and the two mention commands) now check that the main window called them, as the others do.
- **The benchmark did not keep the model in its folder,** although the confirmation said that commands that delete or go online were refused. The review showed scripts the model wrote, and single commands such as `sed`, `awk`, `tar` and `zip` running commands of their own, reading, writing and deleting outside the folder. The benchmark's commands are now a short allow-list: Python or Node.js running a file of the task's folder, the test runners (`python3 -m unittest`, `python3 -m pytest`, `node --test`), and commands that look at files (`ls`, `cat`, `grep` and a few more), with no wildcards, no redirection of any kind and nothing in the background. Every other program is refused, and the model is told the list. The confirmation and the audit log now say plainly that the model's code runs on your computer with your rights and that the app cannot see inside it. Allow all keeps asking for the same programs (above).
- **A benchmark's check could be passed without solving the task,** by code that switched off the tests or printed the pass line itself, **and could hang so that "End the benchmark" did nothing.** Now a task's script only prints the results of the model's code, and the app itself compares them with the expected values built into the app, so tampering with the tests changes nothing. The check cannot hang, stops when you end the run, and each check has its own row in the audit log. Left-over temporary folders from an interrupted run are removed when the next run starts. All eight commands of the check and the benchmark now check that the main window called them.
- **The model check said "Nothing leaves this computer" also for a model on an endpoint.** For an endpoint, both confirmations now say that the tasks and the model's replies go to that endpoint.
- The reviews found no way around the approval rules in the task, the mentions' folder checks (apart from the window above) or the pins: no tool can write a pin, the task's changed files and checks come from the app's own record, and command output never enters the task note.

### Known limits
- **Nothing of 0.0.4 has been tried by hand on Windows or macOS.** All of it was built and tested on Linux: about 1,700 unit and integration tests, 248 tests of the interface, 28 scripted end-to-end scenarios against the real app (one or more for each new feature), and the chat, agent and model-check suites with a real small model. The Windows and macOS builds come from CI. On those systems the preview sandbox rests on the framework's source as read and on the start-up check; if the check fails there, previews show without scripts and say so.
- **DuckDuckGo may turn the app away.** It refused automated searches from the machine this was built on, so a complete search through the default service has not been seen working. If a search says DuckDuckGo refused it, choose Brave, Tavily or SearXNG in *Settings → Search*. Brave and Tavily were tested only against their documented answers, without keys.
- A script in a preview can still open a connection to a server (no request is sent, but the server's name leaves). This is why pages written after the model read outside content wait for "Run scripts".
- Allow all and the benchmark's command rule read the command line. They do not see what a script file or a program does once it runs, including a script the model wrote a minute earlier.
- **A benchmark is not a sandbox.** When it runs code the model wrote (the task's tests, which load the model's files; the model's own scripts; the app's check of the result), that code has your full rights: it can read, change or delete any file you can, and go online. The allow-list only limits the command lines the model types. A program that such code starts on its own can keep running after the benchmark ends. Such code can also read the app's check script and the app's files. Real confinement needs the operating system's own sandbox and is later work. The model check is not affected: it runs nothing the model writes.
- Tested settings and benchmarks were run against a real local model once, the 0.6B test model on the CPU: the check finished by itself, left nothing behind and changed no file outside its folders, and ending a benchmark stopped it in a fraction of a second and kept the partial run. That shows the check works with a real model server, not that a small model passes it: that model never passed the error-recovery task, and the setting chosen for it differed between two runs. A check or a benchmark does not wait for a chat reply on the same local model; both go to the server and wait there. The Windows parts of the benchmark's command list have not been run.
- There is no switchable "tool calls read from text" setting to test; the check only records how a model's tool calls arrived.
- **Mentions:** when a mentioned file is opened, a folder above it that is swapped for a link at that moment is still followed (only the last part of the path is protected; the check that it is the same file catches most such swaps). A hard link inside an approved folder to a file outside looks like an ordinary file (the file tools have the same gap). On Windows there is no check that the opened file is the one that passed the checks. Edit-and-resend drops mentions, as it drops attached files. The index notices changed files when you search again after 3 seconds; it does not watch the folder. The search times were measured in an unoptimised build.
- After `update_task`, the turn counts as having read outside content, so in *Ask first* a command you had allowed for the chat asks again in that turn.
- When the task note is sent, a local model reads the previous turn again (the note is not stored with the message, so the server's saved state stops one message earlier). This is why the note is not sent with every message.
- The context inspector's token numbers were seen only with a stand-in endpoint, not with a local model's exact count. Section estimates and the total can differ by a few tokens. For an endpoint, the inspector asks it for its context size each time it refreshes.
- A database made by an earlier 0.0.4 development build has to be deleted; one from 0.0.3 upgrades.

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
- **Browser: the first navigation in a new tab could skip the request checks.** A tab counted as usable as soon as the browser reported it attached, while its requests were put under the app's checks by a separate task a moment later. A navigation sent in that moment was not paused at all: its redirects were not checked against the private-network and loopback rules, and the tab got no granted site, so a pop-up it opened later waited for an approval nobody was asked for. In practice the moment is microseconds long; it showed as a browser test that hung on two CI runs in a row (Windows, then Linux). Now no browser call touches a tab until its set-up is complete: `Fetch.enable` answered, page events switched on, and the frame tree recorded. A new test holds the moment open for 1.5 seconds: before the fix the browser followed a redirect to a loopback address, with it the redirect is blocked. (A first version of the fix released calls as soon as the checks were on; CI then showed a first snapshot of a page the tab had not reported yet, so the release point moved to the end of set-up.)
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
