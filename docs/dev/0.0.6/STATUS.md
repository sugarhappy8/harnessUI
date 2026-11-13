# 0.0.6 status

Keep this current: the owner's usage limit can stop a session in the middle, and the next
one starts from this file and `README.md`.

## 2026-11-12, when the branch was made
- `claude/0.0.6` = 0.0.5's head (e48d934, pull request 4, not merged yet) + 71310d9 (the
  half-built work re-applied by reverting a0177eb; see README "Git history") + the docs commit
  that added this file.
- Merge result: one conflict, in `crates/policy/src/engine.rs` (D11 and Full access; resolved
  as README step 2 says). Two other places needed 0.0.5's names: the coding-app policy test
  (`src-tauri/src/outside_agent/tests.rs`) gained the `full_access` field and a Full access
  case; the chat view test mocked the header button by its old name (`AllowAllButton`, now
  `ApprovalModeMenu`).
- Checked on Linux in the session that made the branch:
  - `cargo check --workspace --tests`, `cargo clippy --workspace --all-targets -- -D warnings`,
    `cargo fmt --all --check`: clean.
  - `cargo test --workspace`: 1,923 passed, 0 failed, 6 ignored (42 test groups).
  - `npm run lint`, `npm run typecheck`, `npx vitest run` (416 tests in 50 files),
    `npm run build`: pass. `scripts/check-spawn.sh` and `scripts/notices.py --check`: pass.
  - Not run yet: the end-to-end suites, and Windows and macOS (no CI run on this branch yet;
    its commits so far carry `[skip ci]`).
- Passing tests do not mean the packages are done: they cover what was written. Where each
  package stood when it was paused is in `history/STATUS-at-pause.md` (WP4b's build had
  finished; WP1a and WP3b were in their final checks; the rest were partway; no package had
  been reviewed).
- `crates/agent/tests/wp3a_real_model_tmp.rs` looks like a temporary test a developer left
  behind; check it before keeping it.
- Items 5 to 8 of the plan are not designed yet.

## 2026-11-12, the 0.0.6 session's first check
- This container: `npm ci`, lint, typecheck, vitest (416 in 50 files), build, spawn check,
  notices, `cargo fmt --check`, clippy `-D warnings`, `cargo test --workspace` (1,923 passed,
  0 failed, 6 ignored): all pass. End-to-end suites not run yet. (A fresh container needs the
  Tauri system libraries from `ci.yml` and `rustup toolchain install 1.99` first.)
- Where each package stands (read against spec Part 3, not yet reviewed):
  WP1a ~95% (the optional `llama-fit-params` step not built); WP1b ~90%; WP2 ~90% (fit marks
  follow saved, not unsaved, settings); WP3a ~95%; WP3b ~90%; **WP4a not started**
  (`catalog_list`/`compare_facts` still return WP0 sample data); WP4b ~95% but shows sample
  data until WP4a; **WP4c not started** (catalog unchanged); WP5a ~90%; WP5b ~95%; WP6 ~90%
  (Codex flags never run: Codex not installed here; Windows file identity weaker).
  No package has had its security and correctness reviews.
- `crates/agent/tests/wp3a_real_model_tmp.rs` is a builder's leftover (its header says so);
  it passes by skipping without a real model. Delete it or rename it on purpose.
- Spec revision 3: three approval modes (D12), D11 as merged, D4 amendment proposed, items
  5 to 8 in Part 3b; research in `item5-rea-research.md` and `item6-usage-offloading.md`.
- The owner answered: keep D11 as merged; item 5 = rea with Kuna and Ghidra, installed
  optionally from its own Settings tab like the ROCm pack, files only (no running
  programs); item 6 = Fine control in each model's settings, live usage always visible;
  item 4 = publisher quotes, reviewed by the owner before release.
- 2026-11-12 17:50 UTC: the owner said go (with workflows). Running, all in this one tree:
  | Run | Packages |
  |---|---|
  | wf_abb8586c-a87 | WP1a, WP1b, WP2, then WP8 |
  | wf_48eea688-ae5 | WP3a, WP3b, WP5a, WP5b |
  | wf_612d61cb-2dd | WP6, WP7 |
  | wf_da155c8a-e67 | WP4a, WP4c, then WP4b |
  | wf_4aac3ffd-edc | design of items 5 and 6 (writes item5-spec.md, item6-spec.md) |
  Each package: develop (finish or build) → security and correctness reviews → fix → an
  independent check (up to two fix rounds). Script and briefs: the run's script under
  /root/.claude/projects/-home-user-harnessUI/<session>/workflows/scripts/; packages'
  notes are the runs' args. The developers never commit; an hourly check commits
  work in progress with [skip ci].
