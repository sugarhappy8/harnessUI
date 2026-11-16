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
- P1 review: the port is exact against the real binary for 8 more architectures. Two major:
  (M1) the loaded model's saved copies were counted twice when planning the next load (a
  Reload after a long chat could drop Automatic to 4,096); (M2) a fixed 8 copies made
  long-window models (OLMo 3 7B: 12 GiB of copies) "too large" at every size. Decided: the
  copy count per load comes from a memory budget (1–8), copies never turn a verdict red or
  drop Automatic to the floor. Plus 7 minor (hybrid models' copies, mistral4 MLA, wording,
  Automatic vs marks, unreadable system memory). Being fixed by P1 (round 1).
- P7 (website capture) built: `browser_capture_site` (rendered page with scripts removed +
  CSP, style sheets, assets, animations.json, screenshots, report.md), a site+folder grant,
  every capture request through the network checks; copy of a local test page matches the
  original at 99.94 %, gsap.com 99.16 %. Under independent review. Still to wire: its short
  method into the call guide (P4's guide.rs), and new built-in tools' default state for
  users who changed their tool choices.
- Disk: pruned 1,693 old test-build copies under target/debug/build (older than 2 h, newest
  kept per crate): 3.9 GB → 14 GB free.
- P3 verification: all nine round-1 findings fixed, no safety rule weakened. Seven residual
  problems found (the Allow all / Full access confirms not re-armed when the window comes
  back; a window event while still minimised counted as "brought back"; the card re-arm
  reacting a little after the window shows; Stop racing turn end could restore without the
  keyboard; the ±100 ms own-input window; docs; Windows quiet restores could still activate
  the window). All being fixed by P3 (round 2), plus nine owner-checklist items.
- P5 verification: all original findings fixed. New major from the m5 fix: the store of
  Claude replies to send back was shared by every chat on an endpoint and keyed by the
  reply's text, so a "Done." in one chat could carry another chat's thinking. Minor: some
  this-computer addresses (`0.0.0.0`, `[::]`, `localhost.`) still went through the system
  proxy; the host-change guard trusted a failing keychain; early turns of long chats
  evicted; more than 20 pictures in one request (stricter size rule) not handled. Nits:
  scrubbing too eager, one refusal cleared every chat, `drop_block` always on, picture size
  read from 64 KiB. All being fixed by P5 (round 2).
- P6b review: start check, start decisions, status, Delete, `load()` and
  `pending_approvals` sound. One critical: Cancel deleted "finished" files the download
  never wrote (a file already in place and only checked, or reached through a linked
  folder). One major: Resume could add an unchecked same-size file after a failed fetch.
  Minor: Cancel didn't wait for the task; an interrupted Cancel blocked that download for
  good; resume/cancel without the start lock; Delete could remove an image file the app
  didn't download; no progress while a file is checked in place; replies stopped before the
  backend was refused. All being fixed by P6b (round 1); Cancel now deletes only files the
  download itself wrote, inside the models folder.
- P4 verification: 7 of 10 fixed; the computer phrases and made-up-result detection only
  partly. New: helpers no longer shared the main turn's cached prompt (the helper's list
  left out `run_subagent`); the computer phrases still matched coding requests ("the project
  folder is on my desktop", "open excel files in python") and missed "open Google Chrome";
  `fitted()` reserved more room than the sent form needs; mixed instructions for
  `run_subagent` at 6–8k. All being fixed by P4 (round 2), which also wires P7's capture
  method into the call guide.
- P7 review (real captures of hostile local pages): paths and writes, size caps, the
  address checks for ordinary requests, the text the model reads, the grant and keywords
  sound. One critical: a hostile page could make the saved copy run script and call out
  when opened (elements in a foreign namespace slipped past the in-page clean-up; a page
  that replaced its root element left the no-script policy without effect; the picture
  block could land before that policy). Three major: saved SVG pictures could run script
  when opened directly; WebSocket traffic during a capture is not checked; `file://`
  addresses kept in the copy (an SMB request on Windows). Minor: new tool off for anyone
  who ever changed tools; no test with a hostile page. Decided: the copy is re-read in Rust
  with a real HTML parser and cleaned from that tree (the tree a browser builds from the
  file), SVGs rebuilt from a real XML parser, WebSocket blocked during a capture, and a new
  built-in tool follows its group when the whole group is on. All being fixed by P7
  (round 1).
- P3 round 2 done: all seven fixed test-first and mutation-checked (one shared click-delay
  hook for cards and the Allow all / Full access confirms; events while minimised ignored;
  the re-arm signal sent before the window is restored; Stop pending gives the keyboard
  whichever restore runs first; own-input window narrowed to 50 ms before / 25 ms after,
  plus a pointer-position check; Windows quiet restores with `SW_SHOWNOACTIVATE`; docs).
  computer 152 pass, clippy clean on Linux/Windows/macOS targets; app and vitest failures
  only in other packages' files being edited. Its verification is folded into the
  whole-release review.
- P5 round 2 done: one store of Claude's thinking per chat (32 chats, least recently used
  dropped whole), each reply keyed by everything sent before it plus what it showed;
  this-computer addresses get a loopback client (no proxy); keychain failures can't let a
  key follow a moved endpoint; many-picture rule (over 20: none above 3,000 px; at most 100
  or 600); scrubbing only of long key pieces; `drop_block` only to recover. agent all pass
  (lib 237, cloud 21, turn_loop 88), app providers/chat 49, clippy clean. A short
  independent verification of round 2 is running.
