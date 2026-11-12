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
