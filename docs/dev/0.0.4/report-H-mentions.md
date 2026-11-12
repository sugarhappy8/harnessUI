Feature H (file and symbol mentions) is finished and wired end to end. The `mentions` end-to-end scenario passes, and every check in the rules file passes.

Your four answers worked out like this. The D9 test shows a `Part::Attachment` in the user's message already marks the turn untrusted, so there is no finding and I made no change to the loop. I did not touch `outside.rs`. `working_folder_for` is now `pub(crate)` and nothing else in `turns.rs` changed. The `dto.rs` and `ChatView.tsx` edits are as approved. No model tool was added, so `OWN_DATA_TOOLS` does not apply.

The second developer's claim held: 20 passed, 12 ignored. I kept their engine and design with one exception, explained under point 2.

**1. Files**

Created:
- `crates/tools/src/symbols/find/python.rs`, `script.rs` (TypeScript and JavaScript), `go.rs`, `managed.rs` (Java, C#, Kotlin, Swift), `c.rs` (C, C++)
- `crates/tools/src/symbols/resolve/tests.rs`, `crates/tools/src/symbols/index/tests.rs`
- `src-tauri/src/mentions.rs`
- `src/components/MentionPicker.tsx`, `src/components/Composer.test.tsx`

Changed (my files):
- `symbols.rs`: module root; the `allow(dead_code)` and TODO are gone.
- `find.rs`: language dispatch, the brace-rule fix, unused `first_word` removed.
- `find/tests.rs`: all ignored tests un-ignored, one new test, the stale TODO removed.
- `index.rs` and `resolve.rs`: were one-line stubs, now complete.
- `Composer.tsx`, the user-message part of `MessageItem.tsx`.

Small edits:
- `attachments.rs`: added `save_text_in` and `save_text`; `document_refs` now skips parts that have a `mention` key.
- `chat.rs`: `send_message` takes `mentions: Option<Vec<MentionRequest>>`, resolves them before anything is stored, and an empty-text message with mentions is allowed. `message_dto` fills `mentions`.
- Approved edits: `dto.rs` gets `#[serde(default)] pub mentions: Vec<MentionRef>` on `MessageDto`; `turns.rs` makes `working_folder_for` `pub(crate)`; `ChatView.tsx` passes `chatId={detail?.chat.id ?? null}` and the mentions in `onSend`.

Shared registration files:
- `src-tauri/src/lib.rs`: `mod mentions;` plus `mentions::search_mentions` and `mentions::resolve_mentions` after `attachments::get_attachment`.
- `commands.list`: `search_mentions` and `resolve_mentions` at the end.
- `capabilities/main.json`: `allow-search-mentions` and `allow-resolve-mentions` at the end.
- `src/lib/types.ts`: `MentionInput`, `MentionHit`, `MentionSearch`, `MentionRef`, `MentionCheck`, and optional `mentions` on `MessageDto`.
- `src/lib/ipc.ts`: `mentions?` on `SendOptions`, plus `searchMentions` and `resolveMentions`.
- `src/store/app.ts`: `send` takes a fifth parameter, `mentions?`.
- `e2e/scripted_agent.py`: `p_mentions` and its `PROBES` entry.
- `crates/agent/tests/turn_loop.rs`: one test appended at the end (re-read just before the edit).

**2. What it does now, and design choices**

What the user sees:
- Typing `@` opens a list above the message box.
  - With nothing typed after `@`, it shows recently changed files.
  - Otherwise it shows files (fuzzy match on the path) and definitions (name, kind, `file:line`).
  - `path#name` searches definitions inside matching files.
- Arrow keys, Enter or Tab, and the mouse choose an item; Escape closes the list.
- An `@` inside a word, such as an e-mail address, opens nothing.
- Accessibility: the box is a `combobox` with `aria-expanded`, `aria-controls` and `aria-activedescendant`; the list is a `listbox` of `option`s.
- Choosing an item puts `@calc.py#add ` in the text and a chip under the box. The chip can be removed, which also removes its text. Deleting the text drops the chip.
- Only chips whose `@token` is still in the text are sent. A typed or pasted `@name` is plain text.
- Before the box is cleared, the composer calls `resolve_mentions`. If a mention can't be read, the text stays and the reason is shown.
- Sent messages show the mentions as chips, again after a restart; a chip's tooltip shows the line range.

The one engine change, and why: the old rule was "a `}` in column 0 closes every open block". That broke ordinary C++ written without indentation inside a namespace: in the hand-checked fixture, `namespace shop` ended at the class's `};` on line 22 instead of line 49. The fixture was right. New rule: a column-0 `}` closes the open blocks whose opening line was indented, then one block whose opening line starts in column 0. The per-line `{` depth is now taken from the same brace table, so scopes and lines agree. The lexer's drift case (`#ifdef` with two opening lines) gives the same result, and a new test, `a_closing_brace_in_the_first_column_heals_a_drifted_count`, covers both cases. Every other expected line in the ignored tests was correct as written.

What each language recognises:

| Language | Found | Not found |
|---|---|---|
| Rust | `fn`, struct/union/enum/trait, `impl` (the type is the name, the trait is the container), `mod` with a body, const/static, `type` | macro-generated items, `mod x;`, associated types inside a trait or impl |
| TypeScript / JavaScript | `function` (with a body; overload signatures skipped), `class`, `interface`, `type`, `enum`/`const enum`, `namespace`/`module`; exported or top-level `const` arrow or function values, including `memo`/`forwardRef`/`observer`/`lazy`/`debounce`/`throttle` wrappers; class methods, getters and arrow-function properties | object-literal methods, interface members, functions assigned inside expressions |
| Python | `def`, `async def`, `class`; the block ends by indentation (strings and bracket continuations count); methods and nested functions | lambdas, assignments |
| Go | `func`, methods with the receiver type as container, `type` (struct, interface, other), including `type ( … )` groups | `var`/`const` |
| Java | class/interface/`@interface`/enum/record; methods and constructors by shape, only directly inside a type and only with a body | abstract and interface methods without a body, members of enum-constant bodies |
| C# | same as Java plus struct, `namespace`, `record class`/`record struct`, `=> expression;` methods | properties, `operator` methods |
| Kotlin | `fun` (backtick names, receivers, kept even without a body), class/interface/object, `enum class`, `fun interface`, `typealias`; companion object members belong to the enclosing class | properties |
| Swift | `func`, `init`, class/struct/enum/protocol/actor, `extension` (kind `impl`), `typealias`; `class func` is read as a modifier | properties, subscripts, `deinit`, operator functions |
| C / C++ | struct/union/enum/class with a body, `typedef struct {…} name;`, `namespace`, `using X =`, `extern "C" {}`; functions by shape (a type before the name, or a qualified name `A::b`, a constructor or destructor in its class, or the return type alone on the line above), only at file, namespace or class level and only with a body; `operator==` | declarations without a body, macros, function pointers, K&R definitions |

Every definition ends at its matching brace, or by indentation for Python, with a 40-line fallback. `.h` is read as C++.

How a mention is stored (the binding `bytes_path` finding is kept):
- `mentions::attach` resolves every chip first; one failure fails the send before anything is stored.
- Each excerpt is written as `<sha256>.txt` in the app's own `attachments` folder (`save_text_in`), with an attachment row: mime `text/plain`, `extracted_text` = the excerpt, and `bytes_path` always the app's copy, never the user's file.
- Content part: `{"type":"document","attachment_id":N,"name":"calc.py#add","mention":{path,kind,symbol,container,startLine,endLine,truncated}}`.
- `turns::history_of` turns it into `Part::Attachment` with no change to `turns.rs`, so it is sent again after a restart and on Regenerate, and feature I's inspector sees it as an ordinary stored attachment.

What the model receives (the existing attachment wrapper, sanitised). The `File:` line and the `tool="attachment"` marker were checked in the end-to-end request; the wrapper's id lines are written as the earlier handoff describes them:
```
<tool_output id="<16 hex>" tool="attachment" source="calc.py#add">
File: /home/u/project/mentions/calc.py, lines 1-2 (function add)
     1	def add(a, b):
     2	    return a + b
</tool_output id="<16 hex>">
```
- A whole file starts `File: <abs> (N lines)`, followed by lines in `read_file`'s numbered format.
- When cut: `[Lines X-Y of the file were left out (N lines). The file tools can read more: call read_file with offset=X.]`
- The excerpt for a definition starts at its leading comment or attribute lines.
- Cap per mention: the smallest of 8,000 tokens, 20% of the context, and 40% of the context shared by the mentions, at 3 characters per token, never under 600 characters.
- At most 8 mentions per message.

The path sandbox (`resolve.rs`, inside the tools crate so it can use the same functions as `read_file`):
- `PathSandbox::new(Flavor::native(), approved, forbidden, RealFs, volume_case)`, then `check(path, working folder or first root)`, which resolves symlinks before comparing.
- Refused: not existing, a folder, not a regular file.
- Also refused, stricter than `read_file`: inside a skill folder (`skill_guard::skill_root_for`), and any place `always_ask_reason(_, Edit, _)` names (`.git`, `.ssh`, shell start-up files, credential configuration). Over 1 MB and binary files (a NUL in the first 8 KB) are refused too.
- `recheck` runs right before the file is opened, and the read is capped at 1 MB + 1 byte.
- No approval card: choosing a mention is the user's own act, like attaching a file.

The index (`index.rs`):
- One per approved folder, process-wide, shared by chats. A folder inside another approved folder is covered by the outer one.
- Built on a std background thread: names breadth-first, then definitions, published in batches of 400 files, so the window and a running turn are never blocked.
- The search command runs in `spawn_blocking`; its first query waits at most 150 ms for a small folder.
- The walk:
  - never follows links;
  - checks every folder with the sandbox (forbidden folders are skipped, and an approved root that is itself forbidden yields nothing) and with the skill and sensitive checks;
  - skips `.git`, `node_modules`, `target` (as `search_files` does) plus build output, caches, dependency folders, `.ssh` and `.gnupg`;
  - leaves out known binary extensions, files over 1 MB, binary content and `always_ask` files.
- A rebuild runs at most every 3 s when searched, reuses entries whose size and modified time are unchanged, and keeps serving the old list until the new one is complete.
- Memory caps: 20,000 files and 200,000 definitions per folder, 2,000 per file, 4 folders kept (least recently used dropped and its build cancelled). Worst case is roughly 15 MB of definitions per folder (estimated, not measured).
- No new dependencies.

Measured search times, all in the unoptimised debug build (release builds are forbidden), using the ignored test `timing_on_this_repository`:

| Tree | Files | Names listed | Full index | Search time | Rebuild, nothing changed |
|---|---|---|---|---|---|
| This repository | 704 indexable files | 7 ms | 8,929 definitions in 482 ms | 1–7 ms | 7.5 ms |
| Copy of crate sources (scratchpad) | 3,972 | 59 ms | 61,800 definitions in 2.6 s | 5–51 ms | 53 ms |
| All cargo registry sources | 55,300 (capped at 20,000 files and 200,000 definitions) | 230 ms | 18.3 s | 28–204 ms | 367 ms |

- On the 3,972-file tree, a first search returns files as soon as the names are listed, about 60 ms. That meets the 200 ms first-results target in debug.
- The 55,300-file row was measured before I switched per-file definitions from a `Box` to an `Arc` slice, which removed a quadratic copy at each batch publication. I did not re-time that row after the change.
- The repository has only 704 files once `node_modules` and `target` are ignored, which is why the larger trees were also measured.

**3. Tests**

- **Finder:** all 12 ignored tests are un-ignored. Python and TypeScript/JavaScript (with `names_shown_to_the_user`) were un-ignored together; Go, then Java, C#, Kotlin, Swift, then C and C++ one language at a time. Plus the new drift test. Total: 20 finder tests plus 13 lexer tests.
- **Resolver:** 15 tests, including the mandatory sandbox ones: outside the approved folders, `..`, a symlink leaving the folder (file and folder links; a link staying inside works), a forbidden folder, too large, binary, `.git`/`.ssh`/`.bashrc`, a skill folder, a folder or missing file or no approved folder. Others cover file and definition text, cutting and the left-out note, qualified names and the nearest line, display paths, the cap formula, and "whatever a mention reads, `read_file`'s describe has no hard limit for".
- **Index:** 9 tests plus 1 ignored timing test. They cover ignore rules, links never followed, forbidden and skill folders, search, recency, cache reuse, the file cap, sharing, nesting and eviction, and searching while a build runs.
- **App:** 2 tests. A stored mention is a chip and not a document; deleting a chat with a mention does not touch the mentioned file (all paths the store returns are deleted, and the user's file stays intact).
- **Agent:** `an_attachment_in_the_user_message_marks_the_turn_untrusted` checks that the first decision sees untrusted content and the request holds `tool="attachment"`.
- **Frontend:** `Composer.test.tsx` has 10 tests.
- **Seen failing first:** 13 of 15 resolver tests against a stub that refused everything (the 2 pure helper tests passed), and the C++ finder test on its first real run (the namespace extent). The second developer saw the finder tests fail against an empty stub; with my patterns, all but C++ passed on their first run. The index, app, agent and frontend tests were written after the code and passed on their first run.

**4. Checks run** (all with `CARGO_INCREMENTAL=0`; disk 9.8 GB free)
- `cargo fmt --all --check`: clean.
- `cargo +1.99 clippy -p tools|agent|app --all-targets -- -D warnings`: all clean. An earlier run on `tools` failed only at `files.rs:2724` (feature B, mid-edit); it was clean on the later run.
- `cargo +1.99 test -p tools`: 188 passed, 1 ignored (timing), plus 3 (pdf_worker).
- `cargo +1.99 test -p agent`: 151 + 30 + 12 + 1 + 1 + 1 + 49 passed.
- `cargo +1.99 test -p app`: 387 passed.
- `npm run lint`: clean. `npm run typecheck`: clean. `npm test`: 34 files, 234 tests passed.
- `cargo +1.99 build -p app`, then `flock … PROBES=mentions … scripted_agent.py`: PASS.
  - Checks: the picker shows `add function calc.py:1`; Enter puts `@calc.py#add` in the text with a chip.
  - The request holds `File: <abs>/calc.py, lines 1-2 (function add)`, the definition lines (not `sub`) and `tool="attachment"`.
  - The chip is still there after a reload.
  - `resolve_mentions` refuses an absolute path and a `..` path outside the folders; `search_mentions` returns nothing outside; `send_message` with an outside mention fails.
- Not run: `npm run build`, other scenarios.

**5. For ARCHITECTURE.md**

File and symbol mentions (9.3). Typing `@` in the message box searches a background index of the chat's approved folders: file paths by fuzzy match, and definitions found by `tools::symbols::find_symbols`, a line-pattern finder over text whose comments and strings were blanked first (`lex`). It is not a parser. It covers eleven languages and gives each definition a name, kind, container and line range: brace matching, indentation for Python, a 40-line fallback.

The index has one entry per approved folder, shared by chats. It is built on a background thread, names first and then definitions, published in batches. It is bounded: 20,000 files and 200,000 definitions per folder, 4 folders. It is refreshed incrementally by size and modified time. Its walk never follows links, never enters a folder the path sandbox refuses or one mentions refuse, and leaves out what a mention could not read. So the picker never shows a name outside the approved folders.

Only chips the user chose in the picker are resolved. The `@path` text is never parsed back, so a typed or pasted mention, and anything in model output or tool results, reads nothing. Resolution is the user's own act, like attaching a file, so there is no approval card. It goes through exactly `read_file`'s checks: `PathSandbox::check` (symlinks resolved), the app's forbidden folders, and `recheck` before opening. It is stricter in that it also refuses skill folders, `always_ask` places, files over 1 MB and binary files. The excerpt is capped to a share of the context and ends with a note pointing to `read_file`.

The excerpt is stored as a copy in the app's attachments folder, never by reference to the user's file, because a deleted chat's attachment paths are returned for deletion. It becomes a document part of the user message, so the model receives it as an attachment inside the untrusted-data markers. Its presence marks the turn as holding untrusted content (D9) from the first policy decision, and it is sent again after a restart and on Regenerate.

**6. Unverified, and TODOs**
- Only built and run on Linux. Windows and macOS paths (separators in labels, case-insensitive volumes) are compile-level only.
- All timings are from the debug build.
- `resolve_mentions` (the pre-send check) sizes the cap for a 4,096-token context. Its `truncated` value can differ from what the send uses, which sizes for the chat's real context.
- Edit-and-resend drops mentions, as it already drops images and documents (`ChatView.editResend` calls `send(text, parentId)`).
- Index changes are picked up when the folder is searched at least 3 s after the last build; there is no file watching.
- No `TODO(0.0.5)` markers remain in my code.
- The cargo-sources copy used for timing is left in the scratchpad at `/tmp/claude-0/-home-user-harnissUI/23ef2267-cb6f-5325-b082-a4b40b3ccb19/scratchpad/timing-tree` (88 MB); it can be deleted.
