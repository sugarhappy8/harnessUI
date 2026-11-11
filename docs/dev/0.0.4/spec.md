# harnessUI 0.0.4: shared spec for all feature agents

Repository: `/home/user/harnissUI` (Tauri 2 desktop app; Rust workspace in `crates/*` and
`src-tauri/`; React + TypeScript frontend in `src/`). It runs local AI models with tools.
Design and security model: `ARCHITECTURE.md` (8 agent loop, 9 tools, 13 security; decisions
D1 to D17 in section 1). History: `CHANGELOG.md`. The owner is a non-technical person
testing on Windows 11 with a Radeon RX 9070 XT and local GGUF models of about 9B.

0.0.4 adds thirteen features, built by several agents at once in ONE working tree. Read
part 1 (rules) and part 2 (shared decisions) fully, then your own section in part 3.

---------------------------------------------------------------------------------------

## Part 1. Rules (all agents)

### Working together in one tree
- Other agents are editing other files in this same working tree right now. Edit only the
  files your section says you own, plus the **shared registration files** below.
- Shared registration files (several agents touch them): `src-tauri/src/lib.rs` (one
  `mod x;` line, entries in `generate_handler!`), `src-tauri/commands.list`,
  `src-tauri/capabilities/main.json`, `src-tauri/src/toolset.rs` (one `host.add_builtin`
  line in `build_host`), `src-tauri/src/turns.rs` (ONLY at a marked `[0.0.4 <feature>]`
  point that names your feature), `src/lib/ipc.ts`, `src/lib/types.ts`, `src/store/app.ts`,
  `src/components/toolcards.tsx`, `src/components/SettingsDialog.tsx` (one mount of your
  own component), `crates/store/src/schema.rs` (ONLY your marked block inside `V2`),
  `crates/store/src/lib.rs` (one `mod` line and re-exports), root `Cargo.toml`
  (`[workspace.dependencies]`, members), `package.json`, `e2e/scripted_agent.py` (your
  own scenario function and one entry in `PROBES`).
  For these files: read the file again immediately before each edit; make one small,
  additive edit with the Edit tool; never reformat, reorder or rewrite; never run a
  formatter that touches other people's lines except `cargo fmt --all`.
- Put your code in NEW files you own wherever you can (a module per feature, a component
  per feature). DTOs go in your own module, not in `dto.rs`.
- Never change git state: no commit, push, stash, checkout, reset, restore, clean. Never
  run `cargo clean` or delete `target/`.
- If a build or typecheck fails in a file you do not own, someone is mid-edit: wait 60 to
  90 seconds and retry, up to 5 times, before reporting it. Do not "fix" other agents'
  files.

### Building and testing
- Before every cargo command: `export CARGO_INCREMENTAL=0`. Check `df -h /`; under 2.5 GB
  free, stop building and report. Never build with `--release`.
- CI uses Rust 1.99: `cargo +1.99 clippy -p <crate> --all-targets -- -D warnings` and
  `cargo +1.99 test -p <crate>`. Format with `cargo fmt --all`. The app crate is `app`
  (`src-tauri`). Cargo commands wait on a shared build lock while other agents build:
  give Bash a timeout of 600000 ms for cargo.
- TypeScript: do NOT run prettier. Match the surrounding style by hand. Run
  `npm run lint`, `npm run typecheck`, `npm test`. (`npm run build` only at the end.)
- Tests are part of the work: behaviour needs tests; write the test first and watch it
  fail where practical, and say in your report which tests you saw fail first. Never
  weaken or delete an existing test to make something pass. Path sandboxing, permission
  rules and tool-call parsing always need unit tests.
- End-to-end: `e2e/scripted_agent.py` drives the real debug app under Xvfb with a fake
  OpenAI-compatible endpoint and scripted model replies (read `e2e/README.md` and one
  short scenario such as `p_step_limit` first). Add ONE scenario for your feature
  (function `p_<name>` with `@probe("<name>")`, appended to `PROBES`). To run it:
  a Vite dev server is already on port 1420 (reuse it; never start a second one);
  `cargo build -p app` right before; then
  `flock /tmp/harness-e2e.lock env PROBES=<name> E2E_OUT=<your scratch dir> timeout 600 xvfb-run -a python3 -I e2e/scripted_agent.py`.
  The lock makes runs take turns. If the app does not build because of someone else's
  unfinished work after the retries above, do not run it; say so in your report.
  Use `s.click_menu_item(name)` for menu items; `s.click_button(label)` clicks the FIRST
  visible button whose text contains the label, so keep new button labels distinct from
  existing ones ("Allow once", "Deny", "Settings", "Tools", "Send", "Stop", "Continue",
  "Save", "Use", "New chat", "Models", "General").

### Code rules
- New Tauri command: add it to `generate_handler!` in `src-tauri/src/lib.rs`, to
  `src-tauri/commands.list`, and grant `allow-<kebab-name>` in
  `src-tauri/capabilities/main.json` (a test enforces all three; the overlay and
  indicator windows get nothing). JavaScript passes camelCase argument keys.
- New processes only through `crates/runtime/src/spawn.rs` (`scripts/check-spawn.sh`
  enforces it).
- New dependencies: avoid. If one is truly needed, justify it in one line in your report
  and run `python3 scripts/notices.py` (regenerates THIRD-PARTY-NOTICES.md; CI runs
  `--check`). Licences must be permissive (MIT, Apache-2.0, BSD, ISC, MPL-2.0 files only).