- Disk: 3.9 GB free at 10:26 UTC; removed build copies of the project's own crates idle
  for an hour and 14 old test binaries: 6.4 GB free.
- P6b round 1 done: Cancel deletes only files the download itself wrote (recorded per
  group), inside the models folder, unused by installed models; a failed file is checked
  again on Resume; Cancel waits for the task; half-finished cancels finished at start-up;
  resume/cancel take the start lock; Delete removes only an image file the app downloaded;
  "checking the file…" while a file is checked; the backend checked before replies are
  stopped. models 145, app 875 (1 failure from P1's new field), vitest 844, clippy clean.
  Verification of round 1 (deletion safety first) running.
- P4 round 2 done: helpers share the turn's list and prompt cache; computer phrases need a
  clear desktop request or a machine word ("my pc", "my screen"); "open Google Chrome" is
  no longer a web search; `fitted()` counts exactly what is sent; the made-up-result check
  only takes a block of its own; P7's capture method is in the call guide when that tool
  is on. agent all pass, tools 220, real model still calls read_file.
- Started the cross-package review of P3, P4 and P6a (also P3's and P4's last round and P6a's
  only verification) while P1, P2 and P7 finish their rounds.
- Disk: 3.4 GB free again at 10:40 UTC; removed idle build copies of the project's crates:
  4.8 GB free.
- P5 round-2 verification: the major is fixed (a fake API that runs Anthropic's documented
  prefix check saw no thinking cross between chats in 35 requests over 12 scenarios,
  including regenerate, edits, folds, model switches, helpers and evals); proxy, keychain,
  pictures, refusals, recovery and picture size fixed; round 1 still holds. Left: a key
  under 8 characters wasn't scrubbed; `localhost.` can go to DNS; whitespace-only system
  prompt changes; the per-chat limit comment; helpers share the chat's store; the
  over-20-pictures rule dropped two pictures it didn't need to. Small round 3 running.
- P1 round 1 done: a Reload after a long chat no longer counts the running model's saved
  copies twice (only what the computer's free memory fell by since the model became ready
  counts as freed); the number of saved copies is chosen per load from memory (1–8, half
  the computer's memory left after the 2 GB reserve and the model's own part there) and
  passed as `--ctx-checkpoints N`; copies never turn a mark red or shrink Automatic; the
  running state of hybrid models (Qwen 3.5, Qwen3-Next, Granite 4.0-H, RWKV, LFM2, …)
  counted as llama.cpp does; mistral4 keys-only; Automatic follows the placement's marks;
  unknown computer memory stays unknown; the bar shows the copies as their own item.
  Measured: with 1 copy, editing the last message re-reads the whole chat (9,618 tokens
  vs 787 with 8); kept the minimum at 1 so a computer short of memory is never pushed
  further. models 145, runtime 208+contract, app 876, vitest 844. Verification running.
- P6b round-1 verification: the critical is fixed — in every attack (files found in place,
  failed rows' old files, linked folders, a changed or missing folder, missing or corrupt
  records, a crash before recording, shards, links to installed files) Cancel deleted only
  files the group itself wrote. New major with a rare trigger: after a Cancel that timed
  out, the leftover cancelled row let a second download of the same file start, and the old
  group's clean-up could then delete the new one's partial file. Minor: Delete could leave
  a downloaded image file behind; the end of a timed-out cancel never showed; two windows
  where a cut-short cancel wasn't finished at start-up. Nits: lock handling, a drop guard,
  wording, long Windows paths. Round 2 running.
- P2 round 1 done (all of `review-p2-findings.md` except m5, which P6b fixed): device
  errors and backends that won't start are no longer taken for a broken image file;
  *Try it again* and *Download the image file again*; no automatic Hugging Face requests
  (a download saves the repository's image files; older downloads get *Look for an image
  file*; e2e m8 offline passes); log matching hardened against names from downloaded files;
  image files of other models never used; writing an `mmproj` file always asks; Windows
  device names rejected; running out of memory while the image file loads keeps the image
  file through the normal memory retries and drops it only as the last step. runtime
  (real server) all pass, app 877, vitest 844. Not done: Browse's own model + image
  download still takes the repository's first image file. Verification running.
- Cross-package review of P3, P4, P6a: nothing critical, no safety rule weakened; P3's
  last round and P6a verified (Stop, cards across reloads, per-chat Stop, attachments,
  listeners). Real-model test passes. One major in P4: a made-up result written in the
  app's own result format (`</tool_output id="…">`) wasn't caught. Minor: a helper asking
  for a coding app through `call_tool` was told the user could switch it on; the computer
  phrases still matched some coding requests ("write code to take a screenshot in
  selenium") and missed some desktop ones ("close notepad"); a checklist step that can't be
  done. Nits: a race that could leave the window minimised after a turn; *Continue* and
  *Resume* without a click delay. Small last rounds for P4 and P3 running.
- P3 round 3 done: "Minimise this window" pressed as a turn ends can't keep the window
  down; *Continue* and the Task tab's *Resume* ignore clicks for a moment when the window
  comes back; checklist items fixed. computer 153, vitest 847. P3 finished.
- P6b round 2 done: cancelled rows count as busy (the same download resumes the old group,
  anything else is refused); a "cancelling" marker saved before Cancel waits, finished at
  start-up, with "cancelling…" on the row; Cancel releases the start lock while it waits;
  clean-up deletes only its own partial files; Delete removes the image file its cancelled
  download wrote and plans from the row as it is after the wait; a drop guard ends a task
  that panics; Delete question wording; long Windows paths. app 888, models 145, vitest
  121 (models), clippy clean. Its verification goes into the final review.
