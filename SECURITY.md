# Security

harnessUI lets a language model running on your computer use tools: read and write
files, run commands, browse the web and, if you turn them on, control the mouse and
keyboard, look inside program files with rea, or hand a task to Claude Code or Codex. You
can also let other apps on this computer use the loaded model. This document describes
what protects you, what does not, and how to report a problem. The design behind it is in
[ARCHITECTURE.md](ARCHITECTURE.md) section 13.

## Reporting a vulnerability

Please report security problems privately through GitHub's **Report a vulnerability**
button on this repository's Security tab, not in a public issue. Include what you did,
what happened, and the app version (Settings → General → About). We aim to answer within
a week.

## What the model can and cannot do

**Model output is untrusted.** Whatever a model writes can only become text in the chat or
a request to use a tool. Every tool request goes through one policy engine before
anything runs.

**Content the model reads is untrusted too.** Web pages, file contents, tool results and
attachments are data, never instructions. They are wrapped in delimiters, control tokens
inside them are broken up, and the model is told that tool output is data. This lowers
the odds of prompt injection. It does not guarantee anything: small models follow
injected text more readily than large ones.

**Untrusted content cannot change permissions.** This is enforced by how the app is
built, not by the model's good behaviour:

- No tool, built-in or external, can change grants, approved folders, enabled tools or
  settings (model settings, Fine control, the sharing switch and the decompiler choice
  included). Those change only through commands the user interface sends.
- The agent loop holds a read-only view of policy state.
- An approval is resolved by a random request id created in Rust and delivered only to
  the main window.
- Computer-use input is refused on the app's own windows, so a model cannot click
  *Allow* on its own approval card.

**It can use permissions you already granted.** If you allowed reads in a folder and also
allowed an action that sends data out, data can leave without a new card. To close the
common routes, once a turn has read untrusted content, every outbound action asks again
(decision D9). What remains is a terminal command or external tool you explicitly allowed
with wide scope.

## Approvals

Each tool call becomes an *action*: what it touches (the canonical path, the resolved
program, the punycode host), a preview, a grant scope and a risk level. The evaluation
order is:

1. Tool not enabled in this chat, or its server not ready → refused.
2. Hard limits → denied without a prompt: a path outside approved folders or inside a
   never-reachable folder, a working folder outside approved folders, a blocked URL
   scheme or address.
3. Always-ask actions → a card with *Allow once* and *Deny* only.
4. A call the model wrote as text instead of a real tool call, unless it only reads → a
   card labelled "recovered from text".
5. Untrusted content in this turn and an outbound action → a card that says why.
6. A grant for this chat covers it → allowed and logged as "allowed by grant".
7. Otherwise → a card with *Allow once*, *Allow for this chat* and *Deny*.

*Allow for this chat* grants live in memory only: they end when the app quits or when you
choose *Tools → Forget everything I allowed in this chat*, and are never written to disk.
A card that offers only *Allow once* can never create a grant, whatever answer reaches it.
Stop (the button, Esc or the global hotkey) closes an open card and cancels the call.

The approved action travels with the call, and the tool re-checks it immediately before
its side effect: a file that changed between the preview and the write is not written,
and a symlink planted after the preview is refused.

**Always ask, whatever you granted:** deleting files, emptying a file, writing inside
`.git`, shell start-up files, git and ssh configuration, autostart locations, and the
settings of Claude Code and Codex (`.claude/`, `.codex/`, `.mcp.json`, `~/.claude.json`);
for the terminal, any delete, elevation, disk and system commands, `git push`, `git reset
--hard`, `git clean`, inline interpreter code, wrapper commands, a command that starts a
coding assistant such as Claude Code or Codex, and anything the parser cannot fully
understand; handing a task to Claude Code or Codex with the Coding apps tools; form
submissions in the browser; and external tools their server marks destructive or that you
marked always-ask.

**Cards are built to be read safely.** Every string on a card was chosen by a model or a
server. Control, right-to-left-override and zero-width characters are shown as visible
escapes. *Allow* never has default focus or an Enter binding, and clicks are ignored for a
short moment after a card appears.

## Allow all and Full access

Every chat starts in *Ask first*, the order above. You can switch a chat, or the mode every
chat starts in, to one of two other modes. Only you can: the switch is in the chat's header
and in *Settings → Permissions*, both behind a confirmation, and no tool or model output
can reach it.

