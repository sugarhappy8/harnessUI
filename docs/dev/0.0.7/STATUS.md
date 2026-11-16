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

- P3 review: no critical issue (the guard still checks every event against the real hit
  test and foreground window). One major: the app now raises its own window, so an approval
  card could already be armed when it appears (its 700 ms misclick delay ran while hidden).
  Minor: a Stop racing the minimise could leave the window minimised; the second keyboard
  hand-back ran after Stop; the window took the keyboard at turn end; under Allow all a
  keyboard action could take the keyboard while the user typed in harnessUI; it minimised
  even when not in the way; the state machine keyed on the chat, not the turn; hardening
  (cloaked windows, ownership check on the main thread). All being fixed by P3 (round 1).
- P4 built: `<tool_list>` + `<tool_call_guide>` in the system message (stable across turns,
  ≤5 % of context / 2,400 tokens, steps down to shorter forms), computer keywords widened and
  the whole computer group shown together, made-up `<tool_output>` replies retried once with
  a note, hints after failed PowerShell/zsh commands, wrong-call messages list what a tool
  takes, `sameCallLimit` setting (2–100, default 5) in Settings → General. Real model
  (Qwen3-0.6B, greedy) still calls read_file with the list and guide. Under independent
  review. Evals/benchmarks don't get the list (separate turn input) — note for later.
- P5 review: no critical issue; 3 major: (M1) a one-character address typo (`http:/host`)
  made the key check and the Cloud mark read a different host than the client (key over
  plain http, no Cloud mark); (M2) Automatic planned 1,000,000 tokens for Claude models
  (decided: Automatic = min(reported, company default) = 200,000 for Claude, 100,000 for
  Haiku 5.5; larger only by explicit choice); (M3) a saved key followed an edited endpoint to
  a new host. 7 minor (key left in mid-stream error text; refused partial reply kept;
  "local network" by name not address; thinking off not really off on some Claude models;
  history edits break replayed thinking; Claude's 10 MB image limit; Responses-only OpenAI
  models listed) plus nits. All being fixed by P5 (round 1).
- P2 review: no critical; 2 major + 11 minor recorded in `review-p2-findings.md` (the
  automatic Hugging Face listing becomes user-triggered). To fix after P6b; m5 given to P6b.
- P4 review: nothing critical; one major: at a 4,096-token context the list pointed models
  to find_tools/call_tool that the turn didn't offer, and its size pushed edit_file out of
  every turn. Minor: new computer keywords matched coding requests ("chrome extension code",
  "open settings.json", "my laptop"); made-up-result detection fired on prose mentions; two
  PowerShell hints misfired (`&` messages; cmdlet echoed in the output); a quadratic scan on
  big outputs; the evals harness ran without the list. Plus nits. All being fixed by P4
  (round 1).
- P1 finished with `--ctx-checkpoints 8` (real server: at most 8 copies kept, memory grew
  74 MB vs 275 MB with the default 32 on Gemma 3 270M). Gemma 4 12B at 65,536: 1,504 MiB on
  the card, 2.5 GiB of saved copies in computer memory (10 GiB at the default 32). Under
  independent review.
- P3 round 1 done: all nine review findings fixed (cards re-armed when the window is shown;
  three ways back: Stop focuses, a card comes to the front without the keyboard, turn end
  restores without the keyboard; steps aside only when in the way; under a grant only right
  after a card answer or 1.5 s without input; state keyed by turn; ownership checks on the
  main thread). computer 146, app 849, vitest 832 pass. Independent verification running.
- P5 round 1 done: all 3 major, 7 minor and the nits fixed (one shared address parser; cloud
  Automatic = min(reported, company size): Claude 200K, Haiku 5.5 100K, OpenAI/OpenRouter
  128K, abliteration.ai 32,768; keys never kept across a host change; stream errors scrubbed;
  only localhost/private IP literals count as local — a `.local` name with a key now needs
  https; effort low where thinking can't be switched off; earlier thinking replayed when
  unchanged; 1-hour cache on system/tools; big pictures become a note for Claude).
  agent 229 lib / 19 cloud / 84 turn_loop, vitest 832. Independent verification running.
- P6a round 1 done: all five review findings and the nits fixed test-first (per-chat Tools
  panel; Task tab follows a reply known only after a reload; edit Send off while busy and a
  refused send no longer blanks the running reply; StrictMode-safe follower; copied
  attachments can't be combined with new ones; the turn slot is taken first so Stop right
  after sending works). app 849, vitest 832, clippy clean. Its verification is folded into
  the whole-release review (all findings were minor).
- P6b done: all nine bugs fixed test-first (resume keeps finished files and verifies instead
  of refetching; paused groups show as paused and don't auto-resume; one atomic duplicate
  check in `DownloadManager::start`, incl. the P2 review's m5; Cancel deletes finished files
  no installed model uses; a failed last step shows and retries alone; Installed rows can
  always be removed; `load()` picks the backend before stopping anything; Delete takes the
  image file too unless shared; approval cards survive a window reload via
  `pending_approvals`). app 849, models 137, vitest 832. NOTE: WIP commit 933c56a briefly
  contains old-behaviour copies of downloads.rs/hub.rs/runtime_manager.rs/turns.rs (P6b
  showing tests fail first); later commits have the real code. Under independent review
  (deletion safety first).
- P2 round 1 started (findings in `review-p2-findings.md`; Hugging Face listing becomes
  user-triggered).
- P4 round 1 done: all 10 findings fixed (the list says how tools are reached this turn from
  the turn's own tools; `roster::fitted()` keeps the core file tools in at 4k; computer
  keywords as phrases with bare words weak only; result-tag detection only for real
  blocks; exact PowerShell texts; bounded scan; evals send the list; nits). agent 229,
  tools 220, app 850, vitest 832; real model still calls read_file. Verification running.
