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
   Not designed yet. First read that repository (what it does, its license, what it needs to
   run), then pick a free decompiler whose license allows shipping it with the app, and write
   the spec like the others. Ask the owner before choosing.
6. "on 0.0.6 i want more control options as well for the models seeing how much of my gpu and cpu is currently being used and how offloading would look like and options for select offloading btw. similar to how LM studios does it."
   Not designed yet. What it adds to item 2:
   - **Live usage:** how busy the graphics card and the processor are, and how much of their
     memory is in use, while a model loads and runs.
   - **What offloading would look like, before loading:** how much of the model would sit on
     the graphics card and how much in main memory, with the memory each would need, for the
     settings chosen.
   - **Choosing the offloading:** the controls LM Studio offers, for example how many layers
     go to the graphics card, keeping the experts of a mixture-of-experts model on the
     processor, whether the conversation memory (KV cache) stays on the graphics card, and
     processor threads.
   This goes against the current spec on purpose: item 2's decision D4 keeps the settings
   few and plain and lists "threads, batch sizes, slots, rope, tensor split" as never shown.
   The spec must be revised: keep the plain choices as the default, and add a place with the
   finer controls and the usage view. Ask the owner where to draw that line. Usage figures
   must come from this computer only (no telemetry), on Windows (the owner's AMD Radeon
   RX 9070 XT), macOS and Linux, and degrade to "not available" where a system gives none.

7. "i also want to make it clear what folder is being used if u dont have full access on and stuff. it currently kinda hidden."
   Today the chat's folders are only in the Tools panel ("Folders for this chat") and in
   Settings → Permissions; the model is told them, the owner hardly sees them. Show the
   folder the chat works in, and the approved folders, where the owner looks anyway (for
   example the chat header or beside the message box), with a quick way to change or add
   one. In Full access say plainly instead that the whole computer is reachable.
8. "also when i download a new model it doesnt auto show up here fix that in 0.0.6" (the owner's
   screenshot: the chat header's model menu, "On this computer").
   Cause found while writing this down: that list shows only the model files used recently
   (the setting `RECENT_LOCAL_MODELS`, read by `list_model_options` in
   `src-tauri/src/models_cmd.rs`), not the installed ones, and a finished download is not added
   to it, so a new model appears only after it was loaded once from the model manager. The
   menu re-reads its list each time it opens (`ModelPicker.tsx`), so listing the installed
   models (and refreshing when a download finishes, `download-progress`) fixes it. The same
   screenshot lists `Ornith-1.5-9B-uncensored.Q8_0.gguf` twice: the list shows only the file
   name, so two copies in different folders look the same. Find out which it is; show enough
   of the folder to tell them apart, or one entry per file.

## What already exists
- **Design:** `spec.md` (revision 2, with "Amendments after WP0"), the map of the code it is
  based on (`map.md`, `map-details.md`) and the three independent critiques that shaped it
  (`critique.md`). Line numbers refer to the code as of 0.0.4. The spec still says "0.0.5"
  in places, from before the pause.
- **Groundwork (WP0):** shared types, command stubs and component splits, built and
  independently checked; reports in `report-WP0-*`. The contract every package builds on is
  the section "CONTRACT (updated)" of `report-WP0-fix.md`.
- **Half-built packages:** about 32,000 lines across 119 files for the eleven work packages
  (WP1a to WP6), partly written, none reviewed yet. They are on this branch,
  **`claude/0.0.6`**, on top of 0.0.5 (Full access): see `STATUS.md` for what compiles and
  passes. `package-briefs.json` holds the instructions each package's developer got.
- **History of the paused run:** `history/STATUS-at-pause.md` (which package was where when
  the owner stopped it), `history/packages-workflow.js.txt` (the build → security and correctness
  reviews → fix pipeline that ran them) and `history/resume-args.json` (its arguments, with
  `resume: true` for every package that had not finished).

## Git history: read this before merging anything
The paused work was first committed on the 0.0.5 branch (7827001, also the branch
`claude/0.0.5-paused`) and then undone there by a0177eb, which set every file back to 0.0.4.
So 7827001 is already in the history of the 0.0.5 branch and of `main` once 0.0.5 is merged:
- merging `claude/0.0.5-paused` into anything that has 0.0.5 does **nothing**;
- merging `main` into `claude/0.0.5-paused` fast-forwards it and the work **disappears**.

This branch got the work back by reverting a0177eb on top of 0.0.5 (commit 71310d9). Do not
use `claude/0.0.5-paused` for anything any more; it is kept only as a record.

## How to pick it up
1. Keep this branch up to date with 0.0.5: merge `claude/zealous-brown-d3vmfb` (0.0.5's
   branch, pull request 4) while 0.0.5 is not merged, and `main` once it is. Ordinary merges
   are fine now.
2. Update `spec.md` for 0.0.6:
   - approval modes are three (Ask first, Allow all, Full access); see ARCHITECTURE.md 13.3
     "Full access (0.0.5)" and `docs/dev/0.0.5/full-access-review.md`;
   - D11 (a task for a coding app always asks) now reads: always asks in Ask first and Allow
     all; in Full access nothing asks (the owner's Full access rule; the terminal could start
     those apps anyway). This is how the engine was merged (`crates/policy/src/engine.rs`);
     confirm it with the owner;
   - item 6 changes item 2's D4 (see above);
   - items 5 to 8 get their own parts (5 once it is researched; 7 and 8 are small).
3. Resume the packages: each package's developer continues from the code already written
   (`history/STATUS-at-pause.md` says where each stood), then two independent reviews
   (security, correctness) and fixes, as `history/packages-workflow.js.txt` did. Then design and
   build items 5 to 8, then integration: every check, every e2e suite, a whole-release
   review, docs, version 0.0.6, CI on all three systems, a draft pull request.
4. The owner checklist (spec Part 4) still applies: graphics card settings on the owner's
   Radeon RX 9070 XT, the 16 GB Mac, and Claude Code / Codex using the shared model.
