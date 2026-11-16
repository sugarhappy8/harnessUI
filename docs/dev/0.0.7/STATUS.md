# 0.0.7 status

Keep this current: a session can stop in the middle, and the next one starts from this file
and `README.md`.

## 2026-11-16
- Branch `claude/0.0.6` restarted from `main` (d3a37c5, the 0.0.6 merge). Plan in `README.md`.
- Packages, built in parallel, each reviewed independently before it is committed:
  - P1 memory estimates (item 1a)
  - P2 image files (items 1b, 1c)
  - P3 computer use (item 1d)
  - P4 tools in the system message, call and shell guide, repeat limit (items 2, 3)
  - P5 cloud endpoints (item 1e)
  - P6 bug hunt (item 4)
- Baseline CI on the plan commit (run 43811806801) failed only the macOS
  `progress_keeps_a_call_alive_and_its_text_stays_out` (mcp-host `managed_e2e`): ten progress
  notes 300 ms apart against a 1 s limit and a 4 s cap came more than a second late on a busy
  runner. Fixed in f65f107 (five notes 600 ms apart against a 2 s limit; same property).
- CI on f65f107 (run 43815674194, code = 0.0.6 + that test fix): Linux, Windows (tests and
  installer) and the Ghidra digest passed; the macOS `managed_e2e` passed too. macOS failed
  `real_llama_server_agent_turn` (`crates/agent/tests/llama_agent.rs`): with the server's
  default sampling the 0.6B model wrote a made-up `<tool_output>` block instead of calling
  read_file. Fixed in aee9be3 (greedy decoding, fixed seed; passes locally with the real
  model; to be confirmed by the next full CI run). The Intel Mac smoke test failed only
  because the macOS job made no disk image. The made-up tool result was passed to P4 as a
  real-model behaviour to handle.
- Owner, after trying computer use: "the model still can't use my computer too well, they
  don't know what tools they have". Cause found: adaptive tool selection shows the Computer
  group only for words like "screen", "desktop", "mouse"; "open Notepad", "use my pc",
  "take a screenshot" bring no computer tools (or the browser's). Added to P4.
- P6 hunt finished. Findings (all fixed in P6a or P6b):
  - P6a (started; chat and frontend): the step-limit notice and its Continue button follow
    you into another chat; edit-and-resend drops attachments; a pending approval in another
    chat is invisible; stale "running" tool cards; `send_message` saves before its busy check;
    event-listener cleanup race (ToolsPanel can save the previous chat's tools).
  - P6b (after P2, which edits the same files): resume re-downloads finished files from zero;
    pausing after the first file leaves the group stuck as "downloading"; two groups for one
    file corrupt each other; cancel leaves finished files on disk; a failure after the last
    file leaves the group "downloading" forever; a downloaded model whose files are gone or
    outside the models folder can't be removed from Installed; `load()` stops the old model
    before checking the backend can run; deleting with files keeps the image file.
    Added from P6a: an approval card pending when the window reloads is lost (keep the card
    in `PendingCard` in `turns.rs` and read it back at startup).
- P3 (computer use) built: harnessUI minimises itself before approved input and comes back
  for cards, turn end and Stop; keyboard handed back to the planned window; 27 new tests.
  Under independent review.
- P6a (chat/frontend fixes) built: all six bugs fixed with tests (vitest 804, app 827).
  Under independent review. To re-check in the integration e2e run: "stop and regenerate"
  and "regenerate after error" failed in P6a's real-app run under 100% CPU (text present in
  the rows, `innerText` caught up ~1.5 s later because of `content-visibility: auto`).
- P6a review: nothing critical or major; five minor findings (ToolsPanel could still save the
  previous chat's tools in a narrow window; the Task tab ignored a reply known only after a
  reload; the edit box could send while a reply ran and blank it; a StrictMode late answer
  could leave a reply shown as running; copied attachments skipped the per-message limits)
  plus nits. All being fixed (P6a-r1).
- P2 (image files), P1 (memory estimates, plus `--ctx-checkpoints 8`), P5 (cloud endpoints)
  built. P2 and P5 under independent review; P1 finishing the checkpoint change.
- Disk filled up once (139 MB free): a reviewer had built its own 5.7 GB target. Deleted;
  agents told to build only in the shared target.

