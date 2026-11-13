# 0.0.6 plan

These are the things the owner asked for after 0.0.4. Work on the first four started as
"0.0.5" and was paused on 2026-11-12, when the owner chose to ship **Full access** as 0.0.5
first. Items 5 to 8 were added while 0.0.5 was being finished. The owner's words are kept
as they wrote them.

1. allow per model on loading context window setting and it auto compresses.
2. More and more model loading setting and model reloading but simplify it to normal human language.
3. Allow for local loaded models to be able to talk to Claude or codex apps upon user request.
   (The owner chose both directions: other apps may use the model loaded here, and the local
   model may hand a task to Claude Code or Codex.)
4. Add model comparisons for models so users can compare different models and what their good at from the download huggingface thing we have
5. "i wanna install https://github.com/morluto/rea as a main feature in it but with a free decompiler. maybe lets keep that for version 0.0.6."
   Designed in `item5-spec.md` (research in `item5-rea-research.md`), with the owner's
   answers at its top: rea with Kuna and Ghidra, installed from its own Settings tab; Ghidra
   on Windows, Kuna only on a Mac; an Install button for Apple's command line tools; off in
   new chats until turned on, then new chats follow the last choice. Built (WP9a to WP9e,
   WP9g, WP9h; WP9f dropped by the owner's answer).
6. "on 0.0.6 i want more control options as well for the models seeing how much of my gpu and cpu is currently being used and how offloading would look like and options for select offloading btw. similar to how LM studios does it."
   Designed in `item6-spec.md` (earlier sketch `item6-usage-offloading.md`), with the owner's
   answers 1A, 2A, 3A at its top, and built (WP10a to WP10e). What it adds to item 2:
   - **Live usage:** how busy the graphics card and the processor are, and how much of their
     memory is in use, while a model loads and runs.
   - **What offloading would look like, before loading:** how much of the model would sit on
     the graphics card and how much in main memory, with the memory each would need, for the
     settings chosen.
   - **Choosing the offloading:** the controls LM Studio offers, for example how many layers
     go to the graphics card, keeping the experts of a mixture-of-experts model on the
     processor, whether the conversation memory (KV cache) stays on the graphics card, and
     processor threads.
   This went against item 2's decision D4 on purpose; `spec.md` Part 3b amends D4: the plain
   choices stay the default, and the finer controls sit in a closed **Fine control** section
   of each model's settings, with live usage always visible under the model name. Usage
   figures come from this computer only (no telemetry) and say "Not available" where a
   system gives none.

7. "i also want to make it clear what folder is being used if u dont have full access on and stuff. it currently kinda hidden."
   Before 0.0.6 the chat's folders were only in the Tools panel ("Folders for this chat") and
   in Settings → Permissions. Built (WP7, `spec.md` Part 3b): a folder chip in the chat
   header with a menu to change or add folders, "Whole computer" in Full access.
8. "also when i download a new model it doesnt auto show up here fix that in 0.0.6" (the owner's
   screenshot: the chat header's model menu, "On this computer").
   Cause found while writing this down: that list shows only the model files used recently
   (the setting `RECENT_LOCAL_MODELS`, read by `list_model_options` in
   `src-tauri/src/models_cmd.rs`), not the installed ones, and a finished download is not added
   to it, so a new model appears only after it was loaded once from the model manager. The
   menu re-reads its list each time it opens (`ModelPicker.tsx`), so listing the installed
   models (and refreshing when a download finishes, `download-progress`) fixes it. The same
   screenshot lists one Q8_0 model file twice: the list showed only the file name, so two
   copies in different folders looked the same. Built (WP8): every installed model is
   listed, one entry per file, and two files with the same name each show the end of their
   folder.

## What exists
- **Design:** `spec.md` (revision 3: items 1 to 4 in Part 3, items 5 to 8 in Part 3b, the
  owner checklist in Part 4), `item5-spec.md` and `item6-spec.md` (each with the owner's
  answers at the top), the map of the code they started from (`map.md`, `map-details.md`,
  line numbers as of 0.0.4) and the critiques that shaped them (`critique.md`, and each item
  spec's critique log).
- **Groundwork (WP0):** shared types, command stubs and component splits; reports in
  `report-WP0-*`. The contract every package built on is the section "CONTRACT (updated)"
  of `report-WP0-fix.md`.
- **Every work package is built, reviewed and fixed:** WP1a to WP8 (items 1 to 4, 7 and 8),
  WP9a to WP9e, WP9g and WP9h (item 5), WP10a to WP10e (item 6), each with an independent
  security review and a correctness review, fixes and a check; then a whole-release review
  and a final fix round. `STATUS.md` has the timeline, `integration-todo.md` every leftover
  with its outcome. `package-briefs.json` holds the instructions the first eleven
  packages' developers got.
- **What only the owner's computers can show:** `owner-checklist.md` (Windows PC with the
  Radeon RX 9070 XT, the 16 GB Mac, either).
- **History of the paused run:** `history/STATUS-at-pause.md` (which package was where when
  the owner stopped it), `history/packages-workflow.js.txt` (the build → security and correctness
  reviews → fix pipeline that ran them) and `history/resume-args.json` (its arguments).

## Git history: read this before merging anything
The paused work was first committed on the 0.0.5 branch (7827001, also the branch
`claude/0.0.5-paused`) and then undone there by a0177eb, which set every file back to 0.0.4.
So 7827001 is already in the history of the 0.0.5 branch and of `main` once 0.0.5 is merged:
- merging `claude/0.0.5-paused` into anything that has 0.0.5 does **nothing**;
- merging `main` into `claude/0.0.5-paused` fast-forwards it and the work **disappears**.

This branch got the work back by reverting a0177eb on top of 0.0.5 (commit 71310d9). Do not
use `claude/0.0.5-paused` for anything any more; it is kept only as a record.

## How to pick it up
**0.0.6 is complete, pending the owner's review and CI on all three systems.** All eight
items are built, the version is 0.0.6 everywhere, and the docs (CHANGELOG, ARCHITECTURE,
SECURITY, README) describe it. The last step on Linux was a final pass: every check and
every end-to-end suite with the real model, with each failure fixed (its outcome is in
`STATUS.md`). Draft pull request #5 (`claude/0.0.6` into `main`) carries it. Nothing is
left to build unless the final pass, the owner's review or CI finds something.

1. **Read `STATUS.md` (last entries) and the latest CI run on pull request #5.** The first
   CI run (43380365172) stopped at a Linux clippy finding, since fixed. In run 43409122542
   (2026-11-13, 10:45 UTC) Linux had passed and the Windows and macOS jobs, which also run
   the tests there for the first time, were still running. A failure on Windows or macOS is
   a real bug to fix test-first; do not skip or weaken a test to get green.
2. **The owner's review.** Give the owner `owner-checklist.md` and these with it: the
   "rea test files" folder (artifact `rea-test-files` of the macOS job of the *Reverse
   engineering* workflow, `reverse.yml`), and the table of publisher quotes behind the
   catalog's *What is it good at?* answers (WP4c's report; the owner approves or swaps each
   quote before release, D6). The workflow runs only from the default branch (manual or
   nightly), so it can first run after the merge, or by hand on `main`.
3. **What stays open on purpose** is listed in CHANGELOG "Known limits" and SECURITY.md;
   `integration-todo.md` ends with the outcome of every leftover, so a "Known limit" there is
   a decision, not forgotten work.
4. **Before tagging v0.0.6:** the owner's go, CI green on all three systems, then merge and
   tag (the release workflow re-checks Ghidra's checksum before it builds anything).
5. Toolchain: CI uses stable Rust; the final pass built, linted and tested with
   `cargo +1.100` (spec Part 1 still says 1.99, from when the packages were built).
   Keep this branch up to date by merging `main` (0.0.5 was merged as 4189a3a).
