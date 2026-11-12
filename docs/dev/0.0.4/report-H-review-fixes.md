All three review findings are fixed, with tests, and the `mentions` end-to-end scenario still passes. I edited only `crates/tools/src/symbols/resolve.rs`, its `resolve/tests.rs`, and `src-tauri/src/mentions.rs`.

**Finding 3: race between the last check and opening the file**
- **What changed:** a new `open_checked(path, checked_meta)` opens the file without following a link at the end of the path, and the size, kind and content now all come from that one handle:
  - **Unix:** opens with `O_NOFOLLOW | O_NONBLOCK` (the second flag stops a FIFO swapped in from blocking the read). It then compares the handle's device and inode with those of the path the sandbox checked, and refuses a mismatch with "the file changed while it was being read; try again".
  - **Windows:** opens with `FILE_FLAG_OPEN_REPARSE_POINT` and refuses if the handle is a reparse point.
  - **Both:** refuses a handle that is not a regular file, and enforces the 1 MB limit on the handle's size and on the bytes actually read.
  - **Earlier pre-checks:** they now use `symlink_metadata` on the checked path, so they describe the path itself, not a link's target.
- **What still remains open:**
  - **Swapped parent folder:** on Unix, `O_NOFOLLOW` only covers the last part of the path. A parent folder swapped for a link after `recheck` would be followed. The inode comparison catches that case only if the file reached through the link is a different file. If the link leads back to the very file that was checked, the comparison passes, and that is harmless.
  - **Hard links:** a hard link inside the approved folder to a file outside has the same inode, so nothing here can tell it apart. `read_file` and the sandbox have the same gap.
  - **No identity check on Windows:** std's file index is unstable and I added no dependency, so a swapped parent folder or a swap to another regular file is not detected there.

**Finding 4: a single line longer than the cap was cut silently**
- **What changed:** `numbered` now reports when it cut a line. A cut line sets `truncated = true` and adds this note:
  `[Line N is longer than a mention can hold, so only its start is shown. The file tools can read the file: call read_file with offset=N.]`
  The "left out" note is still added when lines after it were dropped. This applies to whole files and to definitions.

**Minor: no window check on the two commands**
- **What changed:** `search_mentions` and `resolve_mentions` now take the calling window and reject any window other than the main one, using the same check as the pinned-instructions commands.

**Tests** (each of the first two I saw fail before the fix):
- `a_single_line_longer_than_the_cap_is_marked_cut_with_a_note`: a one-line file of about 50 KB; asserts `truncated` and the exact note. It failed against the old code.
- `the_open_refuses_a_link_swapped_in_after_the_check` (Unix): drives `open_checked` directly. A link swapped in after the check is refused; another regular file swapped in is refused (inode mismatch); the checked file opens, with its size taken from the handle.
  - It failed against a temporary `open_checked` that did a plain `File::open`.
  - Its first run against the new code also failed, because of the test: it deleted the file first, so the filesystem gave the same inode number to the new file. The test now keeps the original file open.
- `the_commands_are_for_the_main_window_only` (in `mentions.rs`): the grants are in `main.json` and `commands.list` and in neither of the other two windows' capability files (`overlay.json`, `indicator.json`), and the check rejects the overlay, indicator, artifact and empty labels. At first it failed to compile because the check did not exist yet.

**Checks run**
- `cargo fmt --all --check`: clean. An intermediate run showed a diff in `files.rs` (feature B); later clean.
- `cargo +1.99 clippy -p tools -p app --all-targets -- -D warnings`: clean on the third try. The first two tries stopped in other developers' files mid-edit: `crates/agent/src/turn/runner/preview.rs`, then `src-tauri/src/outputs.rs`.
- `cargo +1.99 test -p tools symbols`: 59 passed, 1 ignored (the timing test).
- `cargo +1.99 test -p app mentions`: 3 passed. Before that it was blocked for a while by another developer's mid-edit `context_cmd.rs` and `errors.rs`.
- `cargo clippy -p tools --target x86_64-pc-windows-msvc`: does not build here. It stops in the `libz-sys` build script (a C dependency pulled in through `git2`; there is no MSVC compiler on this machine), not in my code.
  - Instead I copied `open_checked` into a throwaway crate in the scratchpad with no C dependencies. It passes `cargo clippy -- -D warnings` for the Windows target and for Linux.
- `cargo +1.99 build -p app` then the `mentions` end-to-end scenario: PASS.
