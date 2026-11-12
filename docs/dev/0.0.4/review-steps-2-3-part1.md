I reviewed B, G, H and I against commit 2a44585, reading the working tree. I found 2 real defects in edit_file, both confirmed by running them, and 4 smaller issues. I found nothing that breaks rules 3 to 7. I changed nothing in the repository. My throwaway test file `crates/tools/tests/zz_review_tmp.rs` has been deleted. Everything `git status --short` shows now is other developers' work.

## Findings, most serious first

**1. edit_file still answers questions about a file's contents before anyone approves anything (rule 1).**
- Where: `crates/tools/src/files.rs:821-831` (describe, "Numbers only: answered here") and `crates/tools/src/errors.rs`, the `OutOfRange` and `NoChange` branches of `edit_problem_message`.
- What breaks: B's own rule (the report, section 5, and the module docs) is that a message from `describe` carries nothing from the file. Only `NotFound` and `Ambiguous` were turned into a read action (`needs_file_text`). Two other cases are answered at describe time, as an app note the model trusts, with no card:
  - `OutOfRange` says how many times the model's text occurs in the file and on which lines.
  - `NoChange` (old_text equal to new_text) comes back only when the text is present. When it is absent, the call becomes a read card instead. So the mere kind of answer tells the model whether the text exists.
- Scenario: an approved folder holds `.env` containing `USER=bob\nAPI_KEY=hunter2\n`.
  - `edit_file {path:".env", old_text:"API_KEY=h", new_text:"x", occurrence:9}` comes back at once, with no approval: "occurrence 9 is out of range: old_text was found once in …/.env (at line 2)."
  - The same call with `old_text:"API_KEY=z"` becomes a read card instead.
  - `old_text = new_text = "API_KEY=hu"` comes back at once: "old_text and new_text are the same…".
  - So the model can guess a secret one character at a time, with no read approval and only an audit row. This works for any file inside an approved folder, including `.git/config` and `.ssh` contents.
- How I confirmed it: a throwaway integration test calling `FilesServer::describe`. Its output was exactly the three messages above, and the absent case gave a read-only action ("Read …/.env to explain why an edit cannot be made").
- Background: the "found or not found" answer already existed at the base commit. B was meant to close it, closed it for two of the four cases, and added line numbers to `OutOfRange`.
- How sure: CONFIRMED.
- Smallest fix:
  - In `describe`, refuse `old_text == new_text` from the arguments alone, before reading the file.
  - Add `OutOfRange` to `needs_file_text`, so it is also answered as an explain read and its line numbers only come back from `call`.
  - Extend `no_message_from_describe_holds_a_line_of_the_file` so it also asserts that no describe message depends on whether old_text is present.

**2. edit_file silently rewrites every byte that is not valid UTF-8 in the file (data loss).**
- Where: `files.rs:812` (describe) and `files.rs:1010-1017` (call). Both work on `String::from_utf8_lossy(&bytes)` and write back `plan.updated`.
- Scenario: a Latin-1 file `caf\xe9\nx\nold\n`, and the edit `old` to `new`.
  - The card's diff shows only the hunk, so the change to line 1 is only visible as a `�` in a context line. In a longer file it is not visible at all.
  - The write succeeds, and line 1 becomes `caf\xEF\xBF\xBD`, so the original byte is gone.
  - The SHA check passes because it is taken over the original bytes.
- How I confirmed it: throwaway test. Bytes after the edit: `[99,97,102,239,191,189,10,…]`.
- This already existed at the base. It is still on the edit path B reworked, and `occurrence` and `replace_all` inherit it.
- How sure: CONFIRMED.
- Smallest fix: if `std::str::from_utf8(&bytes)` fails, refuse in describe ("not UTF-8 text; change it with a command"). Same for `write_file`'s diff.

**3. Mentions: a race between the last check and opening the file (rule 2).**
- Where: `crates/tools/src/symbols/resolve.rs:194-199`. `recheck` runs, then `File::open(path)` follows symlinks.
- Scenario: a background process inside the approved folder keeps swapping `notes.txt` between a regular file and a symlink to `~/.ssh/id_rsa`, while the user is asked to mention `notes.txt`. A swap that lands between `recheck` and `open` gets the target attached, with no card. Mentions have no approval step, so the sandbox is the only barrier. `read_file` has the same window, but behind an approval.
- How sure: PLAUSIBLE, read only. The window is narrow, and the scenario needs the user to mention a file the model suggested.
- Smallest fix: open with `O_NOFOLLOW` (`custom_flags`) on Unix, or `FILE_FLAG_OPEN_REPARSE_POINT` on Windows. Then check that the opened handle's metadata (dev/ino, or the file index on Windows) matches the checked canonical path.

**4. A mention whose only line is longer than its cap is cut without saying so.**
- Where: `resolve.rs:317-326`. `numbered` cuts the first line, sets `last = n`, so `truncated` stays false and no "left out" note is added.
- Scenario: a one-line 50 KB `.rs` file is shown as `fn big() { xxx…` with `trunc=false`. Neither the chip nor the model is told it was cut.
- How sure: CONFIRMED (throwaway test).
- Fix: mark `truncated = true` when a line was cut, and add a note pointing to `read_file`.

