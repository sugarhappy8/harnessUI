# Security

harnissUI lets a language model running on your computer use tools: read and write
files, run commands, browse the web and, if you turn it on, control the mouse and
keyboard. This document describes what protects you, what does not, and how to report a
problem. The design behind it is in [ARCHITECTURE.md](ARCHITECTURE.md) section 13.

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
  settings. Those change only through commands the user interface sends.
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
`.git`, shell start-up files, git and ssh configuration, autostart locations; for the
terminal, any delete, elevation, disk and system commands, `git push`, `git reset
--hard`, `git clean`, inline interpreter code, wrapper commands and anything the parser
cannot fully understand; form submissions in the browser; and external tools their server
marks destructive or that you marked always-ask.

**Cards are built to be read safely.** Every string on a card was chosen by a model or a
server. Control, right-to-left-override and zero-width characters are shown as visible
escapes. *Allow* never has default focus or an Enter binding, and clicks are ignored for a
short moment after a card appears.

## Files

Tools only reach folders you approved, for all chats or for one. The app's own data
folder, its install folder and the agent browser's profile are never reachable, whatever
you approve. Every path is checked lexically first (so a network path such as
`\\server\share` is refused before Windows tries to connect to it), then resolved through
symlinks and junctions and checked again, case-insensitively where the volume is.

## The terminal is not a sandbox

There is no operating-system sandbox in version 1. An approved terminal command or an
external MCP server runs with your permissions. It can read and change anything you can,
including this app's database (its approved-folder list and audit log). "Changed only by
UI commands" and "append-only" are properties of the app's own code, not of the files on
disk. The terminal's command parser is a guard against accidents and obvious abuse, not
a security boundary, and the app says so when you turn the terminal on.

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

## Secrets and privacy

- API keys, the Hugging Face token and MCP secrets are kept in the operating system's
  keychain, never in plain files. The database stores only references.
- No telemetry, no crash upload, no account. With no model downloaded and nothing
  clicked, the app makes no network request. Every network use is listed in
  ARCHITECTURE.md 13.6.
- Logs stay on your computer and contain neither prompt text nor secrets.
- llama-server listens only on 127.0.0.1, on a random port, with a random key per launch.
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

## Model files

Model files are parsed in full by llama-server. A checksum proves where a file came from,
not that it is safe. The app pins one llama.cpp build; a llama.cpp security advisory
triggers a new pin and an app release.