- **Allow all** (0.0.4) answers most cards for you. Deleting, throwing away work, system
  commands, sensitive places, paying and publishing still ask, and so does sending data
  out after the model has read outside content in the same turn. Since 0.0.6, starting
  Claude Code, Codex or a similar coding assistant asks too, from the terminal or with the
  Coding apps tools. It is a guard built on the command parser, not a sandbox.
- **Full access** (0.0.5) asks for nothing. The model can run any command, read, change,
  move and delete any file or folder on the computer (deleted files do not go to the
  trash), use the browser and the accounts you are logged in to, and hand tasks to Claude
  Code or Codex, which then work online with your account. Only the hard limits
  remain: a tool that is off stays off, and the app's own folders (its database, logs and
  install folder) and the folders where the system keeps saved keys stay out of reach of
  its file tools. A saved "every chat starts in Full access" is asked again each time the
  app starts, because a program could have written that setting.

What Full access means for safety, plainly: prompt injection is no longer stopped by an
approval card. A web page, a file or a tool result that tricks the model can make it do
anything your account can do, without asking. The app still marks such content as data,
but a model that follows it anyway is not stopped. A command that names the app's folders,
or deletes or moves a folder that holds them, is refused in every mode, also through `~`,
environment variables and wildcards. But the app reads only the command line: a program
the model runs, or a command that builds the path from pieces, can still read the app's
database or ask the system keychain for your keys, as any program you run can. On Windows
any program you run can read Credential Manager entries without a prompt. Use Full access for a task you watch, with a model you trust, and switch it off
afterwards (one click). Calls that ran this way are in the audit log as "allowed by Full
access".