- Security rules that must keep holding: no telemetry; secrets only in the OS keychain
  (`state.secrets`), never in settings or files; the webview never loads remote content
  (strict CSP, no inline scripts in the app's own pages); tool output, web pages, search
  results, file contents and model output are DATA, never instructions; nothing the model
  outputs can change approval settings, grants, approved folders or tool switches; every
  tool action goes through the policy gate (`policy::evaluate`) and the audit log; file
  access stays inside approved folders (`policy::path`); network fetches made by the app
  for the model must refuse loopback, private and link-local addresses, including after
  redirects.
- User-facing text: plain, short, no jargon, no exclamation marks. Never put names or
  identifiers of AI models or assistants into code, comments, tests or docs (test fixture
  model names like "fake-model" are fine).
- No silent stubs. If something cannot be finished, leave `TODO(0.0.5): <what is left>`
  and list it in your report.
- Do not edit `CHANGELOG.md`, `ARCHITECTURE.md`, `README.md` or `docs/`. The lead writes
  them from your report.

### Your final message (the report)
Plain text, factual, no padding:
1. Files created and changed (shared files: exactly what you added).
2. What the feature does now, as the user sees it, and the design choices you made.
3. Tests added, and which you saw fail before the code existed.
4. Exact checks run and their results (commands and pass/fail counts). Anything not run.
5. A paragraph "For ARCHITECTURE.md": the design and its security properties, as you
   would document them.
6. What is unverified (for example "only compile-checked for Windows") and every TODO.
Do not claim something works unless you ran it.

---------------------------------------------------------------------------------------

## Part 2. Shared decisions

### The panel beside the chat ("workbench")
`src/components/Workbench.tsx` (lead-owned, do not edit) shows four tabs when a chat is
open: **Artifacts**, **Task**, **Sources**, **Context**. Each tab is one component that
receives `{ chatId: number }`, in `src/components/workbench/<Name>Tab.tsx`, owned by its
feature (replace the placeholder that is there). Open the panel on a tab from anywhere
with `useApp.getState().openWorkbench("artifacts" | "task" | "sources" | "context")`.
Styling: Tailwind with the theme tokens in `src/styles.css` (`bg-surface`, `bg-bg-subtle`,
`bg-bg-muted`, `border-border`, `text-fg-muted`, `text-accent`, `btn-primary`, `enter`,
`rounded-xl`, `shadow-soft`). Look at `src/components/Welcome.tsx` and
`ToolCallCard.tsx` for the current style. Icons: `lucide-react`.

### Tool cards
`src/components/toolcards.tsx` holds `TOOL_CARD_EXTRAS: Record<toolName, (call) => ReactNode>`.
A feature adds one entry per tool of its own to show something under the card's header
(an "Open" button, a list of sources, a link to a full log). `call` is a `ToolCallView`
(`name`, `arguments`, `status`, `text` = the result text the model saw). Put the
component in your own file and add only the registry lines to `toolcards.tsx`.

### Built-in tool servers and tool names (fixed; other features rely on them)
A built-in server implements `mcp_host::BuiltinServer` (`crates/mcp-host/src/builtin.rs`:
`name()`, `tools() -> Vec<ToolDef>`, `describe(...) -> Described` with a `policy::Action`,
`call(...) -> ToolOutput`). Look at `src-tauri/src/subagent.rs` (`SubagentsServer`) and
`crates/tools/src/files.rs` for the pattern. Register with one line in
`toolset.rs::build_host`. A server that needs the database takes an `AppHandle` and gets
`app.state::<AppState>()` when called.

| Server | Tools (arguments) | Owner |
|---|---|---|
| `outputs` | `read_output {id: integer, lines?: "A-B", grep?: string, context?: integer}` | Outputs |
| `artifacts` | `create_artifact {title, kind: "markdown"\|"html"\|"svg"\|"mermaid", content}`, `update_artifact {id, content}` or `{id, old_text, new_text}`, `read_artifact {id, version?}`, `list_artifacts {}` | Artifacts |
| `search` | `web_search {query, library?: string, max_results?: integer}`, `read_source {n: integer, query?: string}` | Search |
| `task` | `update_task {goal?, constraints?: string[], steps?: [{text, status: "todo"\|"doing"\|"done"}], next_action?, status?: "active"\|"done", note?}` | Task |
| `toolfinder` | `find_tools {query}`, `call_tool {name, arguments}` | Tool selection |

Every new tool needs a `describe` that returns a correct `policy::Action` (server, tool,
one-line summary, preview, scope, risk, `read_only`, `outbound`). Tools that only read or
write the app's own database for the current chat use **`policy::Scope::AppData`** with
`Risk::Normal` and `outbound: false`: the engine never prompts for it (it exists already,
with a test in `crates/policy/src/engine.rs`), and the audit records such calls as "no
approval needed".

**Companion tools.** `find_tools`, `call_tool`, `read_output` and `update_task` are offered
in a chat automatically whenever at least one other tool is on in it; they are not user
toggles. `artifacts` and `search` are ordinary groups, on in new chats by default. The
tool selection feature implements this rule in `mcp_cmd.rs`; the other features only
register their servers.

### Hook points already in `src-tauri/src/turns.rs`
- `AppTools::execute`: after `host.call`, `crate::outputs::after_call(&app, chat_id,
  &call_id, name, &mut out)` and `crate::task::after_call(&app, chat_id, name, args,
  action, &out)` are already called. Their modules (`outputs.rs`, `task.rs`) are no-ops
  that their owners fill in. `AppTools` has `app: AppHandle`, `hidden: Vec<RegisteredTool>`
  (tools on in the chat but not shown this turn) and `shared` (call in progress).
- `run`: `crate::tool_select::for_turn(&state, &chat, &history, enabled)` returns
  `(shown, hidden)`; today it returns everything as shown.
- `AppPolicy::decide`: a marked point for the chat's approval mode.
Owners may change the signature of their own entry point (they own both the module and the
one call line).

### Database (schema version 2, `crates/store/src/schema.rs`)
Tables exist already: `artifacts`, `artifact_versions`, `sources`, `tasks`,
`pinned_instructions`, `tool_outputs`. Each block is marked with its owner in brackets;
the owner may change its own block while 0.0.4 is unreleased. Store code goes in a new
file `crates/store/src/<feature>.rs` with `impl Store { pub async fn ... }` methods
built on `self.call(move |conn| ...)` (see `crates/store/src/config.rs` for the pattern)
and unit tests against an in-memory store (see the tests in `crates/store/src/chats.rs`).
Small per-feature settings go in the `settings` key-value table
(`state.store.get_setting / set_setting`). The generic `set_setting` Tauri command only
accepts a fixed allow-list of plain UI preferences (`models_cmd.rs`); anything with a
policy meaning gets its own command.

### Approval modes (owner's decision, 0.0.4)
A chat can be in **Allow all** mode (see section A). Other features must not add their own
ways around the gate: every tool goes through `describe` and the engine.

### Things the lead does (not you)
Version bump to 0.0.4, CHANGELOG, ARCHITECTURE, README, final integration, the full
end-to-end run, commits and pushes.

---------------------------------------------------------------------------------------

## Part 3. Features

### A. Allow all (owner item 3)
Owner: "an allow all button so it doesn't ask for approval all the time (deleting any type
of file or files still needs approval no matter what, but not code changes or file
editing)".

**Behaviour.** A chat has an approval mode: *Ask* (today's behaviour) or *Allow all*.
In Allow all the gate answers the card itself, except for what is listed under "still
asks". The mode is per chat, kept in memory like grants (D12: ends when the app quits or
the user switches it off). A setting in Settings → Permissions, off by default, starts
every chat in Allow all ("Allow all in every chat"); switching it on shows a confirmation
that lists what still asks. Only the user can change the mode: a dedicated Tauri command
from the main window, never a tool, never `set_setting`.

**Still asks in Allow all** (card with Allow once / Deny, the reason stated on the card):
1. Deleting: `delete_file`; any delete command (`rm`, `unlink`, `rmdir`, `shred`, `del`,
   `erase`, `rd`, `Remove-Item` and aliases, `git rm`, `git clean`, `find -delete`),
   whatever its flags; replacing a file with an empty one.
2. Discarding or destroying work or the system: `git reset --hard`, `git push --force`,
   disk and system commands (`format`, `diskpart`, `mkfs`, `dd`, `shutdown`, registry
   edits), elevation (`sudo`, `runas`, `Start-Process -Verb RunAs`).
