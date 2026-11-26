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
  *Allow* on its own approval card. Since 0.0.7 the app minimises its window before
  approved computer input that the window is in the way of, and brings it back for every
  approval card of that chat, in front of your other windows but without taking the
  keyboard. Each time it comes back, approval cards and the Allow all and Full access
  confirmations ignore clicks for a moment, so a click meant for another app cannot
  approve an action or switch the mode. Without a card answered just now (a grant, Allow
  all) the app steps aside only when you have not touched the keyboard or mouse for 1.5
  seconds (a key pressed within a few hundredths of a second of the model's own input can
  be taken for the model's). It hands the
  keyboard to the window the input was planned for only while it has the keyboard itself
  and Stop was not pressed, checked in the step that moves it. The check before every
  input event is unchanged, so input that would still reach the app's window (it did not
  go, or you brought it back) is refused.

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
and in *Settings → Permissions*, and no tool or model output can reach it.

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
  its file tools.

Since 0.2.0, switching a chat to Allow all or Full access, or saving either as the start
mode, takes effect without a confirmation, and a saved Full-access start mode is applied
at launch without being asked again. Earlier versions asked again at each launch, in case
another program had changed the setting; the owner chose to drop that check.

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

## Website capture (0.0.7)

`browser_capture_site` saves a copy of a web page's front end so a model can study its
design and rebuild it with your own content. The tool's description tells the model it is
not for reproducing other people's logos, brand names or sign-in pages; nothing in the app
can check what a model then does with the copy.

- **What it saves, and where.** Into a new folder inside a folder you approved (never
  overwriting anything): the page as shown, every style sheet, the pictures, fonts, icons,
  Lottie animations and small videos the page loaded, its scripts as `.txt` files,
  `animations.json`, screenshots and `report.md`. At most 300 MB and 2,000 files, and no file
  over 25 MB; what was left out is listed with the reason. Saving is a write: in Ask first a
  card says what, where and how much, and *Allow for this chat* covers that site into that
  folder only. A folder outside the approved ones, the app's own folders and skill folders
  are refused in every mode; a `.git`, ssh or autostart folder always asks. The folder is
  checked again just before writing, every file is created new, and file names come from
  the app (lower-case letters, digits, `-`, `_`, `.`; no Windows device names; at most 80
  bytes), never from the website.
- **Network rules.** The page opens in the agent's own browser with its own profile, so
  your sign-ins and cookies are never used. While the capture runs, every web request of
  that browser (pictures, fonts, scripts and style sheets too, not only pages) is paused and
  checked like a page load: http and https only, nothing on loopback, private, link-local
  or other blocked addresses, judged on the address the name resolves to. That includes
  the requests of the page's workers: a shared worker's are paused on its own debugging
  session before it may run, a dedicated worker's where the browser pauses them, with its
  page's. **Service workers do not run during a capture**: the browser accepts the request
  checks on one but does not apply them to a service worker that a site installed earlier
  and that starts again (tried in Chrome 153), so every service worker that starts during
  the capture is kept waiting and stopped when it ends; the report says that anything the
  page would have had from it is missing. Shared and service workers that already run when
  the capture starts are stopped first, because what they hold came from requests that
  were never checked one by one. If one cannot be stopped, or a shared worker's requests
  cannot be paused, the capture stops and saves nothing. Only responses to
  requests that passed those checks are saved, and a response that reached the browser from
  a blocked address on a direct connection is refused even if the name looked public when
  it was checked. The capture only reads: it never clicks, types or submits, so no form is
  sent; the site's own scripts run in the agent's browser while the page is open, as on any
  visit.
