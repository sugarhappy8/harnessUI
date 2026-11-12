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
  notices, `cargo fmt --check`, clippy `-D warnings`: all pass. (A fresh container needs the
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
- Waiting for the owner's go and answers before the long build.

## Next
README "How to pick it up". The new session's first job: check this state, update the spec,
research items 5 and 6, then report to the owner and wait for their go.