3. A command the classifier cannot fully break down (redirections, variables, command
   substitution, subshells, `eval`, `Invoke-Expression`, `-EncodedCommand`, wrappers,
   inline interpreter code) IF its text contains a delete or destroy word from lists 1
   and 2 or a file-removal call of a common language (`os.remove`, `os.unlink`,
   `shutil.rmtree`, `fs.rm`, `fs.unlink`, `rimraf`, `File.Delete`, `Directory.Delete`,
   `unlink(`, `rmtree`). Without such a word it is allowed. Scan the raw command text,
   case-insensitively, on word boundaries; a unit test table must cover hits and
   near-misses (`format` as an argument value is a hit only as a program name where the
   parser can tell; when it cannot tell, it is a hit).
4. Sensitive writes that already always ask: under a `.git` folder, shell start-up files,
   git and ssh configuration, autostart locations.
5. Web actions that spend money, destroy or publish: a button or submission whose label
   is one of pay, buy, purchase, order, place order, checkout, transfer, delete, remove,
   send, post, publish. Ordinary form submissions, typing then pressing Enter, and
   "submit", "confirm", "search", "next", "log in", "sign up" do not ask.
6. Computer use: input to terminals and shell hosts, system settings and password
   managers (D7's never-covered list). Input to this app stays refused.
7. External MCP tools their server marks destructive, or the user marked always-ask.
8. After untrusted content entered the turn (D9): browser navigation to another site,
   browser typing and submissions, external MCP calls, `git clone / pull / push`, and
   terminal commands that name a network program (`curl`, `wget`, `Invoke-WebRequest`,
   `iwr`, `irm`, `Invoke-RestMethod`, `nc`, `ncat`, `ssh`, `scp`, `sftp`, `ftp`, `telnet`,
   `rsync`, `Start-BitsTransfer`, `bitsadmin`, `certutil`, `gh`, `aws`, `az`, `gcloud`,
   `npm publish`, `git push/fetch/pull/clone`) or that the classifier cannot fully break
   down. Plain local commands (build, test, run a script file) are allowed.
Hard limits (outside approved folders, blocked addresses) stay denials, as today.
Publishing with git also keeps asking: the `git_push` tool and any `git push` command
(it belongs with rule 5: it publishes in the user's name).
Everything else is allowed without a card, including: file reads, writes and edits in
approved folders; plain commands that the classifier can break down and that are not on
the lists above; `git_commit`, `git_status`, `git_diff`, `git_log`; `git_pull` and
`git_clone` when no untrusted content has entered the turn; browser reading, navigation
and ordinary clicks; screenshots; ordinary computer-use input to ordinary applications.
Calls recovered from text are treated like native calls in Allow all.

**Engine.** Extend `crates/policy`: `TurnContext { allow_all: bool, .. }`; a new
`Decision::AllowedByMode`; and on `Action` a new field that says whether the action keeps
asking in Allow all and why, e.g. `keep_asking: Option<KeepAsking>` with variants
`Delete`, `Destroy`, `Sensitive`, `Publish` (pay/buy/send/post/push), `ProtectedApp`,
`MarkedDestructive`, plus `network: bool` for rule 8 (true when the action can send data
out; for commands: names a network program or cannot be broken down). Describers set
them: files (`crates/tools/src/files.rs` + `crates/policy/src/files.rs`), terminal
(`crates/policy/src/command.rs`, `crates/tools/src/terminal.rs`), git
(`crates/tools/src/git.rs`), browser (`crates/browser`: where the action for a click or
submission is described; the consequential-verb list is in `intent.rs`), computer
(`crates/computer`), external tools (`crates/mcp-host/src/describe.rs`). Keep
`Risk::AlwaysAsk` meaning what it means today in Ask mode; Allow all consults the new
fields. `evaluate` order in Allow all: steps 1 and 2 unchanged; then keep-asking → Ask
(OnceOnly, a new `AskReason::StillAsksInAllowAll`); then untrusted + `network` → Ask (as
D9); then `AllowedByMode`. `Scope::AppData` (the app's own data for this chat) already
exists and never prompts in any mode; keep that true in Allow all too.
Every existing engine test must still pass unchanged (Ask mode is the default).
Add a table-driven test for Allow all that covers every row above, allowed and asked.

**App.** State for the per-chat mode (next to `Grants` in `turns.rs`/a new
`src-tauri/src/approvals.rs`), read by `AppPolicy::decide` on every decision (so a switch
reaches a running turn, like revoked grants do; there is a test for grants to copy).
Audit: decisions by mode are recorded as "allowed by Allow all" (`decision_name` in
`turns.rs`, the audit enum in `crates/store/src/audit.rs`, the audit view in Settings).
Subagent helpers inherit the chat's mode. Commands: `get_approval_mode(chat_id)`,
`set_approval_mode(chat_id, allow_all)`, `get_allow_all_default()`,
`set_allow_all_default(on)`.

**UI.** (1) In the chat header (`ChatView.tsx`, next to the Tools button; add only your
button): a shield button; in Ask mode it reads "Ask first" (muted), in Allow all it reads
"Allow all" in the warning colour so the mode is always visible. Clicking it in Ask mode
opens a small confirmation (Radix AlertDialog, as elsewhere) that says in plain words
what will no longer ask and lists what still asks, with "Allow all in this chat" and
"Cancel". Clicking it in Allow all switches back at once. (2) On every approval card
(`ApprovalCard.tsx`) that is grantable: a third choice "Allow all in this chat" that
opens the same confirmation and, when confirmed, approves this card and switches the
mode. Keep the card's safety rules (no default focus on Allow, input delay). (3) Settings
→ Permissions (`PermissionsSettings.tsx`): the default switch with its confirmation.
Labels must not contain "Allow once" or "Deny".

**Files you own.** `crates/policy/**`; `src-tauri/src/approvals.rs` (new);
`src-tauri/src/permissions.rs`; `src/components/ApprovalCard.tsx`,
`ApprovalCard.test.tsx`, `PermissionsSettings.tsx`, a new `AllowAllButton.tsx`; the
describe/classify functions (only those) in `crates/tools/src/files.rs`, `terminal.rs`,
`git.rs`, in `crates/browser/src/server.rs` / `intent.rs` / `approval.rs`, in
`crates/computer/src/server.rs` / `guard.rs`, and `crates/mcp-host/src/describe.rs`.
Another agent (Outputs) edits error messages and result shaping in `files.rs` and
`terminal.rs` at the same time: touch only describe and classification code there, with
small edits.
**e2e scenario** `allow-all`: in Ask mode a `write_file` call shows a card; switch the
chat to Allow all through the header button; a `write_file` and an `edit_file` call run
with no card and the audit shows "allowed by Allow all"; a `delete_file` call still shows
a card; switching back makes `write_file` ask again.

### B. Helpful tool errors and manageable tool output (owner items 7 and 8)
**Errors (7).** Go through every built-in tool in `crates/tools` (files, terminal, git,
skills) and make each failure say exactly why and what to do next, in one short
paragraph the model can act on. Required cases, each with a unit test on the exact text:
- `edit_file`, text to replace found more than once: list each match as `line N: <the
  line, trimmed to 120 chars>` (up to 10), and say how to pick one: include more
  surrounding lines in the text to replace, or pass the new optional argument
  `occurrence: <1-based index>` or `replace_all: true` (add both arguments; schema,
  describe preview and execution must agree).
- `edit_file`, text not found: say whether it differs only by whitespace or indentation,
  line endings (CRLF vs LF) or letter case (try those comparisons); show the closest
  lines in the file (best 3 by similarity, with line numbers) so the model can copy the
  exact text; remind it to read the file first if it has not.
- File not found: list up to 5 similarly named files in that folder and its parent
  (case-insensitive and small edit distance), and the working folder.
- Path outside approved folders: keep the hard limit; say which folders are approved and
  that the user can approve another in the Tools panel.
- `write_file` to an existing file that changed since it was read (if the tool tracks a
  hash), binary files, too-large files, directories given where a file is expected, and
  the reverse: say which and what to call instead.
- `run_command`: a non-zero exit shows the exit code, the last lines of stderr and
  stdout, and the first lines that look like errors (regex on error|failed|panic|
  exception|traceback|cannot find|not found|undefined|denied, case-insensitive, up to 5
  lines with their line numbers in the full output); "command not found" names the
  program and says what is on PATH that is close; a timeout says how long it ran, what
  it printed last, and how to run it with a longer limit if the tool has one; a working
  folder that is not allowed names the allowed ones.
- git tools: not a repository, nothing to commit, merge conflicts (list the files),
  authentication failure (say that credentials are the user's to set up), detached head.
Never put file contents into an error beyond the lines above (they are capped), and keep
every string sanitised the way existing results are (`policy::sanitize`).

**Output (8).** Tools already size what the model sees to `chat.result_cap_tokens` and
return the complete text in `ToolOutput.full_text` when they shortened it. Build on that:
- `src-tauri/src/outputs.rs::after_call` (already called after every call): when
  `out.full_text` is set (the model saw less than everything), store the full text in
  `tool_outputs` (store module `crates/store/src/outputs.rs`; cap one stored output at
  2 MB, keeping head and tail with a marker in the middle; keep at most the 200 newest
  per chat) and append one line to what the model sees:
  `[Shortened: N of M lines shown. Full output is saved as output #ID. Read more with read_output {"id": ID, "lines": "A-B"} or {"id": ID, "grep": "text"}.]`
- Make the shortened view itself more useful for long command output: head, tail, and in
  between the lines that look like errors (same regex as above) with line numbers, so the
  model rarely needs to ask. This shaping lives in the terminal tool (or a shared helper
  in `crates/tools`), with unit tests on fixed inputs.
- Server `outputs`, tool `read_output {id, lines?, grep?, context?}`: returns the
  requested line range (1-based, inclusive, at most what fits the result cap; say what
  was cut and how to get the next range), or the lines matching `grep` (plain substring,
  case-insensitive) with `context` lines around each (default 2), with line numbers.
  Only outputs of the current chat can be read (test this). Read-only, never prompts.
  Clear errors: unknown id (say which ids exist), bad range, no matches (say how many
  lines there are).
- Tool card: in `src/components/toolcards.tsx` register nothing for every tool; instead,
  own `ToolCallCard.tsx`: when the result text ends with the "Shortened" line, show a
  "Full output" button that opens the stored text in the card (a Tauri command
  `get_tool_output(id)` returning the text for a chat's own output; plain text, scrollable,
  with "Copy").
**Files you own.** `crates/tools/src/files.rs`, `terminal.rs`, `git.rs`, `skills.rs`
(error messages and result shaping only; the Allow all agent edits the describe and
classification functions in the same files, so keep edits small and re-read before each);
a new `crates/tools/src/errors.rs` or similar for shared helpers (similar names, closest
lines, error-line picking); `crates/store/src/outputs.rs`; `src-tauri/src/outputs.rs`;
`src/components/ToolCallCard.tsx`, `ToolCallCard.test.tsx`.
**e2e scenario** `tool-errors-output`: a scripted `edit_file` with an ambiguous target
returns the match list and the model's scripted second call with `occurrence` succeeds;
a `read_file` of a long file is shortened with the reference line; a `read_output` call
with `grep` returns the matching lines; the card shows "Full output".

### C. Artifacts beside the chat (owner items 1 and 9)
Owner: "Open generated documents, HTML pages, diagrams, or small interactive tools beside
the chat. Support editing, versions, and export. Interactive previews need an isolated
execution environment." and "Preview generated pages and interactive tools, then send
rendering errors back to the model for correction."

**Model side.** Server `artifacts` with `create_artifact`, `update_artifact`,
`read_artifact`, `list_artifacts` (arguments in part 2). Kinds: `markdown` (a document),
`html` (a page or a small interactive tool: one self-contained HTML document with inline
CSS and JavaScript), `svg`, `mermaid` (a diagram in Mermaid syntax). `update_artifact`
with `old_text`/`new_text` makes a targeted replacement with the same helpful errors as
`edit_file` (not found, found more than once). Each create or update stores a new
version (author `model`). Limits: content up to 1 MB; title up to 120 characters,
sanitised; clear errors when exceeded. These tools touch only the app's database for the
current chat: no prompt (`Scope::AppData`), audited like every call. The result text starts with a fixed first line the
card can parse: `Artifact #ID "Title" (KIND), version N saved.` followed by the render
report (below).

**Render feedback (item 9).** After a create or update of an `html`, `svg` or `mermaid`
artifact, the tool waits up to 4 seconds for the app to render that version in the
preview and report what went wrong: uncaught errors, unhandled promise rejections,
`console.error` lines, blocked requests (CSP violations), a Mermaid parse error, or SVG
that does not parse. The result then says `Rendered without errors.` or lists up to 10
problems (`line:col message`, each capped at 300 chars) and asks for a corrected version.
If no chat window can render in time (for example the chat is not on screen), say
`Not rendered yet: open it to check.` Mechanism: the Rust tool emits an event to the
main window with the artifact id and version and awaits a oneshot that a Tauri command
`report_artifact_render(id, version, problems)` resolves; the frontend renders the
version in the panel (opening the panel on the Artifacts tab for the chat on screen),
collects problems for about 1.5 seconds after load, and calls the command. Render
problems are model-made content: treat them as data, cap and sanitise them.

**Isolation (must hold; test it).** `html`, `svg` and `mermaid` previews run in an
`<iframe>` that is isolated from the app:
- Served by a custom URI scheme protocol registered in Rust
  (`register_asynchronous_uri_scheme_protocol("artifact", ...)`): URL
  `artifact://localhost/<id>/<version>` (Windows/Android form
  `http://artifact.localhost/<id>/<version>`; build the URL with Tauri's
  `convertFileSrc`-style helper or by platform). Not `srcdoc`, `blob:` or `data:`:
  those inherit the app's own CSP.
- Response headers on every artifact response:
  `Content-Security-Policy: sandbox allow-scripts; default-src 'none'; script-src 'unsafe-inline' <artifact origin>; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; connect-src 'none'; form-action 'none'; frame-src 'none'; base-uri 'none'`,
  `X-Content-Type-Options: nosniff`, `Cache-Control: no-store`,
  `Cross-Origin-Resource-Policy: same-origin`. `sandbox` without `allow-same-origin`
  gives the page an opaque origin: no cookies, no storage, no access to the app's
  origin. No `allow-top-navigation`, `allow-popups`, `allow-forms`, `allow-modals`.
- The iframe element also carries `sandbox="allow-scripts"` and `referrerpolicy="no-referrer"`.
- The app's own CSP (`src-tauri/tauri.conf.json`, `app.security.csp`) gets `frame-src`
  for exactly the artifact scheme (both URL forms) and nothing else. No other directive
  is loosened. There is a test that pins the CSP string; update it deliberately.
- The page must have no way to call the app: Tauri commands are refused for it. Verify
  how Tauri 2 treats a custom-scheme, opaque-origin frame (capabilities apply to the
  app's own URL only; `src-tauri/capabilities/*.json` grant nothing to remote or other
  origins) and prove it in the e2e scenario: an artifact whose script tries
  `window.__TAURI_INTERNALS__`, `window.parent.document`, `window.top.location = ...`,
  `fetch("https://example.com")`, `new WebSocket(...)`, `localStorage`, and
  `window.open` must report every one of them as failed or absent (the script writes its
  findings into the page and posts them to the parent; the test reads them).
- The only channel out of the frame is `window.parent.postMessage` with a small JSON
  message `{source: "artifact", id, version, kind: "problem" | "ready", ...}`. The
  parent accepts messages only from the iframe's own `contentWindow`, checks the shape,
  caps sizes, and treats everything as untrusted text. A small bootstrap script that the
  protocol handler puts first in the document installs the error, rejection,
  `console.error` and `securitypolicyviolation` listeners and posts problems.
- `markdown` artifacts are rendered by the app's existing `Markdown` component (no
  scripts run there): no iframe needed. `svg` is rendered inside the sandboxed frame too
  (never inlined into the app's DOM).
- Mermaid: the library must be available offline. Add the `mermaid` npm package (MIT);
  serve its browser bundle to the frame from the artifact protocol
  (`artifact://localhost/__lib/mermaid.min.js`, embedded at compile time from
  `node_modules/mermaid/dist/mermaid.min.js` with a `build.rs` check that fails with
  "run npm ci first" when the file is missing). The frame page for a `mermaid` artifact
  is a tiny HTML page that loads the bundle, renders the source with
  `securityLevel: "strict"`, and posts parse errors as problems. Run
  `python3 scripts/notices.py` after adding it.

**User side: the Artifacts tab** (`src/components/workbench/ArtifactsTab.tsx`):
- A list of the chat's artifacts (title, kind, version count, updated time), newest first;
  choosing one shows it. When the model creates or updates one in the chat on screen,
  the panel opens on it.
- Viewer with two modes: **Preview** and **Source**. Source is an editor (a monospace
  `<textarea>` is enough; Tab inserts two spaces) with "Save as new version" (author
  `user`) and "Discard changes"; unsaved edits are kept while switching modes.
- Versions: a "Version N of M" control with previous/next, the author and time of each,
  and "Restore this version" (stores its content as a new version).
- Export: "Save to file…" (the dialog plugin's save dialog; default name from the title;
  extension by kind: `.md`, `.html`, `.svg`, `.mmd`; written by a Tauri command to the
  path the user chose) and "Copy" (source to the clipboard).
- Problems: when the preview reports problems, show them above the preview with a button
  "Ask the model to fix these" that puts a short message with the problems into the
  message box (`window.dispatchEvent(new CustomEvent("app:compose", {detail: text}))`).
- Rename and delete (with a confirmation).
- Tool card extra for `create_artifact` / `update_artifact`: a button "Open artifact"
  (parse the id from the result's first line) that opens the panel on it.
**Commands** (yours to name): list, get (with versions), save user version, rename,
delete, export to path, report render.
**Files you own.** `crates/store/src/artifacts.rs`; `src-tauri/src/artifacts.rs` (server,
commands, protocol handler, bootstrap script); `src-tauri/build.rs` additions;
`src-tauri/tauri.conf.json` (the CSP line only) and the test that pins it;
`src/components/workbench/ArtifactsTab.tsx` and new files under
`src/components/artifacts/`; one `mermaid` line in `package.json`.
**e2e scenario** `artifacts`: a scripted `create_artifact` (html with a button and a
script that throws once) → the panel opens, the preview shows, the tool result lists the
thrown error; a scripted `update_artifact` fixes it → "Rendered without errors."; the
isolation probe artifact described above reports every escape as failed; the user edits
the source and saves version 3; restore version 1 makes version 4; export writes the
file (call the export command with a path in the test's output folder); a mermaid
artifact renders an `<svg>`.

### D. Web search with verifiable sources, version-aware (owner items 2 and 10)
Owner: "Add a search service, retrieve relevant pages, and attach verifiable source
references to answers" and "Prioritize official documentation and match sources to the
library versions used in the project."

**New crate `crates/search`** (pure logic plus HTTP; no Tauri):
- Providers behind one trait: `duckduckgo` (no key; the HTML endpoint
  `https://html.duckduckgo.com/html/`; the default), `brave` (API key;
  `https://api.search.brave.com/res/v1/web/search`), `tavily` (API key), `searxng` (the
  user's own instance URL, JSON API `/search?format=json`). Parse each into
  `{title, url, snippet}`. Parsers are tested against saved fixture responses (write
  small realistic fixtures; do not depend on live network in tests). A provider error
  says what happened and what to do (no key set, rate limited, blocked, timeout).
- Fetch (`fetch.rs`): GET with the workspace's HTTP client (`reqwest`, already used);
  `http`/`https` only; resolve the host and refuse loopback, private, link-local, CGNAT
  and unspecified addresses BEFORE connecting, connect to the address that was checked,
  and repeat the check on every redirect (follow at most 5, by hand). Reuse the checks in
  `crates/browser/src/netpolicy.rs` if they can be shared (move shared pure functions to
  `crates/policy` only if it is a clean move; otherwise depend on the browser crate's
  public function or copy the small address classification with its tests). 10 second
  timeout, 2 MB body cap, text/html and text/plain only, a plain User-Agent with the
  app's name and version and nothing about the user. A test hook (cargo feature
  `test-hooks`, as in the browser crate) allows chosen loopback ports for the crate's
  own integration tests; the app never enables it.
- Extract (`extract.rs`): HTML → readable text (title, main content paragraphs, code
  blocks kept as text; scripts, styles, nav, header, footer, forms dropped). A small
  dependency for HTML parsing is acceptable (prefer one already in the lock file; check
  `Cargo.lock` first); justify it.
- Rank (`rank.rs`): split page text into passages (about 700 characters, on paragraph
  boundaries), score against the query with BM25, keep the best passages per page.
- Versions (`versions.rs`): read the project's libraries and versions from manifests in
  a folder: `package.json` (+ `package-lock.json` for exact versions), `Cargo.toml`
  (+ `Cargo.lock`), `pyproject.toml`, `requirements.txt`, `go.mod`. Output
  `{name, version, ecosystem}`.
- Official sources (`official.rs`): for a library, the hosts that are its official
  documentation: fixed per-ecosystem hosts (`docs.rs`, `doc.rust-lang.org`, `crates.io`,
  `npmjs.com`, `nodejs.org`, `developer.mozilla.org`, `docs.python.org`, `pypi.org`,
  `pkg.go.dev`, `go.dev`, `learn.microsoft.com`, `react.dev`, `vuejs.org`, `angular.dev`,
  `svelte.dev`, `nextjs.org`, `tailwindcss.com`, `typescriptlang.org`, `vite.dev`,
  `tauri.app`, `docs.djangoproject.com`, `fastapi.tiangolo.com`, `flask.palletsprojects.com`,
  `numpy.org`, `pandas.pydata.org`, `pytorch.org`, `kotlinlang.org`, `docs.oracle.com`,
  `developer.apple.com`, `developer.android.com`, and more of that kind, as data) plus,
  when the library's registry says so, its own documentation or home page host (one
  metadata request to `registry.npmjs.org/<name>`, `pypi.org/pypi/<name>/json` or
  `crates.io/api/v1/crates/<name>`, cached in memory, fetched through `fetch.rs`).
  `version_note(url, text, version)`: says "matches <lib> <major.minor>" when the URL or
  page names that version (docs.rs/<crate>/<version>, `/en/<version>/`, `/v<major>/`,
  `<major>.x`), "for <other version>" when it names another, else nothing.
- Pipeline `search(query, options) -> Vec<SourceHit>`: when `library` is given (or a
  known project library's name appears in the query), add its version to the provider
  query and look up its official hosts; get results; order them official first, then by
  provider rank; fetch the top pages (default 4, in parallel, each failing softly);
  extract and rank passages; return for each hit: title, url, site, snippet, excerpt
  (best passages, capped), `official`, `version_note`, content hash (SHA-256 of the
  extracted text), fetch time.

**App side** (`src-tauri/src/search_cmd.rs`):
- Server `search`: `web_search` stores each hit in `sources` with the next number `n`
  for the chat (store module `crates/store/src/sources.rs`; a URL already stored for the
  chat keeps its number) and returns to the model, per hit:
  `[n] Title — site (official documentation; matches tokio 1.38)` / URL / excerpt.
  It ends with: `Cite a source as [n] right after the sentence it supports. Use only what
  the excerpts say; if they do not answer the question, say so.` `read_source {n, query?}`
  fetches that source's page again and returns more passages (for `query` if given).
  Both are outbound (`outbound: true`, network) and their content is untrusted (it
  enters the turn as a tool result, which the loop already treats as untrusted). Scope:
  a new `Scope::WebSearch` (grantable per chat) unless the policy agent tells you
  otherwise; coordinate through your report, and build against what exists in
  `crates/policy/src/engine.rs` when you start.
- Settings (key `searchSettings`: provider, SearXNG URL, how many pages to read, version
  matching on/off) through your own commands; API keys only in the keychain
  (`state.secrets`, key names `search.brave`, `search.tavily`), written and cleared
  through commands that never return the key (return only "set" or "not set").
- The project's libraries come from the chat's working folder (`ChatContext`), read
  through the path sandbox like the file tools do.
**Citations in the UI.**
- In assistant messages, `[n]` that matches a stored source of the chat renders as a
  small chip (the number; the title and site in a tooltip). Clicking opens the panel's
  Sources tab on that source. `[n]` with no such source renders as plain text with a
  dotted underline and the tooltip "No such source". Implement in the markdown rendering
  (`src/components/Markdown.tsx` takes the chat's source numbers as an optional prop from
  `MessageItem.tsx`; add only what you need, and do not turn model text into HTML).
- Sources tab (`src/components/workbench/SourcesTab.tsx`): every source of the chat by
  number: title, site, badges ("official documentation", the version note), the search
  it came from, when it was fetched, the excerpt the model was given (plain text), the
  content hash, and "Open in browser" (through the existing confirmation for links, see
  `SafeLink` in `Markdown.tsx`). A "Check quotes" result per source: quoted passages
  (text in double quotes of 20 characters or more) in answers that cite it are looked up
  in the excerpt after normalising whitespace and case; show "quote found" or "quote not
  found in the retrieved text". Pure function with unit tests.
- Tool card extra for `web_search`: the hits as a compact list (number, title, site).
- Settings → a new "Search" tab (`src/components/SearchSettings.tsx`): provider choice,
  key fields (password inputs; show only set/not set), SearXNG URL, a "Test search"
  button, and one sentence saying that searches go to the chosen provider and pages are
  fetched from their sites.
**Files you own.** `crates/search/**` (new; add to workspace members);
`crates/store/src/sources.rs`; `src-tauri/src/search_cmd.rs`; `SourcesTab.tsx`;
`src/components/SearchSettings.tsx`; `src/components/sources/**`; the citation part of
`Markdown.tsx` and the prop pass in `MessageItem.tsx` (small edits; another agent edits
the user-message part of `MessageItem.tsx`).
**e2e scenario** `search`: the test starts a tiny local HTTP server that answers as a
SearXNG instance; set the provider to it through your settings command; a scripted
`web_search` call returns numbered hits (their pages are on loopback, so fetching them
is refused: the result must still list them with snippets and say the pages could not be
read); the scripted answer cites `[1]` and `[9]`; the chip for `[1]` exists and opens
the Sources tab; `[9]` is marked as no such source.

### E. Adaptive tool selection (owner item 4)
Owner: "Show a small set of relevant tools for each task, with clear descriptions and
argument examples. Let the model discover additional tools when needed."

**Behaviour.** The Tools panel still decides which tools are ON in a chat (allowed).
New: of those, each turn SHOWS the model only a small set; the rest stay hidden but
reachable. The model's request then carries fewer tool definitions (the owner had 25).
- Shown set = companion tools (`find_tools`, `call_tool`, `read_output`, `update_task`,
  when registered) + a core of file tools (`read_file`, `list_dir`, `search_files`,
  `edit_file`, `write_file`) when they are on + the tools of every group that matches
  the task + tools the chat has already used or found. Cap: 14 shown tools by default
  (setting `toolSelection`: `{auto: true, max: 14}`; `auto: false` shows everything that
  is on, as before). When the cap would be exceeded, keep companions, then used/found,
  then matched by score.
- Matching: score each group and tool against the text of the user's latest message and
  the two before it, using keywords per group (terminal: run, command, install, build,
  test, npm, cargo, pip, script, compile…; git: commit, branch, diff, push, pull, merge,
  repository…; browser: website, page, url, http, open, click, log in, form…; search:
  search, look up, latest, documentation, docs, find out, news…; artifacts: document,
  report, page, html, diagram, chart, draw, mermaid, svg, tool, calculator, table…;
  computer: screen, window, app, desktop, mouse, click on…; skills: skill names and
  descriptions; external servers: their tool names and descriptions) plus the words of
  each tool's name and description. Deterministic, pure, unit-tested with a table of
  messages → expected groups.
- Stability (prompt cache): the shown set is computed when the turn starts and does not
  change during the turn. It only grows across a chat (a tool once shown stays shown for
  that chat, kept in `chats.tool_prefs` as `shown: [names]`), so a later turn's tool
  list is the earlier one plus additions, in a stable order (the existing order of
  `enabled_for_chat`, with `run_subagent` last as today).
- `find_tools {query}`: searches ALL tools that are on in the chat (shown and hidden) and
  returns up to 8 matches, each with its name, a one-sentence purpose, its arguments
  (name, type, required) and one or two argument examples as JSON, and whether it is
  shown already or must be called through `call_tool`. It also names groups that exist
  but are switched off in this chat ("Git tools are off in this chat; the user can turn
  them on in the Tools panel"), without listing their tools. Found tools are added to
  the chat's shown set for the NEXT turn.
- `call_tool {name, arguments}`: runs a tool that is on in the chat but hidden this turn.
  It must behave exactly as if the model had called that tool: arguments validated
  against the target's schema (errors name the field), the policy `describe` and the
  gate use the TARGET tool's action (the approval card and the audit row show the real
  tool), the result is the target's result. Implementation: in
  `AppTools::describe` and `AppTools::execute` (`turns.rs`, at the marked point), unwrap
  `call_tool` into the target before describing and executing; a `call_tool` naming a
  tool that is off or unknown is refused with a message that lists close names;
  `call_tool` cannot call `call_tool`, `find_tools` or `run_subagent`. `find_tools`
  itself reads only the chat's tool list: no prompt (`Scope::AppData`).
- Catalog: `src-tauri/src/tool_catalog.rs`: for EVERY built-in tool (files, terminal,
  git, skills, browser, computer, subagents, and the new ones named in part 2) a short
  purpose line, group, keywords, and one or two example argument objects that validate
  against the tool's schema. A unit test builds the host's built-in tool list and fails
  when a built-in tool has no catalog entry or an example does not validate (for servers
  that are not registered yet when you finish, cover them with a test that validates the
  examples against the argument shapes in part 2, and list them in your report so the
  lead re-runs the full check at integration). External (MCP) tools use their own
  description and schema, and an example generated from the schema's required fields.
- Companion rule and defaults (in `mcp_cmd.rs`): implement "companion tools are offered
  whenever at least one other tool is on" and add `artifacts` and `search` to the groups
  that are on by default in a new chat; bump the saved-last-choice key (`DEFAULT_TOOLS`)
  so existing installs get the new defaults once; chats with their own saved choice keep
  it (their new groups start off, companions still apply). Update the tests there.
- Tools panel (`src/components/ToolsPanel.tsx`): each tool that is on shows a small
  "shown" or "on request" mark; a switch "Choose tools for each task automatically" (the
  `auto` setting) with one line of explanation; the count in the header button stays the
  number of tools that are ON.
**Files you own.** `src-tauri/src/tool_select.rs`, `src-tauri/src/tool_catalog.rs` (new),
`src-tauri/src/mcp_cmd.rs`, the marked tool-selection points in `turns.rs`
(`for_turn` call, and the `call_tool` unwrap in `describe`/`execute`),
`src/components/ToolsPanel.tsx`, `ToolsPanel.test.tsx`.
**e2e scenario** `tool-selection`: with files, terminal and git on (set through
`set_chat_tools`), a message about reading a file → the request the fake endpoint
receives carries the core and companion tools but not the git tools; a scripted
`find_tools {"query": "commit"}` returns `git_commit` with an example and says to use
`call_tool`; a scripted `call_tool` for `git_status` shows the approval card of
`git_status` itself; the next turn's request includes the git tools that were found.

### F. Tested settings per model, and coding benchmarks (owner items 5 and 13)  [wave 2]
Shared harness (`src-tauri/src/evals/`): run the real agent loop (`agent::turn::run_turn`)
headlessly against a given endpoint with the real file and terminal tools working in a
temporary workspace folder, under a fixed sandbox policy (only that folder is approved;
anything the gate would ask about is answered by rule: allowed if Allow all would allow
it, otherwise denied and counted as "blocked"), recording every call, error, edit
failure, step, token count and the time. No UI involvement; cancellable.
- Capability check (item 5): three short probes against the loaded model: tool calling
  (call `read_file` on a named file and report a word from it), editing (make a specified
  one-line change with `edit_file`; verified by file content), error recovery (the first
  edit target is ambiguous; succeed after the helpful error). Each probe runs under a
  few setting variants (thinking on/off where the model can think; native tool calls vs
  calls recovered from text; a low temperature) and the best-passing variant is saved as
  the model's profile for (model file identity, backend): key `modelProfile:<hash>`.
  Profiles are applied when that model runs on that backend (request template: thinking
  default, temperature) and shown in Settings → Local models as a small results table
  with a "Check this model" button, the date, and what was chosen. Optional: nothing
  runs unless the user asks.
- Benchmarks (item 13): a fixed set of at least 10 realistic coding tasks as data in
  `bench/tasks/<name>/` (a small fixture project, a task prompt, and a verification:
  either a command with required tools such as `python3`/`node`, or built-in content
  checks when the toolchain is absent; tasks whose toolchain is missing are reported as
  skipped). Mix: Python, JavaScript/TypeScript, Rust-free config and docs edits,
  multi-file rename, bug fix from a failing test, add a small feature, fix after an
  error message. Metrics per task and in total: completed (verification passed), broken
  edits (edit calls that failed, and files left not parsing where a parser check
  exists), tool failures, blocked actions, steps, tokens, time. Results saved as JSON in
  the app's data folder and shown in Settings → Diagnostics (a table per run, compare two
  runs). A way to run the harness with the scripted fake endpoint in tests so CI proves
  the harness and the verifications without a model.

### G. Visible, resumable task state (owner item 6)
Owner: "Track the goal, constraints, changed files, completed checks, and next action.
Support pausing, correcting, and resuming tasks after interruptions or restarts."

**Data.** One task record per chat (`tasks` table, JSON `state`): `goal` (string),
`constraints` (strings), `steps` (`{text, status: todo|doing|done}`), `changed_files`
(`{path, kind: created|edited|deleted, at}`; tracked by the app, not by the model),
`checks` (`{command, passed, at, summary}`; tracked by the app from `run_command`
results whose command looks like a test, build, lint or type check: `test`, `pytest`,
`cargo test|check|clippy|build`, `npm test|run build|run lint`, `tsc`, `go test|vet|build`,
`make`…), `next_action` (string), `notes`, `status` (`active`, `paused`, `done`),
`updated_at`. Store module `crates/store/src/tasks.rs`.
**Model side.** Server `task`, tool `update_task` (arguments in part 2): sets or replaces
goal, constraints, steps, next action, status; never changed files or checks. Touches only
the app's data for the chat: no prompt (`Scope::AppData`). Result: the
task as it now stands, compactly.
**App side** (`src-tauri/src/task.rs`): `after_call` (already wired) records changed
files from successful `write_file`, `edit_file`, `delete_file` calls and checks from
`run_command` results. At the start of every turn a compact note with the task state goes
to the model as an app note (`input.app_notes` in `turns.rs::run`, one line added by you
at a marked `[0.0.4 task]` point you create next to where `input.summary` is set): goal,
constraints, steps with status, changed files (up to 20), last checks, next action, and
for a paused task what the user changed. Keep it under about 300 tokens; no note when
there is no task. The note is data about the task, phrased as state, not as commands.
**User side: the Task tab** (`src/components/workbench/TaskTab.tsx`): goal, constraints,
steps (with status marks), changed files (click copies the path), checks (pass/fail with
time), next action, last updated. The user can edit goal, constraints, steps and next
action in place ("correcting"), saved through commands. Buttons: **Pause** (stops the
running turn if there is one, marks the task paused), **Resume** (marks it active and
sends the message "Continue the task. Start with the next action." so the model picks
up from the state; after an app restart the tab shows the saved state and the same
button), **Mark done**, **Clear task** (with confirmation). A small chip in the tab
header shows the status. When a turn ends unfinished (stopped, step limit, error) and a
task exists, it stays `active` with its state intact; nothing is lost on restart.
**Files you own.** `crates/store/src/tasks.rs`; `src-tauri/src/task.rs`;
`TaskTab.tsx` and files under `src/components/task/`; the one marked line in
`turns.rs::run`.
**e2e scenario** `task-state`: a scripted `update_task` sets goal and steps; a scripted
`write_file` (answer its card) appears under changed files; the tab shows them; Pause
during a streamed reply stops it and shows "paused"; reload the window
(`window.location.reload()`) and the tab still shows the task; Resume sends the continue
message and the request the fake endpoint receives contains the task note.

### H. File and symbol mentions (owner item 11)
Owner: "Let users reference specific files, functions, and classes directly in chat."

**Behaviour.** Typing `@` in the message box opens a picker over the chat's approved
folders: files (fuzzy match on path) and symbols (functions, classes, types; shown as
`name` with its file and line). Arrow keys and Enter choose; Escape closes. A chosen
item becomes a mention in the text (`@src/lib.rs` or `@src/lib.rs#parse_args`) and a
chip under the box. On send, each mention is resolved by the app and attached to the
message as context the model sees: for a file, its text (capped; long files send the
first part and say how many lines were left out and that the file tools can read more);
for a symbol, the lines of its definition (from its start to the end of its block, capped)
with the path and line range. Mentions respect the path sandbox exactly like
`read_file` (approved folders only, forbidden folders never, symlinks resolved), and
their content is untrusted file content (it already counts as an attachment for D9).
Sent messages show the mentions as chips.
**Index** (`crates/tools/src/symbols.rs`, pure, unit-tested per language): a light
definition finder using line-based patterns, not a full parser: Rust (`fn`, `struct`,
`enum`, `trait`, `impl`, `mod`, `const`, `type`), TypeScript/JavaScript (`function`,
`class`, `interface`, `type`, `enum`, exported `const` arrow functions, methods in
classes), Python (`def`, `class`, with indentation for the end of the block), Go
(`func`, `type`), Java/C#/Kotlin/Swift (`class`, `interface`, `enum`, methods by a
conservative pattern), C/C++ (functions by a conservative pattern, `struct`, `class`).
Output: name, kind, file, start line, end line (brace matching or indentation; fall
back to 40 lines). Walk folders with the same ignore rules the file search tool uses
(skip `.git`, `node_modules`, `target`, build output, files over 1 MB, binary files);
cap at 20,000 files and build incrementally in the background with a cache per folder
keyed by file size and modified time; searching must stay responsive (first results
under 200 ms on a project of a few thousand files; measure and report).
**Commands** (`src-tauri/src/mentions.rs`): `search_mentions(chat_id, query, limit)` →
files and symbols; `resolve_mentions(chat_id, mentions)` used by the send path. Hook the
resolution into message sending where attachments are handled (`chat.rs` /
`attachments.rs`: study how `documents` are attached and stored, and reuse it so the
context is stored with the message and shown again after a restart).
**UI** (`src/components/Composer.tsx`, `MentionPicker.tsx` new, the user-message part of
`MessageItem.tsx`): the picker is a listbox positioned above the box, keyboard and
mouse, with accessible roles; chips are removable before sending.
**Files you own.** `crates/tools/src/symbols.rs` (+ one `mod` line in
`crates/tools/src/lib.rs`); `src-tauri/src/mentions.rs`; the mention handling in
`chat.rs`/`attachments.rs` (small edits); `Composer.tsx`, `Composer.test.tsx` (new),
`MentionPicker.tsx`; the user-message part of `MessageItem.tsx` (another agent edits the
assistant-message/citation part).
**e2e scenario** `mentions`: with an approved folder holding `notes.txt` and a small
`calc.py` with `def add(a, b)`, typing `@add` lists the symbol; choosing it and sending
makes the request the fake endpoint receives contain the definition lines with the path
and line range; a mention of a path outside the approved folders (sent through the
command directly) is refused.

### I. Pinned instructions and a context inspector (owner item 12)  [wave 3]
Owner: "Keep important instructions available and show users what information will be
sent to the model."
- Pinned instructions: short texts the user pins for one chat or for every chat
  (`pinned_instructions` table; store module `crates/store/src/pinned.rs`). They are
  part of the fixed start of the prompt (after the system prompt), so compaction and
  trimming never drop them (`crates/agent`: a `pinned: Vec<String>` field on
  `TurnInput`, rendered in `prompt.rs`; unit tests that they survive every context
  stage). Limits: 20 pins, 1,000 characters each. UI: a "Pin" action on a user message
  (pins its text), and a list in the Context tab to add, edit, reorder, remove and
  switch between "this chat" and "every chat".
- Context inspector (the Context tab): exactly what the next request would contain,
  built by the SAME code that builds a real turn (refactor `turns.rs::run` so the part
  that assembles the input and tool list is one function used by both), without
  sending: sections (system prompt, pinned instructions, preamble, skills, loaded
  skills, tool definitions with the shown tools' names, summary of earlier messages,
  messages kept, messages left out, task note, attachments) each with a token estimate
  and expandable text; the total against the model's context size; what would be
  dropped next. A "Copy all" button. Refresh when the chat, its tools or its pins change.

---------------------------------------------------------------------------------------
End of spec.
