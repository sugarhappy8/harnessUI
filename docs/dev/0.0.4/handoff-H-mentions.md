# Handoff: feature H, file and symbol mentions (0.0.4)

Stopped early on the coordinator's instruction. Only the first building block is written.
Everything below marked "decided" is a design I settled on after reading the code; none of
it is implemented unless section (b) says DONE.

## (a) Files created or changed, and what is in them now

| File | State |
|---|---|
| `crates/tools/src/symbols/lex.rs` (new, 771 lines) | Complete and tested. `mask(text, &Rules) -> Masked`: blanks comments and string contents with spaces, keeping every line and column. Per line it returns the masked text, `comment_only`, `in_literal` (line starts inside a multi-line string or comment), `curly` (`{` depth at line start) and `round` (`(`/`[` depth at line start). `Rules` describes one language family; only `Rules::C_LIKE` is defined as a constant, the other families exist as constants inside the test module (`RUST`, `JS`, `PYTHON`) and inline in tests (Go, C#, C/C++). 13 unit tests. |
| `crates/tools/src/symbols.rs` (new, 9 lines) | Module root: a doc comment with `TODO(0.0.5)` and `#[allow(dead_code)] mod lex;`. Nothing public yet. Remove the `allow` once the finder uses `lex`. |
| `crates/tools/src/lib.rs` (shared) | One line added: `pub mod symbols;`. No re-export yet. |

Nothing else was edited by me: no change in `src-tauri/`, `src/`, `e2e/`, `chat.rs`,
`attachments.rs`, `MessageItem.tsx`, `Composer.tsx`, the command lists or capabilities.

Side effect to know about: `cargo fmt --all` was run three times, as instructed, and it
formats the whole workspace. While those commands ran, `src-tauri/src/lib.rs` and
`crates/store/src/schema.rs` changed on disk. That was either rustfmt changing layout or
their owners editing at the same moment; I cannot tell which. Each run also printed
`failed to resolve mod extract: crates/search/src/extract.rs does not exist`, which is the
search agent's unfinished crate, not this feature.

Git: `lex.rs` shows as tracked and modified, `symbols.rs` as untracked, so a commit was
made by someone while only the first draft of `lex.rs` existed. At that commit the file is
not part of the build. I changed no git state.

## (b) Section H, item by item

DONE and tested
- Nothing user-visible. The comment-and-string masking pass (`lex.rs`) that the definition
  finder needs: 13 tests pass.

PARTLY done
- "Index (`crates/tools/src/symbols.rs`, pure, unit-tested per language)": only the masking
  step. What it already handles, each with a test: line and block comments (nested for
  Rust), Rust lifetimes versus character literals, Rust raw strings, Python triple-quoted
  strings and `#`, JS/TS template literals with nested `${ }` holes, JS regular-expression
  literals (only where a value can start, never inside a hole), Go raw strings, C# verbatim
  strings, C++ raw strings, preprocessor lines with `\` continuations, a quote with no
  partner on its line left alone (JSX text such as `Don't {`), non-ASCII text, and a
  self-heal: a `}` in the first column resets the brace count to 0.
  Not written: `Lang` (by file extension), `Symbol`, `find_symbols`, every per-language
  pattern, block-end detection, all per-language tests.

NOT STARTED
- The definition finder for all eleven languages and its tests (found with the right kind
  and line range; calls, comments and strings not matched).
- The folder walk and index: ignore rules, 20,000-file cap, 1 MB per file, background
  build, cache by size and modified time, cancellation, search, and the timing measurement.
- `src-tauri/src/mentions.rs`: `search_mentions`, `resolve_mentions`, registration in
  `generate_handler!`, `commands.list`, `capabilities/main.json`.
- Resolution on send and storage with the message (`chat.rs`, `attachments.rs`).
- The sandbox refusal tests (outside, forbidden, symlink out, missing, directory, binary,
  too large).
- `Composer.tsx` picker and chips, `MentionPicker.tsx`, `Composer.test.tsx`, chips in the
  user part of `MessageItem.tsx`, `ipc.ts`, `types.ts`, `store/app.ts`.
- The e2e scenario `mentions` (no function, nothing in `PROBES`).

No test was seen failing before its code existed: the lexer tests and the lexer were
written together. The first run had 4 failing tests and the second 1. All five wrong
assertions were mine (miscounted spaces in four expected strings, and one `trim_end` that
could not match a line ending in `;`). The masking output was right each time, so I
corrected the expectations and changed no code in `mask`.

## (c) Design decisions to keep (decided, not implemented)

Mention syntax
- In the text: `@<path>` or `@<path>#<symbol>`, for example `@src/lib.rs`,
  `@src/lib.rs#parse_args`. The token is for people and the model to read; it is never
  parsed back. The chip list is the source of truth, and each chip carries the real data.
- `<path>` is relative to the chat's working folder when the file is inside it, otherwise
  `<approved folder name>/<path inside it>`, always with `/`.
- A method is written with its container: `Foo::new` for Rust and C/C++, `Foo.new`
  elsewhere. If a second chip would get the same token, append `:<line>`.
- Only items chosen in the picker become mentions. A typed `@word` with no chip is plain
  text (this avoids e-mail addresses and handles). A chip whose token is deleted from the
  text is dropped; removing a chip removes its token.

Commands and shapes (JavaScript passes camelCase)
- `search_mentions(chatId: number | null, query: string, limit?: number)` returns
  `{ hits: MentionHit[], indexing: boolean, files: number, folders: number }`.
  `MentionHit = { kind: "file" | <symbol kind>, label, path (absolute), symbol?,
  container?, line?, endLine? }`. `chatId: null` means "no chat open yet": global approved
  folders only. A query containing `#` is `path#symbol`. An empty query lists recently
  changed files and no symbols. While `indexing` is true the picker asks again.
- `resolve_mentions(chatId, mentions: MentionInput[])` returns one entry per mention with
  either the metadata (label, kind, start and end line, total lines, truncated) or
  `error`. It does not return file text to the webview. The composer calls it before
  sending so a failure keeps the typed text (`Composer.submit` clears the box at once).
- `MentionInput = { path: string, symbol?: string, container?: string, kind?: string,
  line?: number }`. `path` is untrusted. `line` is only a hint to pick between definitions
  with the same name; the file is read and searched again at send time.
- `send_message` gets one more optional argument `mentions: MentionInput[]`; `SendOptions`
  in `ipc.ts` and `send` in `store/app.ts` get the same optional field. At most 8 per
  message.

How a resolved mention is stored
- As a document attachment, through the existing path: the excerpt text is written to the
  app's own attachments folder as `<sha256>.txt` and a row is inserted with
  `extracted_text: Some(text)`, mime `text/plain`. Add a small `save_text` helper next to
  `save_documents` in `attachments.rs` rather than a second storage path.
- NEVER put the user's own file path into `attachments.bytes_path`. `Store::delete_chat`
  returns the `bytes_path`s that no row refers to any more "so the caller can delete
  them". `chat.rs::delete_chat` ignores that list today, but the day it is used, a mention
  stored by reference would delete the user's file.
- Content part: `{"type":"document","attachment_id":N,"name":"<label>","mention":{"path":
  "<absolute>","symbol":…,"container":…,"kind":…,"line":…,"startLine":…,"endLine":…,
  "truncated":…}}`. `turns.rs::history_of` already turns every `document` part into
  `Part::Attachment { name, text }`, so no change in `turns.rs` is needed and the content
  is there again after a restart and on Regenerate.
- `attachments::document_refs` must skip parts that have a `mention` key, or a mention
  shows twice. `MessageDto` needs a `mentions` field for the chips.

How the model sees it (planned text; the wrapper is the existing one)
- `agent::prompt::project_user` wraps an attachment as
  `<tool_output id="<16 hex>" tool="attachment" source="<name>">` … `</tool_output id="…">`
  and cleans the body with the sanitiser. `source` is `name` cut to 300 characters, with
  `"` turned into `'` and `<>` into `‹›`.
- Planned body, in `read_file`'s own line format (`{:>6}\t{line}`):
  a first line `File: <absolute path> (N lines)` for a whole file,
  `File: <absolute path>, lines A-B (function add)` for a symbol, then the numbered lines,
  then when cut: `[Lines X-Y were left out (N lines). The file tools can read more: call
  read_file with offset=X.]`.
- Cap per mention: the smallest of 8,000 tokens, 20% of the context, and 40% of the context
  divided by the number of mentions, at 3 characters per token as `files.rs` does. Nothing
  in the agent crate cuts attachment text, so the resolver has to.

Sandbox (the part that matters most)
- Resolve in Rust only, with exactly what `FilesServer` does: `PathSandbox::new(
  Flavor::native(), &approved, &forbidden, &RealFs, &|p| volume_case(p))`, then
  `check(path, base, &RealFs)` with `base` = the working folder or the first root, read the
  returned `canonical` path, and call `recheck` immediately before opening.
- Refuse inside skill folders as well. This is stricter than `read_file`, which may read
  them, and it is what the task asked for.
- Refuse: not existing (`CheckedPath.exists == false`), a directory, more than 1 MiB
  (`files::RAW_LIMIT`), binary (a NUL in the first 8,192 bytes, as `files::is_binary`).
- Put the resolver in the tools crate next to `FilesServer`, taking a `&ChatContext`, and
  add a test that anything a mention resolves is something `read_file`'s `describe` gives
  no `hard_limit` for.

Index
- One index per approved folder, in a process-wide map in `mentions.rs` (no change to
  `AppState`). Drop a root that lies inside another root. Walk breadth-first so shallow
  files come first, never follow links, publish in batches so search answers from what is
  there, and let the first query wait up to about 150 ms for a small folder to finish.
- On a rebuild reuse an entry whose path, size and modified time are unchanged; keep
  serving the old list until the new one is complete.
- Bound memory: 20,000 files, and a cap on stored symbols (I planned 200,000 per folder
  and 2,000 per file, and at most 4 folders kept).

Finder
- Run patterns on the masked lines only. Keyword-led definitions at any depth; "type name("
  guesses (Java and C# methods, C/C++ functions) only directly inside a class body or at
  file or namespace scope, never when the line starts inside brackets, with a keyword
  block list, and for C/C++ only when a `{` body follows before a `;`.
- End of a block: scan from just after the name for `{` at bracket depth 0, match it; if the
  rest of the closing line opens another `{`, that one is the body (TypeScript
  `(): { a: number } {`). A `;` first means no body. Python: end of the header by bracket
  depth and `:`, then by indentation, ignoring lines with `in_literal` set or `round > 0`.
  Otherwise 40 lines.

## (d) What I learned that saves time

Attachments
- `send_message` (`chat.rs`) builds `content` as a JSON array: the text part, then
  `save_images`, then `save_documents`. Add mentions there. `target.n_ctx` is already
  resolved at that point, for the cap.
- `save_documents` stores bytes under `<app local data>/attachments/<sha>.<ext>` and the
  text in `attachments.extracted_text`. `store_dir` in `attachments.rs` is private.
- D9: `agent/src/turn/runner.rs` (`Turn::new`) marks the turn untrusted when any message
  of the turn has a `Part::Attachment`. A mention stored as a document gets this for free.
- Edit-and-resend drops images and documents today (`ChatView.editResend` calls
  `send(text, m.parentId)`), so it will drop mentions too unless they are passed along.
- Two files outside the list in the spec need a small edit for this feature, so ask the
  lead: `src-tauri/src/dto.rs` (`MessageDto` is defined there and built only in
  `chat.rs::message_dto`) and `src/components/ChatView.tsx` (the `Composer` mount binds
  `onSend` and has no chat id to give the picker).

Sandbox and context
- The `ChatContext` of a chat is built inline in `turns.rs::run` (about line 1518):
  folders from `state.store.approved_folders_for_chat`, forbidden from
  `crate::permissions::forbidden_for_tools(app)`, the working folder by the private
  `working_folder_for`, skill roots from `toolset::skills_root(app)` plus `~/.agents/skills`
  when the `sharedSkillsFolder` setting is on. `mentions.rs` has to build the same.
- `crates/tools/src/skill_guard.rs` is a private module with `pub(crate)` items; it can be
  used from inside the tools crate only. Its `check` message is about writing, so call
  `skill_root_for` and word the refusal yourself.
- The app's own skills folder is inside the app data folder, which is already forbidden.
- The app crate has no dev-dependencies. `tempfile` and `dunce` are dev-dependencies of the
  tools crate, which is one more reason to keep the resolver and its tests there.

File walk
- `search_files` (`files.rs::search`) skips links, the folders `.git`, `node_modules` and
  `target`, files over 2 MiB and binary files, and stops at 20,000 files. It reads no
  `.gitignore`. The spec for the index adds build output and a 1 MB limit, so the index
  needs its own longer folder list.
- This repository has 546 files outside `node_modules`, `target`, `.git` and `dist`
  (170 `.rs`, 47 `.tsx`, 11 `.ts`), so "a few thousand files" needs a bigger tree for the
  timing measurement.

End-to-end
- `send()` in `e2e/scripted_agent.py` types the text and then U+E007 (Enter).
  `click_button` also matches `[role=option]`, so picker options are clickable by text.
- `send_message` can be called directly with
  `onEvent: "__CHANNEL__:" + window.__TAURI_INTERNALS__.transformCallback(() => {})`.
- `c.work` already holds `notes.txt`; the fake endpoint's requests are in `Fake.requests`.

Lexer details worth knowing before building on it
- The masked line has the same byte length as the original, so a column found in the
  masked text is the same column in the original.
- String delimiters are blanked too: `extern "C" fn` becomes `extern     fn`.
- `curly` and `round` count only code, clamp at 0, and both reset at a `}` in column 0.
- A trailing newline adds no empty last line; an empty text gives no lines.

## (e) Checks run

All from `/home/user/harnissUI`, with `export CARGO_INCREMENTAL=0`; 11 GB free on `/`.

| Command | Result |
|---|---|
| `cargo fmt --all` | Formatted. Printed the `crates/search/src/extract.rs does not exist` error each time (another agent's crate). |
| `rustfmt --edition 2024 --check crates/tools/src/symbols.rs crates/tools/src/symbols/lex.rs` | Exit 0: both new files are formatted. (The 1.99 toolchain has no rustfmt installed; this used the default one, as `cargo fmt` does.) |
| `cargo +1.99 clippy -p tools --all-targets -- -D warnings` | Pass, no warnings (run twice, the second after the last edit). |
| `cargo +1.99 test -p tools --lib symbols` | 13 passed, 0 failed. |
| `cargo +1.99 test -p tools` | 108 passed, 0 failed (unit tests); `pdf_worker` ran; 0 doc tests. |

Not run: any check of the app crate (I did not touch it), `npm run lint`, `npm run
typecheck`, `npm test` (no frontend file touched), `npm run build`, the e2e scenario
(told not to), and the timing measurement (there is no index).