- **Connections those checks cannot see.** The browser's request checks see web requests,
  not WebSocket, WebTransport or WebRTC connections. During a capture the page being
  captured (its documents, including frames' documents where the browser lets the app
  change them and a redirect without a `Location` that the browser shows as a page, and its
  workers' scripts) gets an extra policy that refuses WebSocket
  connections, so a page cannot read a device on your network through one. The tab, its
  frames and its workers, and any shared or service worker, are also watched: if a
  WebSocket, WebTransport or direct-socket connection starts anyway, the capture stops and
  saves nothing, because what came over it could be from your own network. **WebRTC cannot
  be blocked or watched this way**: a page can still use it while it is open (in the
  capture as in normal browsing), for example to probe addresses on your network; nothing
  received over it is told apart in the copy. A WebTransport connection to a public address
  is not refused by that policy (it is stopped by the watch above). Current Chrome and Edge
  also refuse, by themselves, a public page's connections to private and local addresses
  (their Local Network Access check, with permission prompts denied in the agent's browser);
  older browsers do not.
- **The copy runs nothing and loads nothing from elsewhere.** `index.html` is built in two
  steps. The page script copies the page into an inert document, without scripts, frames'
  contents, embedded objects, event handlers and `javascript:`/`file:` addresses. Then the
  app parses that HTML again exactly as a browser opening the file would (so elements a
  page disguised, a replaced root or an attribute named like a tag become the real thing)
  and cleans the tree: no `script`, `noscript`, `iframe` (kept as an empty box), `embed`,
  `object`, `frame`, `base`, SVG `foreignObject` or animations that set an address, no
  `<meta>` but the copy's own policy, character set and description, no `<link>` but style
  sheets and icons saved in the copy, no event handler or made-up attribute, no attribute
  that sends something. Every address of a picture, style sheet, font or video must name a
  file saved in the copy's own `assets/` or `styles/` folder, by a relative address that stays
  in the folder (a picture may also be a `data:image/` address); anything else, including
  `file:` and `//host` addresses (on Windows a network share, which would send your Windows
  sign-in hash) and every web address, is removed. Links keep only `http(s)`, `mailto:` and
  `tel:` addresses. CSS in the page and in the saved style sheets is read with a real CSS
  tokenizer (escaped names included) and held to the same rule; inside `image-set()`,
  `image()` and `cross-fade()` only literal addresses and plain values are allowed, so
  anything else there (`var()`, `env()`, `attr()` or a function the list does not know)
  makes the whole value blank. `index.html` and every saved style sheet start with a UTF-8
  byte order mark and carry no leading `@charset` and no control characters other than tab
  and line ends, so a browser reads them as the UTF-8 that was checked (a sheet declaring
  ISO-2022-JP, for example, could otherwise hide an address in what the check read as a
  comment); CSS in saved SVG pictures and in the page's own style attributes loses the same
  characters. A file asked for as a picture or icon is kept only when its first bytes are a
  picture format the copy keeps (PNG, JPEG, GIF, WebP, AVIF, SVG, ICO or BMP). The result is written with a
  real `<html><head>` whose first child is a policy that allows no script, loads nothing
  from the internet and sends nothing, and a last check parses the output once more: unless
  the root is `<html>`, the policy is the first thing in the head and cleaning it again
  changes nothing, the copy is not saved (the capture's folder is removed again). SVG
  pictures are parsed as XML and written again with only drawing elements from a fixed list,
  without handlers and with only fragment or `data:` picture addresses; a picture that is
  not well-formed or declares a document type (whose entities could hold markup) is left
  out. Scripts are saved as `.js.txt`, because a `.js` file runs when double-clicked on
  Windows.
- **What the model reads.** The report quotes page text (titles, headings, labels) as data:
  one line, quoted, invisible and direction-changing characters shown as escapes, and the
  characters that could start a link or markup escaped. It reaches the model as untrusted
  tool output like any page.
- **Limits.** A capture can save whatever a public page shows, including other people's
  pictures and text; what you do with the copy is up to you. Only what is fetched during
  the capture is checked: what a site stored in the agent's browser on an earlier visit
  (its storage and caches, from requests normal browsing does not check one by one) can
  still reach the page and the copy, as it reaches the screen in normal browsing. A stopped
  capture can leave a partly saved folder.

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
  you press *Show key* (and, while sharing is on, handed only to a Claude Code or Codex run
  from a chat that is set up for this port; see the next section), and *Make a new key*
  replaces it (the old one stops working at once). The key, the address the request was sent to and the absence of a web page's
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
  model in harnessUI, the card says the task goes to your local model instead. That needs
  sharing switched on and working now, and the program set up for `127.0.0.1` and the port
  sharing uses, as the setup steps write it. A program set up for harnessUI while sharing is
  off or not working, for the same port under another name such as `localhost` (which can
  reach another program listening on `[::1]`), or for any other port on this computer (such
  as the port sharing used before you changed it) is not asked. So harnessUI never starts a
  program it has found set up to send its task to a port on this computer that harnessUI
  does not serve.
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
- Secret-looking environment variables (keys, tokens, passwords) are not passed to them,
  with one exception: a program set up to use the model in harnessUI at `127.0.0.1` on the
  port sharing uses now gets the sharing key, from where harnessUI keeps it, in the one
  variable its own setup reads it from, and nothing else.
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
0.0.6 was built, so a CI run of this project (run 43380365172) read it and saved what GitHub
answered; it matched, and it is recorded with that run's address, so a release build offers
Ghidra. A check is recorded only from a record that names a CI run of this project. Every
release build reads Ghidra's checksum on GitHub again before anything is built, and stops if
it no longer matches.

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
its home and temporary folders inside harnessUI's data folder. On Linux, Ghidra cannot work
from there (that folder is under `~/.local/share`), so its temporary files go to a private
folder of harnessUI's in the system's own folder for your session (`XDG_RUNTIME_DIR`), which
only you can open, the file tools never reach, and harnessUI removes whenever rea stops or
the app starts. It never runs its own setup,
so it writes nothing into the settings of Claude Code, Codex or other apps. Ghidra's
analyser that looks up debugging-symbol files is turned off. On a Mac it never starts before
Apple's command line tools are installed, so Apple's install window cannot appear in the
middle of a chat. A CI workflow is set up to run rea with the real decompilers (Ghidra on
Windows, Kuna on macOS, both on Linux) and to fail if they try to reach another computer.
On Linux it watches every program the contract tests start, name lookups included; on
Windows it blocks and watches Node.js, Java and Ghidra's decompiler; on macOS it blocks and
watches the contract tests' TCP and UDP traffic. One more check, of the app's own call path
with rea and Kuna, runs on Linux and macOS outside these watchers. It has not run on GitHub yet: its Linux checks were run on the
build machine, and its Windows and macOS jobs have not run at all.

**Limits:**

- rea, Node.js, Kuna, Ghidra and Java run with your rights, like any tool server you add.
  The app decides which rea tools exist and which files they are given, not what those
  programs could do.
- A decompiler reads a hostile file in full. A file made to attack Kuna or Ghidra could take
  it over, and it would then have your rights and network access. Open programs from people
  you don't trust only on a computer you can wipe.
- If harnessUI crashes, Kuna or Java can keep running for a short while on macOS and Linux.
- On Linux, when `XDG_RUNTIME_DIR` is missing, is not private to you, or has less than
  256 MB free (it gets a tenth of the computer's memory by default, so this happens on
  computers with less than about 2.5 GB of memory), Ghidra's working files (its project and a
  copy of the opened program) go to a private folder in `/tmp` that only you can open; rea
  removes it when it stops. Where the runtime folder is used, the system keeps it in memory,
  so a very large program may not fit there. After Java is stopped by force, a 32 KB file of Java's
  stays in `/tmp/hsperfdata_<your user name>` until Java next starts.
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
- No telemetry, no crash upload, no account. (On Windows, a crash of a program the app
  starts is handled by Windows Error Reporting as your Windows settings say; the app itself
  uploads nothing.) With no model downloaded and nothing clicked, the app makes one kind of
  network request: from 0.0.8, the check for updates, shortly after it starts and every 6 hours,
  to github.com (*Settings → General → Updates* turns it off). It carries nothing about you or
  your chats. A newer version it finds is downloaded and its signature checked against the key
  built into the app, and it is installed only when you click *Restart and update*. Every network use is listed in
  ARCHITECTURE.md 13.6. 0.0.6 adds two of its own, both only when you ask: Compare looks
  up the models you add on Hugging Face, and Settings → Reverse engineering downloads what
  you click. 0.0.7 adds one more, also only when you ask: Models → Installed offers a
  model's image file from the file list saved when you downloaded the model, without asking
  Hugging Face; it asks for that model's file list when you click *Look for an image file*
  (models downloaded before 0.0.7) or *Download the image file*, and by itself only once:
  the next time Installed shows that model after its image file couldn't be read. *Download the image file* downloads it
  like any model file (checked against its published size and checksum, saved only in the
  model's own folder). An image file next to a model is loaded with it, so writing a file
  named like one (a `.gguf` file with "mmproj" in its name) always asks, also in Allow all;
  links next to a model are not followed. A task you let the model hand to
  Claude Code or Codex also leaves this
  computer: that program sends the task and the files it reads to its company (Anthropic or
  OpenAI) with your account, unless it is set up to use the model in harnessUI.
- **Cloud AI companies (0.0.7).** A chat whose model is on an endpoint on the internet, such
  as Anthropic, OpenAI, OpenRouter or abliteration.ai, sends that company everything the
  model reads: your messages, the system prompt and pinned instructions, the files and
  pictures you attach, the tools' descriptions and what the tools return (file contents,
  command output, web pages, screenshots), and with Claude its own earlier thinking (kept in
  memory for each chat and sent back only in that chat; a helper's is its own and forgotten
  when it answers). The
  company's own terms say what it keeps and for how long; OpenRouter passes the chat on to
  the company that runs the model you picked. The chat header shows a *Cloud* mark for such
  a chat, with the host in its tooltip. Only `localhost` and local network IP addresses
  (such as 192.168.1.20) count as not on the internet: a server named any other way, a
  `.local` or one-word name included, gets the mark, because the name leads wherever the
  name server says. Server answers are untrusted data like any model output. On *Automatic*
  a long chat sends at most the company's size with each message (200,000 tokens for
  Claude); a larger size, chosen per endpoint, sends more of the chat each time.
- An endpoint's API key goes only to that endpoint's own address, in the header its API
  uses (`x-api-key` for Anthropic, `Authorization` for the others). A redirect to another
  address is not followed, and a saved key does not follow an edited endpoint to a new
  scheme, host or port: the new address needs its own key, and the old key is removed
  before the endpoint is saved. A keychain that cannot be read counts as holding a key, so
  these rules never pass by default, and a save whose key removal the keychain refuses
  saves nothing. The address is read the same way
  for this check, the *Cloud* mark and the requests, and an address that cannot be read gets
  no key. The key is never written to the database or the logs, and it is removed from every
  error text before the error is shown or stored (every copy of a key of 4 characters or
  more, so a local server's short key too), also from errors inside a reply and from a
  server message cut short across the key. It is never sent over plain http:// except to
  this computer or a local network IP address: the app refuses to save another such endpoint
  with a key and does not use a key saved earlier for one. Over http:// to this computer or
  a local network address a key is still sent (servers there rarely have https), directly
  and never through a system proxy (also for `0.0.0.0`, `[::]` and `localhost.`, which is
  reached as `localhost`, so no name server decides where it goes); anyone who can watch
  that network can read it.
- **Studio (0.1.0).** A local Studio run sends nothing: the prompt, the pictures you add
  and what it makes stay on this computer, and no company sees them. Get local models talks
  to Hugging Face (`huggingface.co`, for search, model listings and model files, with the
  token you already saved for a gated model, sent only to huggingface.co) and the Windows
  Studio engine comes from GitHub, each only when you click: Search, opening a result, or
  Download. Opening Studio, Get local models or Settings → Studio sends nothing. A cloud
  run sends the prompt and the pictures you add to the company you picked, `openrouter.ai`
  or `api.openai.com`, with that endpoint's key, which you saved in Settings → Endpoints;
  OpenRouter passes it on to the company that runs the model, whose own terms say what it
  keeps. A cloud result comes back from that company, or from the file host it points to,
  which harnessUI reads through the same guarded fetcher as web pages: the key never goes
  to a file host, or to any address but the endpoint's own. Results are kept in the gallery
  folder on this computer (by default `harnessUI/studio` in your home folder), each with a
  record of its settings beside it that never holds a key, and the prompt is never written
  into a local result. Studio has no tool: a chat model cannot start a run or read the
  gallery, and a picture reaches a chat only when you click *Send to chat*.
- **Studio's spending rules (cloud runs).** Spending guards are off by default: a daily
  limit (suggested $10), asking before every video, before runs of $1 or more, and when the
  price is unknown are switches in Settings → Studio. Prices are still shown before every
  cloud run. The backend keeps whichever switches are on whatever the window sends: a daily
  limit, counted per calendar day on this computer, means a run that would take today's
  spending past it is not sent, and a run with an unknown price needs $5 of the limit left.
  Deleting a result never lowers today's spending. A run on this computer is free, never asks
  and never counts.
- The usage line under the model name (0.0.6) is read from this computer's own counters,
  only while the window is visible. Nothing is sent anywhere, and the model is never told
  the figures.
- Logs stay on your computer and contain neither prompt text nor secrets.
- llama-server listens only on 127.0.0.1, on a random port, with a random key per launch.
  The sharing proxy, when you switch it on, listens only on 127.0.0.1 too.
- On Windows the webview runtime's crash reporter is switched off so a crashed page,
  which can contain chat text, is not uploaded. See the known issues in CHANGELOG.md.

## Updates

- From 0.0.8 a release build asks github.com for a newer version shortly after it starts and
  every 6 hours (*Settings → General → Updates* turns that off). As with any visit to a
  website, GitHub sees the computer's IP address with each check and each download; nothing
  about you or your chats is sent.
- Only a file signed with the owner's updater key is installed. The app checks the downloaded
  file's signature against the public key built into it before it offers the update, and
  nothing is installed until you click *Restart and update*.
- The signature also covers the version number: the release build writes the version into
  each update file's signature, and the app refuses a file whose signed version is not the
  one the release's `latest.json` announces, or that carries no version at all. The app
  installs only a version newer than the one running, so someone who could change a release
  on GitHub without the key cannot pass an older signed build of harnessUI off as a newer
  one.
- Someone who had both the private key (with its password) and the right to publish releases
  could send any program to every installed copy, and only your click on *Restart and update*
  stands in the way. The owner keeps the key and its password in a password manager and as
  repository secrets, which reach only the release jobs that need them.

## The webview

The interface runs in a locked-down webview: two explicit capability files, app commands
callable only from the main window, a strict Content Security Policy with Trusted Types,
no shell, filesystem or HTTP plugin, navigation away from the app blocked, and model
output rendered as text and React elements, never as HTML. Images in model output load
only from `data:` or `blob:` URLs, so a model cannot leak data through an image link.
Since 0.1.0 the policy also has `media-src blob:`, so Studio can play a video from a
`blob:` address the webview makes from the bytes the app read from the gallery; no remote
video can play.

## Audit log

Every tool call is recorded with its time, chat, server, tool, full arguments, the preview
you saw, the risk level, the decision and who made it, any note, the result and the
duration. It can be filtered in Settings → Audit log and exported as JSON Lines or CSV.
rea calls and hand-offs to Claude Code or Codex are recorded like any other tool call;
requests from other apps through sharing are recorded under the server `other-apps`,
without their text. Each Studio run (0.1.0) is one row under the server `studio`, tool
`picture` or `video`, decided by you: it keeps the prompt's length only, never the prompt
(and a negative prompt's length only, never its text), and a local run's row names the
model's revision and the Studio engine's build. A finished row is never changed.

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

## Running programs: the Studio engine (0.1.0)

Studio's local pictures and videos are made by one more program, the Studio engine
(stable-diffusion.cpp's `sd-cli`), at one pinned build: the Windows pack pinned by size and
SHA-256, the Mac build by commit with its three submodules, built in CI and signed with the
app. It is not part of the Windows installer: it downloads with your first local model,
from GitHub only, after its licence is shown, into its own folder, never the chat model
runner's.

- One engine process runs per job, through the same spawn helper as llama-server: its own
  process tree, which Stop kills whole, and almost none of your environment (no proxy
  settings, no keys). There is no engine server listening on any port.
- Its arguments come from typed fields only: a fixed flag for each model part, numbers, and
  words from fixed lists. Nothing you type, and nothing in a model's listing, becomes a
  flag. The prompt is never on its command line, where other programs could read it: it is
  written to a file in the run's folder, which is deleted when the run ends. A local prompt
  may be at most 16,384 characters, so the child-safety check reads all of it; a negative
  prompt may be at most 262,144 characters; and no prompt can be longer than 1 MiB.
- Model files must be `.gguf` or `.safetensors`. `.ckpt`, `.pt` and `.pth` files are
  refused everywhere, because they can run code when they are loaded. Every file is checked
  against its published size and SHA-256, and a GGUF model file is checked by its own
  tensor table, after download and before every run, so a file whose name lies is never
  run. As with llama-server, a checksum proves where a file came from, not that the engine
  parses it safely.

## Content: what Studio makes (0.1.0)

- Before the first local download Studio shows a note once, and records that you read it:
  "Local models run only on your computer. No company checks your prompts or pictures.
  Don't create sexual images of children, or sexual or intimate images of real people
  without their consent, and don't impersonate people. These are crimes in many countries.
  Each model's licence and use policy also applies." It can be read again from Get local
  models and Settings → Studio. The download dialog shows the licence for each part.
  Hugging Face enforces publisher access requirements for gated repositories.
- A small prompt check that is always on refuses a local prompt that pairs sexual words
  with words for children ("Studio won't make this."). It is a narrow check, not a filter,
  and nothing is retried. Cloud companies apply their own filters; a refusal is explained
  in plain words and never retried.
- Every local picture and video carries a label in its own file saying it was made by AI
  (XMP `DigitalSourceType = trainedAlgorithmicMedia`, harnessUI's version, the model and
  its revision, a unique id and the date), and never the prompt. Cloud results are kept
  exactly as the company sent them.