- 18:10 UTC: 0.0.5 merged into `main` (4189a3a, released as v0.0.5); `main` merged into this
  branch (no file changes). From now on merge `main`, not the 0.0.5 branch.
- 18:51 UTC check: WP1a developed (contract tests 11/11 and m10 pass with the real model),
  waiting for its reviews; WP1b, WP2, WP3a, WP3b, WP4a in development; WP6 developed, in
  review; WP7 developed (its developer returned early, so the fix stage must finish its
  checks) and reviewed; WP4c developed, reviewed, in fixing; items 5 and 6 designed
  (`item5-spec.md`, `item6-spec.md`) and critiqued, being revised.
- 19:45 UTC: items 5 and 6 designed (`item5-spec.md` WP9a-WP9h, `item6-spec.md` WP10a-WP10e),
  owner's answers recorded at the top of each (item 5: Ghidra on Windows, Kuna only on Mac so
  no WP9f, Install button for Apple's tools, off in new chats; item 6: 1A 2A 3A). Run
  wf_a08f0014-543 (script v006-packages-v2.js): group 1 WP9a + WP10c, group 2 WP9b, WP9c,
  WP9g, WP9d, WP9h, group 3 WP9e. Still to start: WP10a, WP10b (after WP1a/WP1b are done),
  then WP10d, WP10e.
- 19:51 UTC check: WP7 done (reviewed, fixed, checked; two minor follow-ups in
  `integration-todo.md`); WP4c done; WP6, WP4a in fixing; WP1a reviewed, WP1b in review,
  WP2 developed; WP3a in review, WP3b and WP5b developed, WP5a in development; WP9a and
  WP10c in development.
- 20:51 UTC check: done: WP7, WP4c, WP4a, WP10c (each reviewed, fixed, checked). WP6 in a
  second fix round; WP1a fixed (being checked), WP1b fixing, WP2 reviewed; WP3a, WP3b, WP5b
  reviewed, WP5a in review; WP4b finishing against the real data; WP9a in development.
  Disk was down to 6.6 GB: old scratch files removed.
- 21:51 UTC check: done: WP1a, WP4a, WP4c, WP7, WP10c. Fixing: WP1b, WP2, WP3a, WP5b, WP4b,
  WP9a; WP3b fixed (being checked next); WP5a reviewed; WP6 second fix round being checked.
  Disk 6.4 GB → 9.3 GB after removing finished reviewers' scratch and superseded test
  binaries in target/debug/deps.
- 22:51 UTC check: done: WP1a? (second fix round running), WP3b, WP4a, WP4b, WP4c, WP7, WP9a,
  WP10c. WP6: round 4 of classifier fixes (wf_5cb207f6-6b1), then known limits written down.
  Checking fixes: WP1b, WP3a. Fixing: WP5a; WP2 and WP5b fixed, checks queued. Developing:
  WP9b, WP9c (then WP9g, WP9d, WP9h, then WP9e). Waiting: WP8 (after WP1b/WP2), WP10a, WP10b,
  WP10d, WP10e (after the models run).
- 23:51 UTC check: WP6 done (four rounds; limits listed in coding_apps.rs). WP1b, WP2,
  WP3b, WP5a, WP5b checked; WP1a final check running (then WP8 starts); WP3a second fix
  round. Item 5: WP9a done, WP9b and WP9c developed (in review next), WP9g and WP9d in
  development. Item 6: run wf_1cc5cb57-517 started (WP10a + WP10b, then WP10d + WP10e).
