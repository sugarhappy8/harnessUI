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
- **Item 5: WP9a, WP9b, WP9c, WP9d, WP9g, WP9h done; WP9e running (wf_59ddd95e-968).** For the lead:
  - **Ghidra pin:** `reverse.lock.json` pins Ghidra 12.1.4 as `first-download-only` (GitHub's
    digest cannot be read from this sandbox). A release build refuses to offer a part without
    its publisher's check, and `pin_reverse.py --check --release` fails on it, so a release
    built today shows Ghidra as "Not available in this version of harnessUI." WP9e adds a CI
    step that reads the digest with the workflow's token; once it confirms, flip the lock to
    `github-digest` (WP9e's report says how). Otherwise Windows has no decompiler in 0.0.6.
  - item5-spec's WP9a line still says `(mode & 0o777) | 0o600`; the code uses
    `(mode & 0o755) | 0o600` (group/other write bits dropped). Update the spec.
  - WP9a added `UpdateInfo.part` and `ReverseError::ServerError { host, status }`: check WP9d
    shows "Node.js"/"Java" for a part update and the server-error text.
  - WP9c (minor, left): a link whose target is not valid text is treated as missing by
    `follow_links`, so a chain of two such links can still make the system look up a network
    place and the folder passes. Fix: treat an unreadable link target as refused.
- **Leftover fixes run A (wf_c6b8dca9-052) done.** Six fixes landed with tests and reviews; the
  webp and model-list fixes had a second round for a major each (decoding WebP now in a
  bounded helper; the list no longer offers paths Installed doesn't know). Still open (minor):
  - pick-cancel: the pick counter is shared by all chats, so a quick pick in another chat stops
    the restore (a regression on the asked-to-wait path); Cancel while another chat is open
    still leaves the picking chat on the new model; re-picking the loaded model while a switch
    waits doesn't cancel the backend's waiting load. Fix in `app.ts` pickModel (per-chat).
  - webp: WebP sent by other apps through the sharing proxy, or by MCP tools, still reaches
    llama-server as image/webp and fails (convert in the proxy and the tool-result path too,
    or refuse with a plain message).
  - helper hand-over: escaping now also changes ordinary non-ASCII text a helper may need word
    for word (ZWNJ/ZWJ in Persian and Indic scripts, emoji variation selectors, RTL marks);
    consider escaping hidden characters only next to bracketed markers. Near-miss markers with
    no hidden characters still pass (doc wording must not overclaim).
  - Other apps: a setup step copied while the new key arrives (a few ms window) is left off
    "Apps to set up again".
  - `tool_catalog.rs:250` TODO(0.0.6) (tool selection counting words of tool names; rea's
    names) and `attachments.rs:643` TODO(0.0.6) (documents in the mentions' share): decide
    before release, or keep with a reason.
- **Leftover fixes run C (wf_6a9d631b-ae5) done** (per-chat model switch cancel, WebP on the
  proxy and tool paths, helper escaping scope, Other apps copy race). Minors found by its
  reviews, for the final fix round (with the release review's findings):
  - pick-cancel: picking an endpoint in the chat whose switch waits leaves the wait (the old
    model is unloaded and the new one loads after the reply although no chat uses it); cancel
    the wait on any pick in that chat. A wait taken over by a newer wait (two chats picking the
    same model, or b then c in one chat) can leave a chat on a model that never loaded; keep
    each chat's model from before its first unfinished switch. Fix the comment ("goes back to
    the model it had before", which can be an endpoint).
    Done (pick-cancel-minors): an endpoint picked in the chat whose switch waits cancels the
    wait; each chat keeps its model from before its first unfinished switch, and a chat whose
    wait a newer load of the same model took over ends with that load (keeps the model if it
    loads, goes back on Cancel). Tests in `src/store/modelChange.test.ts`.
  - helper escaping: a block tag split by a hidden character is missed when a control or tag
    character and a letter follow its name; the docs and report still say summary hidden
    characters are escaped, and the Context tab's summary view changed without a note.
  - webp (proxy): conversion runs before the busy counter and after the upstream snapshot, so
    D3 and "Used by another app" do not cover it and the request can go to a stopped server;
    the tool/stored-picture path decides by the label, so a readable PNG/JPEG labelled
    image/webp is dropped with an untrue note; the per-request total refusal says one picture
    is too large.
- **Docs written and reviewed (wf_50b56912-278).** 11 `TODO(lead): confirm after WP9e` markers in
  ARCHITECTURE.md (1.4, 9.3, 13.6, 16, 17, 20) to confirm or remove once WP9e is merged.
  Flagged by the writer for the release review: rea's `describe` refusals `LINK_OUT` and
  `DEVICE_IN_FOLDER` name a path inside a folder before any approval, which departs from the
  13.9 rule that messages before approval never name files from a folder.
  Done (rea-walk-refusals): `LINK_OUT`, `DEVICE_IN_FOLDER`, `UNREADABLE` and
  `HOLDS_APP_FOLDERS` are fixed sentences now; the entry's path is only on the card's preview
  and in the audit row (`WalkRefusal::detail`). Tests in `crates/reverse/src/describe/tests.rs`
  and the `reverse-describe` probe; item5-spec section 6 and ARCHITECTURE 13.9 updated.

- **Final fix round (07:15 UTC).** Release review processes/network + tests (wf_0772ffdb-bab)
  confirmed 4 minors (scratchpad rr-2.json). The throwaway `crates/reverse/tests/zz_review_wp9c_sec.rs`
  (hidden by .git/info/exclude, failing locally) is deleted; remove the `zz_review_*` /
  `zz_check_*` / `*_review_tmp*` exclude lines at the final pass and check `git status`.
  Every remaining minor goes through v006-final-fixes (fix, review, refix of all findings, check):
  batch a wf_bfaa840a-6b8: share-key-handoff, proxy-timing-flake, windows-locate-tests,
  webp-proxy-minors, unreadable-link-target; batch b wf_55eeb8d4-8e0: pick-cancel-minors,
  helper-escape-minors, mentions-share-docs, wp9d-part-update, rea-name-words.
  Lead decision (share-key-handoff): when a coding app is set up to use harnessUI's sharing,
  the hand-off checks that sharing is on and listening (else a plain refusal) and passes the
  child exactly one extra variable, harnessUI's own sharing key, under the name its own setup
  uses; every other secret variable stays stripped. The key only goes to a program whose
  configuration points at the live local sharing address.
- **WP9e done (wf_59ddd95e-968; report in scratchpad reports/WP9e.md).** CI run 43380365172's
  `ghidra-digest` job confirmed GitHub's digest for Ghidra 12.1.4 (artifact
  `ghidra-release-record`, id 14015748800, artifact digest checked by the lead). Batch c
  wf_dad2159c-dae: ghidra-pin-gate (record must name its CI run, `github_digest_run` in the
  lock, release.yml gate, lock flipped from that record), rea-linux-tmp (Ghidra's session in
  /tmp on Linux because the app folder is under a dot folder; private runtime folder or a
  documented limit; Java perf file), arch-todo-lead (the ARCHITECTURE.md TODO(lead) markers).
- **Batch d (wf_728c1760-d70, 07:40 UTC):** mcp-rename-settings (renamed server keeps its
  chats' tool choices, last choice and grants, or the note says what to switch on; keychain
  failure during the rename), canonical-in-notes (pre-approval messages name the path the model
  gave, never a resolved link target; rea describe and files tools), names-raw-sidebar-welcome.
  Still to queue after the header refix ends: backend-check waiting line (scratchpad
  open-backend-check-d3.json: register the wait before the backend probes; the waiting line
  only while the backend waits).
- **Header layout done (wf_3b4d80b6-e9b, two rounds).** Batch e (wf_09edf0f5-041):
  backend-check-waiting (wait registered before the probes; waiting line only while waiting),
  rea-call-recheck (server.rs reachable/still_there run walk::safe_to_resolve before resolving,
  found by the rea-link-text review). Note: batch a's unreadable-link-target is the finding
  rea-link-text already fixed; its agent re-reads the code first.
- **wf_57274b94-12f done** (rea start stuck, Settings list scroll, Compare fit words, Tools
  panel rea note). Batch f (wf_ab199ebb-70b): settings-dialog-minors, compare-fit-minors,
  tools-panel-file-note. Queued after batch c's rea-linux-tmp: rea-start-guard (doc comment
  says what the managed server really does when a start is dropped; the guard's stop path
  also removes the run temp folder; scratchpad reports/fix-rea-start-stuck.md OPEN).
