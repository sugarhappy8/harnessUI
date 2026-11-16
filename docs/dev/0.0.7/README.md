# 0.0.7 plan

0.0.7 is a bug-fix release. The owner's words, 2026-11-16:

1. "All the things i mentioned" (the problems and requests from the days after 0.0.6 shipped,
   listed under item 1 below).
2. "Allowing models to know what tools they have at all times so they don't think they don't
   have tools even though they do, also a guid for them on how to formate commands and shell
   and everything like that because they make mistakes a lot."
3. "Allowing the user to set how many times the model can repeat the same command."
4. "Bug fixing"

Work happens on the branch `claude/0.0.6` (the name the session's tooling requires), restarted
from `main` after 0.0.6 was merged. A draft pull request is opened at the start, so CI runs
on Windows, macOS and Linux for every pushed commit, not only at the end (0.0.6 found its
Windows and macOS failures only in its last day).

## Item 1: what the owner reported after 0.0.6

### 1a. Memory estimates far too high (Browse, Compare, Automatic, offloading picture)
Seen with `gemma-4-12B-coder…heretic` (Q8_0, 12.7 GB file): "Too large, needs about
53.7 GB". The conversation memory is counted for every layer at full length. Gemma 4 keeps
the whole conversation in only 8 of its 48 layers; the other 40 look back 1,024 tokens
(`attention.sliding_window`, `attention.sliding_window_pattern`) and use smaller rows
(`attention.key_length_swa`, `value_length_swa`). Real need at 65,536 tokens: about 1.6 GB
of conversation memory, not 41 GB. The same rule applies to other models with short-memory
layers (Gemma 2 and 3, gpt-oss, Cohere 2 and others), and to layers that share another
layer's memory (`attention.shared_kv_layers`).
**Fix:** count each layer as llama.cpp at the pinned build allocates it (short-memory layers
at their window, their own row sizes, shared layers at nothing), everywhere the figure is
used. Tests with Gemma 4's real header values.

### 1b. A broken image file stops the model from loading
Seen with `Ministral-3-14B-Reasoning-2512-Uncensored` (mradermacher): the model loads, then
the image file (`…mmproj-f16.gguf`) fails (`unable to find tensor v.token_embd.img_break`)
and the whole load fails, four times (once per backend), with "try the CPU backend".
**Fix:** recognise the image file's failure; load the model again without it and say so in
plain words ("Loaded without image input: the image file next to this model couldn't be
read."); remember it for that file so the next load does not fail first; no stepping
through backends and no CPU advice for it.

### 1c. An image file added later is never used
Seen with `Qwen3.8-27B-Uncensored-HauhauCS-Aggressive`: computer use said "This model
cannot see images" because the model was loaded without its image file. The app links an
image file only when a model is downloaded or imported.
**Fix:** look for an image file next to the model every time it loads when none is recorded
(or the recorded one is gone); in Models → Installed, offer "Download the image file" for a
model whose Hugging Face repository has one; the "cannot see images" text says how to fix it.

### 1d. Computer use refused while harnessUI is in front
Screenshots leave harnessUI's own window out on purpose, so the model sees the apps behind
it, but every click on harnessUI's window and all typing while it has the keyboard are
refused ("covered by this app's own window"). The only way out was a small button inside a
folded tool card.
**Fix:** minimise harnessUI when approved computer input is about to run, bring it back when
an approval card needs the user, when the turn ends and on Stop. The red border and the Stop
window stay. Check how Gemma 4, Ministral 3 and Qwen 3.x write screen positions against their
publishers' documentation and fix the automatic choice where it is documented.

### 1e. API keys for cloud companies
Settings → Endpoints already takes any OpenAI-compatible server with a key. 0.0.7 makes it
easy and safe:
- ready-made choices for Anthropic (Claude), OpenAI, abliteration.ai and OpenRouter: pick the
  company, paste the key;
- requests a cloud server accepts: no llama.cpp-only fields (for example `top_k`), the
  parameter names newer OpenAI models require;
- Claude's own format (Messages API), so its thinking and long-chat caching work;
- a clear mark in the chat header when the chat goes to a cloud server.

## Item 2: models always know their tools, and how to write calls and commands
- The system message lists every tool that is on in the chat, one line each, and says which
  are shown directly and that the others run through `call_tool` (adaptive tool selection
  shows only some tools per turn, and small models then think they have none).
- A short guide for writing tool calls (JSON rules, paths on this OS) and, when the command
  tool is on, for the shell the commands really run in (Windows PowerShell 5.1 on Windows,
  `/bin/sh` on macOS): the mistakes models make most, such as `&&` and `rm -rf` in
  PowerShell 5.1.
- Command errors caused by a common mistake of that kind add a one-line hint.

## Item 3: the user sets how often the same call may repeat
Today a call repeated with the same arguments gets a warning at 3 in a row and ends the turn
at 5 (`LOOP_WARNING_AT`, `LOOP_STOP_AT`). 0.0.7 adds a setting: "Stop when the model makes
the same call this many times in a row", default 5, from 2 to 100; the warning comes two
calls before the stop.

## Item 4: bug fixing
A bug hunt over the flows the owner uses most (loading models, chat with tools, downloads,
settings, computer use); confirmed bugs are fixed test-first. The owner can add bugs at any
time.

## Decisions taken without asking (the owner can change any)
- Repeat limit default 5, range 2 to 100.
- harnessUI minimises itself during computer use (rather than moving or shrinking).
- Claude's own format is built, not only the OpenAI-compatible one.
- Cloud choices: Anthropic, OpenAI, abliteration.ai, OpenRouter.

## Status
See `STATUS.md`.
