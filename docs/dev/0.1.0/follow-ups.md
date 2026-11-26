# 0.1.0: what is still open

The plan's tasks are built and reviewed, except Task L17, which waits for the owner's benchmark.
A whole-branch review then ran, and its findings went to one fix wave of four fixers, run one
after another. The work was wrapped up before the fourth fixer and the wave's re-review ran.
This file lists what is left. [owner-checklist.md](owner-checklist.md) lists what only the
owner's computers can show. [rulings.md](rulings.md) lists every decision made on the way.

## Before release

- **"Before you tag v0.1.0"** at the top of [owner-checklist.md](owner-checklist.md): the
  benchmark and Task L17, the real runs, the legal review of the AI labels, the website pull
  request, the CHANGELOG date and the updater key.
- **The fix wave's re-review did not run.** Three fixers' work is on the branch, each checked
  by its fixer with the full test suites and by CI. No second reviewer has read it yet:
  - backend, `8e191f5..43349d7` and `43349d7..586e243` (the local time estimate, cloud charges
    and errors, run ordering, local engine edges, exports, shared model files, the Windows drop);
  - webview, `586e243..985aff7` (ordering, deleting, the viewer, the card stream, the Windows
    drop, the held extra settings, wording).

  A scoped review of those ranges is worth doing before merging.
- **The Windows picture drop is untried on real machines.** The webview waits 300 ms after a
  native drop and skips it if an HTML5 drop took the files, and finds the zone from the drop
  position divided by the screen's pixel ratio. Try it on Windows, on the Mac and on Linux.

## Not done in the fix wave (the docs and tests fixer)

Two of its items are done: the release gate above, and the Wan 2.1 decoder and settings line in
the owner checklist's benchmark section. These are all small:

1. **ARCHITECTURE.md tables.** *(Done in 0.2.0.)* Section 4's process table has no `sd-cli` row. Section 13.1's
   trust table has no Studio engine row, and its "Model files" row names only llama-server.
2. **ARCHITECTURE.md 13.6.** *(Done in 0.2.0.)* The Studio row's triggers leave out what runs at launch:
   `resume_interrupted` checks each cloud video still waiting or downloading, and downloads its
   result with the endpoint's key. Add it, with whatever else starts a request.
3. **ARCHITECTURE.md accuracy.** *(Done in 0.2.0.)* Five statements no longer match the code. One example: "the
   engine is given only its model files, its prompt file and the run's folder", although added
   pictures are given too. The review lists all five.
4. **The sidecar holds the prompt.** *(Done in 0.2.0.)* SECURITY.md (557–559), README.md (49) and CHANGELOG.md (24)
   call each run's `.json` "a record of its settings". It holds the prompt too, so it should
   say so next to "the prompt is never written into a local record".
5. **"Signed" claims.** *(Done in 0.2.0.)* SECURITY.md (648–650) and ARCHITECTURE.md (352) say `sd-cli` is signed
   and notarized with the app. That holds only once the Apple Developer ID exists. README.md
   (73) has it right.
6. **CHANGELOG.md.** *(Done in 0.2.0.)* Line 49 ends with an internal sentence about the build machine, and two
   more C2 items are open.
7. **README.md.** *(Done in 0.2.0.)* The first sentence still describes the app as chat, tools, skills and MCP,
   with no Studio. Line 58 still says "Versions 0.0.x are early test builds".
8. **ARCHITECTURE.md 7.1, the VC++ bullet.** *(Done in 0.2.0.)* llama.cpp needs the first three DLLs. The Studio
   engine also needs `msvcp140_codecvt_ids.dll` and `vcomp140.dll`.
9. **e2e scripts.** *(Done in 0.2.0.)* m8's `SETTINGS_PAGES` lacks "Search". `install_klein` in m8 has small
    issues. The fake `sd-cli` should split only on `\n`, keeping line ends, as the Rust fake
    does. `settings_tab`, `close_settings` and `studio_tab` belong in `webdriver.py`, shared by
    m8 and m12.
10. **`wire_tests.rs`.** *(Done in 0.2.0.)* The mock should record and check the `Authorization` header, so the
    key is seen to reach the endpoint. Add a comment on the drop order of `WireHarness`'s
    fields.
11. **`scripts/docs_test.py` (around 527).** *(Done in 0.2.0.)* Check local-before-cloud in the CHANGELOG's Added
    section, not in its opening paragraph.

## Kept for later, by choice

The final review found these. Each was judged safe to ship and is listed for the owner:

- **Partial results.** A run that keeps results 0 to k−1 and then cannot keep result k (over
  its cap, wrong type, full disk) ends failed, and for cloud runs charged, with the first ones
  still recorded as its outputs.
- **`Store::file_uses`** is public and tested but unused. Deletes compare paths through
  `hub::used_elsewhere`.
- **Moderation wording.** OpenRouter answers 400 or 403 whose message contains "moderation" or
  "flagged" are shown as blocked, even when they are about something else.
- **Cloud test gaps.** The `provider.options` shape is checked against a literal rather than
  the fixture, and one branch of `picture_field` has no test.
- **Export refuses the whole gallery tree.** This is broader than the real collisions (ruling
  A16).
- **Module cycles in the webview store.** `studio.ts`, `studioRun.ts` and `studioResults.ts`
  import each other. It works, but splitting it would be cleaner.
- **Smaller ones:**
  - `ParamField` could be split per widget;
  - the "more that fit" count counts both kinds;
  - the B3 held-extras test leans on module state;
  - the preview has a short `try_lock` window (L14);
  - a delete and a start can race (L16);
  - a SeekHead edge case (L13);
  - a start-error edge case (L11);
  - the chat Composer's own HTML5 drop on Windows (from before 0.1.0).
- **An owner choice.** The usage line shows the full path, not "~".

## The Linux `strace` checker, known gaps (0.2.0 final review)

`scripts/check_strace.py` guards against a pack that reaches the network by mistake. The
`reverse.yml` run inside a network namespace with no network is what stops traffic. The
0.2.0 final review reproduced these ways for a program written to evade the checker, each
with a real strace 6.8 log, and left them for later:

- **A payload containing `) = -1`.** `result()` reads such a line as failed. That hides the
  rest of a `sendmmsg` past the 32 messages strace prints, an `execve` and a file write. The
  fix is to read the result only from the end of the line.
- **`AF_UNSPEC`.** It is exempt everywhere, but Linux delivers a `sendto` with it on an IPv4
  socket, and binds to 0.0.0.0 with it. The fix is to allow it in `connect` only.
- **Stale `-yy` details.** After a reconnect to a non-loopback address on port 0, strace still
  prints the old loopback peer, so an addressless `send` passes. The fix is to refuse port-0
  connects to non-loopback addresses (the workflow pins the host name to 127.0.1.1), or to
  judge an addressless send by the fd's last traced `connect`.
- **IPv6 only.** A message is judged by its peer only when no `sa_family=` appears anywhere
  in it, so a SOL_IP cmsg or an `AF_UNSPEC` `msg_name` can pass. This is from reading the
  kernel's code; no IPv6 log was recorded.
- **A false finding.** A loopback TCP client that binds before it connects is reported,
  because strace's details go stale. Nothing in the nightly run does this today (run
  45796320126 passed). The fix is to exempt TCP sends from the unknown-peer finding, since
  every TCP connect is traced and judged.

## Owner decisions that remain

- An Apple Developer membership. Signing, notarization, and keeping macOS permissions across
  updates all wait for it.
- The contact line in SECURITY.md.
