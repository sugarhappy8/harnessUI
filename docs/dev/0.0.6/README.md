# 0.0.6 plan

These are the things the owner asked for after 0.0.4 (four at first, a fifth added later). Work on them started as "0.0.5"
and was paused on 2026-11-12, when the owner chose to ship **Full access** as 0.0.5 first.
They are now the plan for 0.0.6.

1. allow per model on loading context window setting and it auto compresses.
2. More and more model loading setting and model reloading but simplify it to normal human language.
3. Allow for local loaded models to be able to talk to Claude or codex apps upon user request.
   (The owner chose both directions: other apps may use the model loaded here, and the local
   model may hand a task to Claude Code or Codex.)
4. Add model comparisons for models so users can compare different models and what their good at from the download huggingface thing we have
5. (Added 2026-11-12, while 0.0.5 was being finished.) "i wanna install https://github.com/morluto/rea as a main feature in it but with a free decompiler. maybe lets keep that for version 0.0.6." Not designed yet: the first step is to read that repository (what it does, its license, what it needs), pick a free decompiler that fits (its license must allow shipping it with the app), and write the spec like the others.

## What already exists
- **Design:** `spec.md` (revision 2, with "Amendments after WP0"), the map of the code it is
  based on (`map.md`, `map-details.md`) and the three independent critiques that shaped it
  (`critique.md`). Line numbers refer to the code as of 0.0.4.
- **Groundwork (WP0):** shared types, command stubs and component splits, built and
  independently checked; reports in `report-WP0-*`. The contract every package builds on is
  the section "CONTRACT (updated)" of `report-WP0-fix.md`.
- **Half-built packages:** about 26,000 lines of partly written code for the eleven work
  packages, saved on the branch **`claude/0.0.5-paused`** (last commit 7827001). None had been
  reviewed yet. `package-briefs.json` holds the instructions each package's developer got.

## How to pick it up
1. Bring `claude/0.0.5-paused` up to date with `main` (which then contains 0.0.5, Full access).
   Expect conflicts in the approval code (`crates/policy/src/engine.rs`, the chat header's
   mode switch, `src-tauri/src/turns.rs`), because Full access changed them.
2. Update `spec.md` for anything Full access changed: approval modes are now three (Ask
   first, Allow all, Full access), and item 3 direction B's "always asks" rule (D11) must say
   what happens in Full access (the owner's Full access rule is: nothing asks).
3. Resume the packages as `docs/dev/0.0.5/STATUS.md` on the paused branch describes: each
   package's developer continues from the code already written, then two independent reviews
   (security, correctness) and fixes, then integration, docs, version 0.0.6, CI.
4. The owner checklist (spec Part 4) still applies: graphics card settings on the owner's
   Radeon RX 9070 XT, the 16 GB Mac, and Claude Code / Codex using the shared model.