What the 0.0.6 features reach in each mode is in their own sections below: [other apps
using the loaded model](#other-apps-using-the-loaded-model), [Claude Code and Codex from a
chat](#claude-code-and-codex-from-a-chat) and [reverse engineering](#reverse-engineering).
Model settings, Fine control, the usage line and the folder chip are not tool actions: no
approval mode lets a tool change them through the app (a program the terminal runs has your
rights, as above).

## Files

Tools only reach folders you approved, for all chats or for one. The app's own data
folder, its install folder and the agent browser's profile are never reachable, whatever
you approve. Every path is checked lexically first (so a network path such as
`\\server\share` is refused before Windows tries to connect to it), then resolved through
symlinks and junctions and checked again, case-insensitively where the volume is.
In Full access the approved folders are the whole computer; the app's own folders stay
out of reach, a folder that holds one of them is never moved or deleted by the folder
tools, and listings and searches do not go into them.

Since 0.0.6 the chat header shows which folders the tools can use: the chat's working
folder ("harnessUI + 2 more"), "No folder · Choose", or "Whole computer" in Full access. Its
menu lists the full paths, with hidden characters shown as visible escapes. In Allow all it
says that terminal commands are not held to these folders; in Full access it says that a
program the terminal starts could still reach harnessUI's own folders.

## The terminal is not a sandbox

There is no operating-system sandbox in version 1. An approved terminal command or an
external MCP server runs with your permissions. It can read and change anything you can,
including this app's database (its approved-folder list and audit log). "Changed only by
UI commands" and "append-only" are properties of the app's own code, not of the files on
disk. The terminal's command parser is a guard against accidents and obvious abuse, not
a security boundary, and the app says so when you turn the terminal on.

The parser reads a line the way the shell runs it (0.0.6): a backslash at the end of a line
joins it to the next, and a line with a carriage return in it counts as one the parser
cannot fully understand, so Ask first always asks and Allow all searches all of its text for
the things it always asks about (a harmless line with a carriage return runs there without a
card).
Before 0.0.6, `echo \` followed by a Windows line ending and `rm -rf x` read as a single
`echo` command: a grant for `echo` covered it, and Allow all ran it without a card.

**Commands that start a coding assistant (0.0.6).** In Ask first and Allow all, a command
line that starts Claude Code, Codex or a similar program, or carries a flag that turns off
their permission checks, asks with *Allow once* and *Deny* only and counts as sending data
out. In Full access it runs. The rule looks through wrappers (`env`, `timeout`, `sudo`,
`cmd /c` and many more), package runners (`npx`, `uvx`, `pipx run`, …), words such as `run`
and `exec` of other programs, wildcards, braces, variables set on the same line and names
split over two lines. It reads the text of the line, so it is a guard, not a sandbox. These
stay unseen by design:

- a copy of such a program under another name, or a link to it;
- a name built from pieces, kept in a file or a script, or passed in from outside the line;
- a program named by a setting (`git -c core.pager=claude`);
- a project's own script or task (`make claude`, `npm run claude`);
- a program that runs on another computer (`ssh host claude`);
- a program not on the app's list that runs the next word it is given without `run`,
  `exec` or `--` before it;
- some wildcard patterns that fix too few letters of the name
  (`uv run ~/.local/bin/??????`), and a few such patterns after options that take two
  values or after another program's own subcommand. The full name in those places is seen.

## External MCP servers

Adding a server shows a warning that it runs with your permissions, and the
acknowledgement is recorded. Every call to it goes through the policy engine. Its tool
descriptions and hints are untrusted: a server saying a tool is read-only never skips a
prompt. New tools, and tools whose name, description or schema changed, start disabled.
Servers get a short allow-list of environment variables plus the `env` from their config,
not your whole environment. Every value in `env` and `headers`, and credentials found in
arguments and URLs, are moved to the system keychain when you add the server; the saved
config keeps `${secret:…}` placeholders, resolved only when the server starts. Removing a
server deletes its secrets. An approval for an external tool is bound to that tool and those
exact arguments (or the whole tool, if you ticked "allow any arguments"); a tool that
changed since you reviewed it is switched off again and loses its grants. Servers that
require OAuth sign-in are not supported in version 1.

## Other apps using the loaded model

*Settings → Other apps → Let other apps on this computer use the loaded model* (0.0.6,
experimental, off by default) lets Claude Code, Codex and the Claude app use the model
loaded in harnessUI. Only you can switch it on: no tool or model output reaches the switch,
the port or the key, in any approval mode.

- harnessUI answers on `127.0.0.1` only, on a port picked at random the first time and then
  kept. llama-server stays behind it on its own random port with its private key.
- Every request needs the sharing key. It is kept in the system keychain, shown only when
  you press *Show key*, and *Make a new key* replaces it (the old one stops working at
  once). The key, the address the request was sent to and the absence of a web page's
  `Origin` header are checked before anything else is read, so web pages cannot use the
  model.
- Requests are rebuilt, not passed through: only the fields each API defines, at most
  32 MB, a capped answer length, and pictures, audio, video and files only inline, never as
  a link the model server would fetch. WebP pictures are turned into JPEG or PNG by the same
  separate helper as in chats, at most 32 in one request. At most 64 connections at once;
  one that never sent the key gives way first.
- Answers have their timing and cache figures removed, because those could show how much
  of a request matches what your own chats sent. The model file's path never appears.
- Every request that asks the model for an answer is one row in the audit log (server
  `other-apps`), with what the app says it is and the token counts, never the request text.
  Token counting writes at most one row a minute, and the list of models, which harnessUI
  answers itself, writes none. Requests without the key are only counted on the page.
- The approval mode does not change sharing: it is not a tool action. What the other app
  then does is up to that app and its own permission settings; harnessUI's approval cards
  do not cover its tools.

What it does not protect against:

- Setting an app up stores the key as plain text in that app's settings, so a program that
  can read those settings can use the model while sharing is on.
- While harnessUI is closed or sharing is off, another program on this computer could take
  the port, pretend to be harnessUI and receive what that app sends, including the key.
  The setup card says so.

## Claude Code and Codex from a chat

The *Coding apps (experimental)* group in a chat's *Tools* panel (0.0.6) lets the local
model hand a task to Claude Code or Codex installed on this computer. It is off in every
new chat, and helpers (subagents) never get it.

- When you switch the group on, the app finds the programs without running them and shows
  their full paths. A program inside an approved folder is refused (a project could have
  put it there), and before each run the file must still be the same file.
- **Ask first and Allow all:** every hand-off shows a card with *Allow once* and *Deny*,
  even after *Allow for this chat* and whether or not the model read outside content. The
  card names the program, the folder, what the app may read or change, that the task runs
  online with your account and uses your plan or credit, and that what the program reads is
  sent to Anthropic (Claude Code) or OpenAI (Codex). When the program is set up to use the
  model in harnessUI, the card says the task goes to your local model instead.
- **Full access:** it runs without a card. The checks above still apply, and the working
  folder must be one you approved.
- The task is handed to the program privately, never on its command line, where other
  programs could see it, and the command line itself is fixed. Claude Code may only read
  files in the chat's working folder (or read and change them there, if the model asks to
  edit), and its own commands and the scripts it can be set up to run by itself are
  switched off. Codex runs in its own sandbox: it can read files elsewhere on this computer
  that you can read and, when the model asks to edit, change files in the working folder and
  the temporary folder and run commands inside that sandbox. No switch that turns off their
  permission checks can be added. On Windows, Codex's sandbox has not been tried.
- Their answer is outside content, shortened like any other tool result.
- Writing to the settings of Claude Code or Codex asks in Ask first and Allow all, whichever
  tool does it (see *Always ask* above).

Once started, Claude Code and Codex keep their own limits; harnessUI only chooses the
command line. Codex was never run while this feature was built. Stop ends the program and
what it started; something it started to run on its own in the background can keep running.

## Reverse engineering

*Settings → Reverse engineering* (0.0.6) downloads rea, a free tool that looks inside
program files, with Node.js to run it and a decompiler: Ghidra with Java on Windows, Kuna
on a Mac. Nothing comes with the installer, and nothing is downloaded until you click
*Download* there, after the licences are shown. After that it works without the internet.

**Every download is checked.** Each download's address, size and checksum are built into
the app (`reverse.lock.json`) and checked before anything is unpacked. Unpacking refuses
files that would land outside the folder, devices and, on Windows, links, and leaves no
program that other users of the computer could change. Where the publisher signs its files
(rea, Node.js, Java), the signature was checked too when the app was built; for Ghidra the
check is the checksum GitHub publishes. Kuna's releases are not signed, so its checksum
comes from the file as first downloaded. A release build offers a part only when every check
its publisher offers has been recorded. Ghidra's checksum on GitHub could not be read where
0.0.6 was built, so until it is confirmed a release build shows Ghidra as "Not available in
this version of harnessUI." and Windows has no decompiler. A CI step compares Ghidra's
download with the checksum GitHub publishes before it may be recorded.
<!-- TODO(lead): confirm after WP9e -->

**Files only.** The model gets 37 rea tools. Each reads one file or folder the model names
(in Ask first you approve it on a card; in Allow all and Full access it opens without one,
see below): what a program is made of, its text and functions, and readable code worked out
by the decompiler. None runs the file, attaches to a running program, debugs, records the
screen or the network, or writes outside rea's own folder. rea has about 140 more tools that
do some of those things; only the app's list keeps them from the model, and it is checked
twice before anything reaches rea. rea, its programs, its cache and its home folder sit
inside harnessUI's own data folder, which the file tools never reach and no rea tool may be
pointed at, in any mode.

**What it reaches in each mode:**

- **Ask first:** opening or checking a file asks (*Allow once*, *Allow for this chat*,
  *Deny*). *Allow for this chat* covers rea opening other files in that approved folder in
  this chat, nothing else; a grant to read files does not cover rea, and the other way
  round. Further questions about the file already open need no card.
- **Allow all:** files in the chat's approved folders open without a card.
- **Full access:** any file on the computer opens without a card.
- **In every mode:** harnessUI's own folders, the system's key stores, network paths,
  devices and pipes are refused without a card, and so is a folder that holds harnessUI's
  folders. Outside Full access, a path outside the approved folders, or a folder holding a
  link that leaves them, is refused too. A folder is checked entry by entry (at most 50,000
  entries and 32 levels) before rea sees it. Reading outside content adds no card, because
  no rea tool sends anything out.

Dropping a program file on the message box only writes its path into your message. This
works for a file in the chat's folders, and in Full access for any file outside harnessUI's
own folders. A path with hidden characters is never put in.

**Each chat keeps to its own file.** The file open in rea, and the results it gave, belong
to the chat that opened it; another chat cannot read them. Right before rea reads a file,
the app checks once more that it is the same file and still inside the chat's folders.
rea's output is outside content: it is marked as data, shortened like other tool output,
and the paths of harnessUI's own folders in it are hidden.

**rea is kept quiet.** It starts with almost none of your settings: no proxy settings, and
its home and temporary folders inside harnessUI's data folder. It never runs its own setup,
so it writes nothing into the settings of Claude Code, Codex or other apps. Ghidra's
analyser that looks up debugging-symbol files is turned off. On a Mac it never starts before
Apple's command line tools are installed, so Apple's install window cannot appear in the
middle of a chat. CI runs rea with the real decompilers (Ghidra on Windows, Kuna on macOS,
both on Linux) and fails if any of their processes tries to reach anything but this
computer.
<!-- TODO(lead): confirm after WP9e -->

**Limits:**

- rea, Node.js, Kuna, Ghidra and Java run with your rights, like any tool server you add.
  The app decides which rea tools exist and which files they are given, not what those
  programs could do.
- A decompiler reads a hostile file in full. A file made to attack Kuna or Ghidra could take
  it over, and it would then have your rights and network access. Open programs from people
  you don't trust only on a computer you can wipe.
- If harnessUI crashes, Kuna or Java can keep running for a short while on macOS and Linux.
- A link made inside a folder after the folder was checked is not seen until the next
  call. A hard link to a file elsewhere on the same drive looks like an ordinary file. A
  network share mounted as an ordinary folder (for example under `/Volumes`) is not
  recognised as one.
- rea may keep a file's results in memory after its folder lost approval, until that chat's
  next call, another chat's open or rea's stop after 15 idle minutes. Every read is checked
  against the folders first.

## Secrets and privacy

- API keys, the Hugging Face token, MCP secrets and the sharing key are kept in the
  operating system's keychain, never in plain files. The database stores only references.
- No telemetry, no crash upload, no account. With no model downloaded and nothing
  clicked, the app makes no network request. Every network use is listed in
  ARCHITECTURE.md 13.6. 0.0.6 adds two of its own, both only when you ask: Compare looks
  up the models you add on Hugging Face, and Settings → Reverse engineering downloads what
  you click. A task you let the model hand to Claude Code or Codex also leaves this
  computer: that program sends the task and the files it reads to its company (Anthropic or
  OpenAI) with your account, unless it is set up to use the model in harnessUI.
- The usage line under the model name (0.0.6) is read from this computer's own counters,
  only while the window is visible. Nothing is sent anywhere, and the model is never told
  the figures.
- Logs stay on your computer and contain neither prompt text nor secrets.
- llama-server listens only on 127.0.0.1, on a random port, with a random key per launch.
  The sharing proxy, when you switch it on, listens only on 127.0.0.1 too.
- On Windows the webview runtime's crash reporter is switched off so a crashed page,
  which can contain chat text, is not uploaded. See the known issues in CHANGELOG.md.

## The webview

The interface runs in a locked-down webview: two explicit capability files, app commands
callable only from the main window, a strict Content Security Policy with Trusted Types,
no shell, filesystem or HTTP plugin, navigation away from the app blocked, and model
output rendered as text and React elements, never as HTML. Images in model output load
only from `data:` or `blob:` URLs, so a model cannot leak data through an image link.

## Audit log

Every tool call is recorded with its time, chat, server, tool, full arguments, the preview
you saw, the risk level, the decision and who made it, any note, the result and the
duration. It can be filtered in Settings → Audit log and exported as JSON Lines or CSV.
rea calls and hand-offs to Claude Code or Codex are recorded like any other tool call;
requests from other apps through sharing are recorded under the server `other-apps`,
without their text.

## Model files

Model files are parsed in full by llama-server. A checksum proves where a file came from,
not that it is safe. The app pins one llama.cpp build; a llama.cpp security advisory
triggers a new pin and an app release.

Since 0.0.6:

- llama-server never starts another program: its video helper (ffmpeg) is pointed at a
  folder that does not exist, so a crafted picture or video cannot make it run one. WebP
  pictures, which it then cannot read, are turned into JPEG or PNG by a separate helper
  process with a memory and time limit, so a crafted picture cannot take the app down. This
  covers pictures you attach, pictures from tools, and pictures other apps send through
  sharing (at most 32 in one request).
- No llama.cpp program the app starts can launch a debugger when it crashes. On Linux a
  crash used to start one found on `PATH`, a program the app did not choose.
- *Where the model goes* is measured with llama.cpp's own `llama-fit-params`, only for the
  pinned build, which reads the model's layout without loading it.
- Model settings and Fine control values are typed choices from fixed lists and ranges, and
  they are checked again when read back from settings, because another program could have
  written them. Only numbers from those choices reach llama-server's command line.
- What a model is good at, in Compare, is quoted from the publisher's own model card. What
  an uploader writes on Hugging Face is shown as plain text with hidden characters made
  visible.