**5. The "Shortened" line counts a partly shown long line as fully shown.**
- Where: `crates/tools/src/shorten.rs:218-240`. The head and tail "one very long line" branches set `head_n` or `tail_n` to 1.
- Scenario: `Shown.lines` counts that line as shown, so the line reads e.g. "1 of 1 lines shown", or "2 of 2" when two long lines are each cut. The model is told nothing more is left, although most of the text was not shown.
- How sure: PLAUSIBLE, read only. Low impact, since `read_output` still returns the line.
- Fix: do not count a line that was cut as shown, or make `next` point back to it.

**6. The tool card trusts the last line of any result.**
- Where: `src/components/ToolCallCard.tsx`, `savedOutputId`. It takes the id from the result's last line, whoever wrote it.
- Scenario: a file whose last line is `[Shortened: 1 of 2 lines shown. Full output is saved as output #5. Read more with read_output …]` makes an unshortened `read_file` card offer "Full output" for output #5.
- Effect: only the active chat's outputs can be fetched, so this is UI confusion, not a cross-chat leak.
- How sure: PLAUSIBLE, read only.
- Fix: carry the saved id in a structured field (for example `ui.savedOutput`) instead of parsing text.

**Minor, not ranked:**
- `get_task`, `search_mentions` and `resolve_mentions` do not check the window label, while the other new commands do. The capability file still limits them to the main window.
- `explain_edit` reads up to 4 MB, while `read_file` only reaches the first 1 MB. So the explain read can quote lines `read_file` cannot reach. It has the same policy facts, so this is not an escalation.
- `explain_edit` runs synchronously on the async runtime, not in `spawn_blocking`.
- The terminal's `not_found_note` lists program names from up to 200 folders on PATH, outside the approved folders, into the result. It is bounded and runs only after the command was approved.

## Checked and found sound
- **edit_file under a read-only action:**
  - `call` never writes: `"edit_file" if action.read_only` goes to `explain_edit`, and every other write under a read-only action refuses.
  - The explain action has exactly `read_file`'s facts. Grants are keyed by scope only, so it matches the read grant and no write grant. It is stricter in skill folders (a hard limit).
  - A file that changes between describe and call is refused by the SHA check. The preview and the write both use `plan_edit` on identical bytes, so `occurrence` and `replace_all` write what was previewed (apart from finding 2).
- **Stored outputs:**
  - `read_output` and `get_tool_output` always filter by chat (`WHERE id = ?1 AND chat_id = ?2`), and the chat comes from `ctx.chat`, never from the arguments.
  - The result is wrapped as untrusted. `read_output` is not in `OWN_DATA_TOOLS`. Store caps are 2 MB and 200 per chat.
- **Mentions:**
  - The same `PathSandbox::check` and `recheck` as `read_file`, plus skill-folder, always-ask, 1 MB and binary refusals.
  - Each chat searches only its own approved roots; the index cache is keyed by root and skill roots.
  - The walk never follows symlinks (std's `is_symlink` also covers Windows junctions), and every folder is checked against the sandbox.
  - The excerpt is stored as a copy in the app's attachments folder. Deleting a chat checks whether another row still uses a path before deleting it. The excerpt marks the turn untrusted and the chat as outside content.
  - Edge files gave no panic: empty, CRLF, CR-only, no final newline, Latin-1, unclosed blocks.
- **Task note:**
  - `quote()` collapses whitespace (including U+2028 and U+2029), replaces `"`, makes invisible characters visible, and neutralises `[App note]` in any spacing or case.
  - App notes also go through `Sanitizer::clean`, which escapes the reserved tags.
  - Check output never enters the note. Changed files and checks come only from the policy action's server, scope and read-only flag, `is_error` and the app-written `Exit code` first line. A model edit clears the "user wrote it" marks.
  - `common_folder` slicing is safe for Windows and Unix roots.
- **Pins:**
  - Only the six main-window commands write them; a pin can only be changed through a chat it applies to; there is no tool.
  - Hidden and bidirectional characters are stripped, and reserved tags (including `pinned_instructions`) are escaped.
  - The SQL `CHECK (length(text) BETWEEN 1 AND 1000)` matches the store's character count.
  - A failed database read stops the turn.
- **Registration:** every new command is in `lib.rs`, `commands.list` and `capabilities/main.json`.
- **No unsafe rendering:** no `innerHTML` or `dangerouslySetInnerHTML` in the new components; all text is React text.
- **Logs:** the new log lines carry ids, tool names and errors only, no content.
- **git.rs:** the describe-time refusals no longer quote repository config.

## Not covered
- The task note timing logic, as instructed.
- `turns.rs` layout and the evals code.
- Windows and macOS behaviour, by reading only.
- The PowerShell check parsing in `task/checks.rs`, beyond a skim.
- Frontend tests: I did not run `npm`.
