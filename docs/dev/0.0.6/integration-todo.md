# 0.0.6: things for the lead's integration

Collected while the packages run. Each needs doing (or a decision) before the release review.

- **WP7, path escaping outside its files:** the Tools panel (`ToolsPanel.tsx` around the
  folder list) and Settings → Permissions (`PermissionsSettings.tsx`) show folder paths raw,
  so a right-to-left override in a folder name displays reversed there while the chip shows
  it escaped. WP7 left a patch (wraps the path in `visiblePath` with `<bdi dir="ltr">`) and a
  TODO note in the `folder-chip` e2e scenario: apply it, turn the note into an assertion.
- **WP7, narrow windows:** below about 950 px the chip shows only an icon in Full access and
  "No folder" without "Choose". Full access must say "Whole computer" in words at every
  width (the owner asked for it to be plain); decide the short forms for the rest.
- **WP7 and WP6 developers returned before their checks finished** (they waited on a
  background run); the checks were re-run by the reviewers and checkers, but re-run WP7's
  `folder-chip` scenario at integration.
- **WP6 done after four fix rounds** (the last two only on the coding-app classifier; the
  lead added the `claude/versions/<build>` folder of Claude Code's own installer). The limits
  that stay are listed in the module docs of `crates/policy/src/command/coding_apps.rs`
  and pinned by `patterns_that_fix_too_little_are_a_known_gap`: copy them into ARCHITECTURE
  13.3 and SECURITY.md. History:
- WP6 had finished two fix rounds with one major left (a long option such as `--frozen`
  after `uv run` hid the program from the coding-app rule) and two minors (one-letter
  patterns after a wrapper; slow reading of many parentheses): round 3 run wf_a82a9107-f79.
  WP6 also fixed a classifier-wide bug: a backslash before CR LF let `echo \<CR><LF>rm -rf x`
  read as one `echo` command (a grant for echo covered it; Allow all ran it); any carriage
  return in a POSIX line now makes the line complex. Document in ARCHITECTURE 13.3.
- **Item 4 (WP4a, WP4b, WP4c) done**, no open findings. WP4a saw the `compare-models` e2e
  scenario fail twice in three runs (a race in its final add loop, which checked buttons
  once without waiting); re-run it several times at integration and confirm WP4b's fix.
  WP4c's table of quotes (in its report, journal of wf_da155c8a-e67) goes to the owner for
  review before release.
- **WP3a, WP3b, WP5a, WP5b done.** Left as minor:
  - WP5b: "Make a new key" while a "Show key" read is still on its way leaves the old,
    refused key on the page and Copy copies it (`OtherApps.tsx` makeNewKey does not clear the
    pending read or `key`); the old-key note is one flag for all apps and is cleared by copying
    any step, even one without the key. Fix both (small).
  - WP3a: `defang_app_notes` in `src-tauri/src/subagent.rs` still lets the extra invisible
    characters hide an "[App note]" marker (WP3a fixed it in `crates/agent/src/prompt.rs`
    with `hidden_in_summary`); reuse that check there.
  - WP3a: the real-model `llama_memory` test takes very long under load (36 min at load 20;
    about 2-4 min alone); keep it out of the default CI run if it is slow there.
- **WP1a, WP1b, WP2, WP8 done.** Left (minor), to fix or decide at integration:
  - **WebP pictures** (lead decision): closing the S2 hole (`--video-ffmpeg-dir` to a fixed
    empty folder, so llama-server never starts ffmpeg/ffprobe) means llama-server can no
    longer read WebP; the composer still accepts `image/webp` (`Composer.tsx`,
    `attachments.rs`). Convert WebP to PNG in `attachments.rs`, or stop accepting WebP with a
    plain message. Verified end to end by WP1a (PNG 200, WebP 400).
  - WP1a: a model that refuses Save memory on a graphics card in use still gets the
    graphics-card wording (rare head-size-80 models); needs a contract change (who refused).
  - WP1a: the contract test `the_alias_is_the_model_id_in_every_api` failed once at
    `/v1/messages` (intermittent, cause unknown; it passed on every other run). Watch it in CI.
  - WP2: at 1100-1200 px with messages the model name in the header can shrink to one
    letter and the memory number is cut; in an empty chat the ready line's reason and
    "· Change" are cut up to 1366-1440 px. Header layout pass (also with WP7's chip and
    item 6's usage line).
  - WP2: cancelling a wait that the backend started on its own (pickModel with no question)
    leaves the chat on the new model while the old one stays loaded (`app.ts` pickModel's
    restore checks `answer.whenIdle`, undefined on that path).
  - WP8: a model opened through a link or mapped drive is listed under its real path when not
    loaded; picking it there switches the chat that opened it to that spelling.
- **WP9g done.** For the lead:
  - Confirm the loosened id rule: a result id belongs to every chat whose own successful
    result about the **same canonical file** showed it (two chats looking at the same file
    share rea's ids); an id seen about another file is "contested" and readable by no chat.
    The spec said ids are never given to another chat.
  - Stop during a slow rea start leaves the state "Starting" and busy (WP9b's `process.rs`,
    `ReaGuard::ensure_started` sets `starting` before the await): make sure WP9b's fix covers
    it (drop guard or a start in its own task).
  - The `reverse-tools` e2e probe's last step expects `{"installed": False, "items": [...]}`;
    WP9h's `reverse_drop.rs` now also returns `groupOn: false`: update the probe (WP9h).
