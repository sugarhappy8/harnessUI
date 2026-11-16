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