- 2026-11-13 ~00:20 UTC: the container restarted and stopped four runs. Files, toolchain and
  tools survived; the dev server was restarted. All four runs were resumed with the same
  scripts and arguments (resumeFromRunId): finished agents came back from cache; the six that
  were mid-way (WP8, WP3a fix round 2, WP9g, WP9d, WP10a, WP10b) started again and were told
  their earlier work is in the tree. Lesson: SendMessage to a workflow's agent does not reach
  it; it starts a second copy from its transcript (six copies were started and stopped again).
  Never message workflow agents. If it happens again: commit, restart the dev server,
  resume each run the same way (args are in the session record and this file's run table).
- 00:51 UTC check: in progress after the restart: WP8 (developing), WP3a (fix round 2),
  WP9g and WP9d (developing; WP9b and WP9c developed, reviews after), WP10a and WP10b
  (developing). Disk filled up once (00:40); freed to 11 GB; a disk guard
  (scratchpad/diskguard.py, every 5 min below 8 GB free) now removes target/debug/incremental
  and superseded test binaries.
- 01:55 UTC: done: WP3a, WP3b, WP5a, WP5b (memory+share run finished); WP8 and WP10a/WP10b
  in fixing; WP9b reviewed. To use the idle processors, the item 5 run was split: wf_a08f0014-543
  resumed with WP9a, WP10c, WP9b, WP9c only; new runs (script v006-packages-v3.js, which
  accepts an existing report) for WP9d (wf_b32b9b4f-38c), WP9g (wf_370c5424-374), WP9h
  (wf_0a683d9b-c4b), starting at their reviews (developer reports in the scratchpad,
  dev_WP9*.txt). WP9e (last) is to be started by hand when all five are done.
- 02:30 UTC: WP1a, WP1b, WP2, WP8 done (models run finished). Leftover minors are being fixed
  by run wf_c6b8dca9-052 (Other apps key, helper app-note escaping, WebP to PNG, cancelling a
  model switch, model list spelling for links, folder path escaping). The header layout pass
  (WP2 widths, WP7 chip at narrow widths, item 6 usage line) waits for WP10e.
- 02:51 UTC check: done: WP9d, WP10a, WP10b (plus everything earlier). Fixing: WP9b (being
  checked), WP9c, WP9g, WP9h. Developing: WP10d, WP10e. Leftover fixes run wf_c6b8dca9-052:
  1 of 6 done. Still to start: WP9e (after WP9b-WP9h), then integration.
- 03:51 UTC check: done: WP9b, WP9d, WP9g, WP9h. WP9c fixing; WP10d/WP10e in review; leftover
  fixes 5 of 6 written, in review. An orphaned Xvfb (parent gone after a `timeout` killed its
  xvfb-run) held /tmp/harness-e2e.lock for 38 minutes and blocked three e2e runs; stopped with
  the owner's permission. Use `flock -o` (the lock is not inherited by the children) from now
  on (spec Part 1 and the v3 script updated).
- 04:30 UTC: item 5's six main packages done (reviewed, fixed, checked); WP9e started
  (wf_59ddd95e-968). Item 6: WP10d/WP10e in review. Leftover fixes run wf_c6b8dca9-052 in review.
- 05:10 UTC: item 6 done (WP10a, WP10b, WP10d, WP10e reviewed, fixed, checked). Leftover fixes
  run A done (6 fixes). Running: WP9e (wf_59ddd95e-968); leftover fixes B (header layout, rea
  link text; wf_3b4d80b6-e9b) and C (per-chat model switch cancel, WebP on the proxy and tool
  paths, helper escaping scope, Other apps copy race; wf_6a9d631b-ae5); release docs
  (wf_50b56912-278). Next: whole-release review, version 0.0.6, every check and e2e suite,
  CI on three systems, draft pull request.
- 06:00 UTC: release review started (script v006-release-review.js; runs wf_7ed6132e-21e gate +
  untrusted input, wf_0772ffdb-bab processes/network + tests, wf_b0fd1ffe-7a1 integration +
  portability, wf_16028e05-829 lifecycle + owner/plain words): finders by dimension, dedup,
  a skeptic per finding. Still finishing: WP9e (fix round), header layout (second fix round).
  Docs done (ARCHITECTURE, SECURITY, CHANGELOG, README; TODO(lead) markers for WP9e).
- 06:50 UTC (owner asked to go faster): version set to 0.0.6 and pushed without [skip ci]
  (9e8de10) so CI on all three systems runs early; draft pull request #5 opened and watched.
  Release review findings are fixed as each review run ends: gate/untrusted-input findings in
  wf_97f1aa9f-152 (rea walk refusals naming inner paths, escaped names); integration/
  portability findings in wf_bf302e55-0de (Windows path test, backend check D3, MCP name clash).
  Review runs still going: processes/network + tests (wf_0772ffdb-bab), lifecycle + owner
  (wf_16028e05-829). Disk guard now also removes superseded rlib/rmeta files (8.6 GB freed).
- 06:55 UTC check: CI on 9e8de10 failed only on clippy 1.100's needless_range_loop in
  crates/tools/src/symbols/find.rs (fixed; the installers were skipped, so the Intel Mac smoke
  test had no disk image). A full clippy 1.100 run is going here before the next CI push.
  Release-review fixes running: wf_97f1aa9f-152 (rea walk refusals, escaped names),
  wf_bf302e55-0de (backend check D3, MCP name clash; Windows path test done), wf_57274b94-12f
  (rea start stuck, Settings list scroll, Compare fit words, Tools panel rea note); review
  run wf_0772ffdb-bab (processes/network + tests) verifying; header refix (wf_3b4d80b6-e9b)
  and WP9e's check round (wf_59ddd95e-968) finishing.
- 07:53 UTC check: release review finished (all four runs; findings fixed or in the final
  round). Done since 06:55: WP9e, header layout (two rounds), rea walk refusals, escaped
  names, Windows path test, backend check D3, MCP name clash, rea start stuck, Settings list,
  Tools panel rea note. Final fix round (v006-final-fixes: fix, review, refix of every
  finding, check): batch a wf_bfaa840a-6b8, b wf_55eeb8d4-8e0, c wf_dad2159c-dae (Ghidra pin
  from CI run 43380365172, Linux temp folder, ARCHITECTURE markers), d wf_728c1760-d70, e
  wf_09edf0f5-041; wf_57274b94-12f reviewing its last two. Then the final pass and CI.
- 08:52 UTC check: final fix round (22 items in batches a-g, v006-final-fixes): every fix
  written except rea-start-guard (batch g wf_e649f2ab-5a2, started 08:42) and the ARCHITECTURE
  markers; the rest are in review, refix or check. Next: the final pass (every check, every
  e2e suite with the real model), CI on three systems, owner hand-off.
- 09:53 UTC check: final fix round nearly done: batch g (rea start guard) finished and checked;
  the other batches are on their last refix or check steps. The final-pass script
  (scratchpad v006-final-pass.js: every check and every e2e suite with the real model on
  toolchain 1.100, fixes re-verified) starts when they end.
- 10:45 UTC: final fix round done (22 items, batches a-g, each fixed, reviewed, refixed and
  checked; reports in scratchpad reports/final-*.md). Throwaway-file exclude rules removed
  (no leftovers). Final pass started on toolchain 1.100: wf_f39d4750-b8d (Rust checks and
  tests incl. real llama-server and real rea; full scripted e2e) and wf_56f91c0c-0cc
  (frontend checks, milestone e2e, completeness critic and owner checklist). CI started on
  this commit so Windows and macOS results come in meanwhile.
- 10:53 UTC check: CI run 43409122542 on 4c850e7: Linux lint and test passed (clippy 1.100,
  workspace tests with the llama-server contract tests, cargo-deny), Ghidra digest passed;
  Windows and macOS installer jobs running their workspace tests. Final pass: completeness
  critic done (owner checklist written), its verify running; Rust, scripted e2e, frontend and
  milestone e2e running.
- 11:53 UTC check: first full Windows/macOS CI (run 43409122542) found Windows-only and
  macOS-only test failures (scratchpad ci-logs/; one PR comment each). macOS fixes done
  (fit-params limit 120 s for the first Metal compile; reverse run-temp tests on a short
  base); Windows fixes: three done, managed_e2e stack overflow in review. Final pass still
  running (Rust and scripted e2e; frontend and milestone e2e); completeness check done
  (owner-checklist.md). Next: one push without [skip ci] for CI on all three systems.
- 12:20 UTC: owner asked to finish. Local final-pass runs stopped (CI is the gate: Linux
  passed in full on 4c850e7; every fix had its own tests and checks; the completeness check
  and owner checklist were done). Windows and macOS fixes pushed in fb7f905; CI run
  43422225789 running. PR #5 description updated. When CI is green: hand the owner the
  checklist and the catalog quotes; delete the hourly save task.
- Container set-up done here (needed again in a fresh container): Tauri system libraries
  (ci.yml), webkit2gtk-driver and xdotool, `cargo install tauri-driver --locked`,
  `rustup toolchain install 1.99`, `python3 scripts/fetch_llama.py linux`,
  `python3 scripts/fetch_test_model.py .cache/models`, dev server on port 1420.

- Final pass, completeness check: every work package and owner decision checked against the
  code and tests; version 0.0.6 in every manifest and in `Cargo.lock`; no `TODO(lead)` or
  `TODO(0.0.6)` marker left (the last one, in CHANGELOG, removed); two model names removed
  from the docs; CHANGELOG gained the user-visible changes it lacked (escaped names, the
  header, renamed MCP servers, Settings fixes); `integration-todo.md` ends with the outcome
  of every item; `owner-checklist.md` written; ARCHITECTURE 13.6 notes Windows Error
  Reporting; `e2e/README.md` lists m10, m11 and the 0.0.6 scenarios. Left in the code
  (integration-todo "To fix"): two old-wording error hints, and real model file names in
  some 0.0.6 test data.
- Container set-up: also `rustup toolchain install 1.100` (the final pass's toolchain).

## Next
0.0.6 is complete pending the final pass's results, CI on all three systems (pull request
#5) and the owner's review: README "How to pick it up", then `owner-checklist.md`.
