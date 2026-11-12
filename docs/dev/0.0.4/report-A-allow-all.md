<!-- Saved by the lead from the developer's final message; the developer could not write this file itself. -->

# Report A: Allow all (owner item 3)

Note on the report file first. You asked for this report at /home/user/harnissUI/docs/dev/0.0.4/report-A-allow-all.md. The Write tool refused it ("Subagents should return findings as text, not write report files"), and I did not work around that with another tool. The file does not exist. This message is the complete report; please save it to that path yourself.

State: the feature is built end to end. Every crate I touched builds, its tests pass, clippy is clean on all eight, and the end-to-end scenario `allow-all` passed three times against the real app. No TODO is left in the code.

1. FILES CREATED AND CHANGED

Created (mine):
- /home/user/harnissUI/crates/policy/src/allow_all.rs: what still asks in Allow all: `command_facts` (commands), `web_label` (web labels), `external` (external tools).
- /home/user/harnissUI/crates/policy/src/allow_all/tests.rs: the table tests, one per rule, both directions.
- /home/user/harnissUI/crates/tools/tests/allow_all.rs: what the files, terminal and git describers tell the engine.
- /home/user/harnissUI/src-tauri/src/approvals.rs: the per-chat mode, the saved default, the four commands, held browser navigations.
- /home/user/harnissUI/src/lib/approvalMode.ts: the window's mirror of the modes.
- /home/user/harnissUI/src/components/AllowAllButton.tsx: header button, confirmation dialog, hint.
- /home/user/harnissUI/src/components/AllowAllButton.test.tsx and /home/user/harnissUI/src/components/PermissionsSettings.test.tsx.

Changed (mine):
- /home/user/harnissUI/crates/policy/src/engine.rs: `KeepAsking`, `Action::keep_asking`, `Action::network`, `TurnContext::allow_all`, `Decision::AllowedByMode`, `AskReason::StillAsksInAllowAll`, the Allow-all branch of `evaluate`, two tests.
- /home/user/harnissUI/crates/policy/src/command.rs: `Verdict::keep_asking`, `Verdict::network`, `parse_lenient` (used only by the Allow-all classification), lists made `pub(crate)`, two tests. `parse`, `always_ask`, `is_read_only` and the Ask-mode part of `classify` are unchanged.
- /home/user/harnissUI/crates/policy/src/files.rs: `keep_asking(canonical, kind, flavor)`, one test. /home/user/harnissUI/crates/policy/src/lib.rs: `pub mod allow_all;`, `KeepAsking` re-exported.
- /home/user/harnissUI/crates/browser/src/intent.rs: `ElementInfo::form_submit_label`, `click_keeps_asking`, `submit_keeps_asking`, `key_keeps_asking`, one test.
- /home/user/harnissUI/crates/browser/src/server.rs: the `action` helper takes a `Mode` (keep asking, network), every describer sets it, three call-time rechecks (`LABEL_CHANGED`).
- /home/user/harnissUI/crates/browser/src/approval.rs: `HeldNavigation::released_in_allow_all`, one test.
- /home/user/harnissUI/crates/computer/src/server.rs: `input_action` sets `keep_asking`; the guard's `always_ask_approved` now also requires `keep_asking`. /home/user/harnissUI/crates/computer/src/server_tests.rs: three tests.
- /home/user/harnissUI/crates/mcp-host/src/describe.rs: `keep_asking` and `network` for external tools, one test.
- /home/user/harnissUI/src/components/ApprovalCard.tsx, /home/user/harnissUI/src/components/ApprovalCard.test.tsx, /home/user/harnissUI/src/components/PermissionsSettings.tsx.
- /home/user/harnissUI/src-tauri/src/permissions.rs: not changed (its audit filter already parses any decision).

Shared files, exactly what I added:
- /home/user/harnissUI/crates/tools/src/files.rs: the two new fields in the three `Action` literals; `write_action` sets `keep_asking: policy::files::keep_asking(..)`.
- /home/user/harnissUI/crates/tools/src/terminal.rs: the two fields in both literals (from the verdict).
- /home/user/harnissUI/crates/tools/src/git.rs: the two fields in `action` (push keeps asking as Publish; network = outbound), in `deny`, and in one test literal (defaults).
- /home/user/harnissUI/crates/tools/src/skills.rs, /home/user/harnissUI/crates/mcp-host/src/host.rs, /home/user/harnissUI/crates/agent/tests/llama_agent.rs, /home/user/harnissUI/src-tauri/src/subagent.rs: the two fields with their defaults.
- /home/user/harnissUI/crates/agent/src/turn/mod.rs: `CallDecision::AllowedByMode`. /home/user/harnissUI/crates/agent/src/turn/runner.rs: one match arm.
- /home/user/harnissUI/crates/agent/tests/turn_loop.rs: `FakePolicy` gained `allow_all` (off by default); one test.
- /home/user/harnissUI/crates/store/src/audit.rs: `AuditDecision::AllowedByMode` (`allowed_by_mode`), one test.
- /home/user/harnissUI/crates/store/src/schema.rs: a marked block `[allow all]` at the end of `V2` that rebuilds `audit_log` with the new decision in its CHECK; one test in the tests module.
- /home/user/harnissUI/src-tauri/src/turns.rs: (1) at the marked point, `allow_all: crate::approvals::for_decision(self.chat_id, untrusted_in_turn)`; (2) `decision_name`: the arm for `AllowedByMode`, label "allowed by Allow all"; (3) `audit_entry`: `AllowedByMode` is decided by "policy"; (4) `ApprovalDto::keep_asking` and its two constructions; (5) the two default fields in the test helper `action_with`; (6) two tests.
- /home/user/harnissUI/src-tauri/src/subagent/tests.rs: default fields in two literals, one test.
- /home/user/harnissUI/src-tauri/src/lib.rs: `mod approvals;`, four handler entries, one setup line `approvals::load_default(&handle);`.
- /home/user/harnissUI/src-tauri/commands.list and /home/user/harnissUI/src-tauri/capabilities/main.json: the four commands.
- /home/user/harnissUI/src-tauri/src/browser_cmd.rs: four marked lines at the top of `AppNavigationApprover::approve`.
- /home/user/harnissUI/src-tauri/src/mcp_cmd.rs: one marked line in `clear_chat_grants`.
- /home/user/harnissUI/crates/browser/src/page.rs: the element script also reports `formSubmitLabel`.
- /home/user/harnissUI/crates/browser/tests/browser_e2e.rs: one test.
- /home/user/harnissUI/src/lib/ipc.ts: five wrappers. /home/user/harnissUI/src/lib/types.ts: `KeepAsking`, the new reason, `ApprovalRequest.keepAsking`.
- /home/user/harnissUI/src/components/ChatView.tsx: one import and one mount of `AllowAllButton`.
- /home/user/harnissUI/src/components/Composer.tsx: the hint's last phrase is now `<ApprovalHint />` (import plus one phrase).
- /home/user/harnissUI/e2e/scripted_agent.py: `p_allow_all` and its entry in `PROBES`.

No new dependency.

2. WHAT IT DOES, AS THE USER SEES IT, AND THE DESIGN CHOICES

Header. Every chat has a shield button next to Tools. In Ask mode it reads "Ask first" (muted; on a narrow header only the icon). In Allow all it reads "Allow all" on a warning tint, always with its text. Clicking "Ask first" opens a confirmation; clicking "Allow all" switches back at once. The hint under the message box follows the mode: "the model asks before it changes anything" or "Allow all is on: the model asks only before deleting and a few other things".

The confirmation, word for word (for the setting, "this chat" reads "every chat"):

Title: Allow all in this chat?
Text: The model will no longer ask before it reads, writes or edits files in your approved folders, runs commands, commits with git, browses, or uses its other tools.
Heading: It still asks before:
- Deleting files or folders, however it is done, and emptying a file.
- Throwing away work or changing the computer: git reset --hard, a forced push, disk and system commands, registry edits, and anything run as administrator.
- Commands that hide what they run.
- Writing to sensitive places: a .git folder, shell start-up files, git and ssh settings, autostart folders.
- Paying, sending and publishing: web page buttons that say pay, buy, order, checkout, transfer, delete, remove, send, post or publish, and git push.
- Typing or clicking in a terminal, in system settings or in a password manager.
- Tools from added servers that are marked destructive, or that you set to always ask.
- Sending data out after the model has read a web page, a file or a tool result in the same turn: opening another site, typing into a page, tools from added servers, git pull and clone, and commands that use the network or cannot be checked.
Heading: Good to know:
- File tools still cannot reach folders you have not approved.
- Commands run with your rights and can also change things outside the approved folders.
- The app checks the command it is about to run, not what a script or program does once it is running.
- It stays on in this chat until you switch it off or quit the app. (For the setting: "It applies to every chat and stays on after a restart, until you switch it off here.")
Buttons: Cancel, and "Allow all in this chat" (or "Allow all in every chat").

Cancel has the focus. The confirm button ignores input for 700 ms after the dialog opens. The buttons stay in view while the text scrolls.

Approval card. A card that can be granted has a third choice, "Allow all in this chat". It opens the same confirmation. On confirm the chat is switched first and then this card is approved (as Allow once); if the switch is refused nothing is approved. The choice is not shown on once-only cards or while the chat is already in Allow all. The card's own rules are kept (no default focus on Allow, the input delay, Enter never approves).

Card reasons in Allow all. A card that shows because of the list says "Allow all is on in this chat, but this still asks: " followed by one of:
- it deletes files or folders, or leaves a file empty
- it can throw away work or change the disk or the system
- it writes to a sensitive place (a .git folder, start-up files, git or ssh settings, autostart)
- it pays, sends or publishes something in your name
- it sends input to a terminal, system settings, a password manager or outside the application in front
- its server marks this tool as destructive, or you set it to always ask
- what it will do cannot be checked
Such a card has Allow once and Deny only. A card that shows because untrusted content is in the turn says: "Allow all is on in this chat, but the model has read content from files, pages or tools in this turn, and this action can send data out or cannot be fully checked. Make sure it is what you asked for."

Settings, Permissions: a section "Approvals" with the switch "Allow all in every chat", off by default. Switching it on shows the confirmation. Switching it off is immediate and puts every chat back to asking, also chats switched on by hand. Settings, Audit log shows such calls as "allowed by Allow all" and can filter for them.

Design choices:
- The mode is one process-wide `ApprovalModes` in approvals.rs (a default flag and a per-chat map behind a mutex). The gate reads it at the marked point on every decision with only the chat id. So the edit in turns.rs is one line, and a helper's gate, which `run_subagent` builds for the same chat, is in the same mode with no extra wiring.
- A chat's own choice is memory only (D12). Only the default is a saved setting (`allowAllDefault`), written by its own command.
- The engine never guesses. Describers state `keep_asking` and `network`. In Allow all `evaluate` is: steps 1 and 2; the Skill and AppData shortcut, unchanged and still audited as "no approval needed"; keep asking gives Ask (once only); untrusted plus network gives Ask (as D9); otherwise `AllowedByMode`. Grants are not read. Calls recovered from text are treated as native.
- Commands. A plain command is judged by its program and its arguments. Anything that cannot be fully broken down (Complex, a wrapper, inline interpreter code, `find -exec`, git with `-c` or an unknown subcommand) has its raw text scanned, case-insensitively, on word boundaries (a word is a run of letters, digits, `_`, `-`, `.`), once as written and once with quotes, backslashes, backticks and `^` taken away (`r''m`, `r\m`, `d^el`). "format" is a hit as a program name where the parser can tell, and as a word where it cannot; a flag's name (`--format`) and a method (`.format(`) are not hits.
- PowerShell: for the Allow-all classification only, a double-quoted string with no `$` and no backtick counts as the literal it is. Without this almost every quoted argument on Windows would make a command "not fully broken down". What a grant can cover in Ask mode is unchanged.
- Held browser navigations. A submission the described action announced is released by its intent, exactly as after an approval in Ask mode. Of the requests that still get held, Allow all releases only a page moving its tab to another site during a call, while no untrusted content had entered the turn when that call was decided (for example a redirect of the first `browser_navigate` of a turn). Held submissions, background sends and anything held outside a call keep their card in every mode: nothing announced them and there is no label to judge.
- "Forget everything I allowed in this chat" also ends Allow all in that chat. The app tells the window when a mode changes (event `approval-mode-changed`), so the header button does not show a stale mode.

Additions beyond the eight rows. All are on the asking side, and each is one list or one rule in allow_all.rs:
1. A delete or publish word handed to another program as an argument of its own, in a plain command whose program is not on the read-only list: `npx rimraf dist`, `cargo clean`, `npm run clean`, `make clean`, `docker rm x`, `kubectl delete`, `pip uninstall`; flags such as `--delete`, `/purge`, `--remove-orphans`; `npm publish`, `docker push`, `firebase deploy`. An argument of several words is scanned like a command (`ssh host "rm -rf x"`).
2. Git commands that discard work: `checkout --`, `checkout .`, `checkout -f`, `restore` (unless `--staged` only), `switch --discard-changes`, `stash drop` and `clear`, `branch -D`, `worktree remove`. `git config` that sets a value counts as sensitive.
3. Commands that hide what they run (kind `Hidden`): `-EncodedCommand` and its short forms, base64 decoding piped to a shell, `FromBase64String`, `b64decode`, `atob(`.
4. A command that is not a plain read and names a sensitive place (`.git/...`, `~/.bashrc`, `.ssh/...`, `$PROFILE`, the Windows Startup folder).
5. More delete programs (`truncate`, `Clear-Content`, `clc`, `sdelete`, `rimraf`, `Remove-*`, `Uninstall-*`), `robocopy /MIR` and `/PURGE`, `/dev/null` as an argument.
6. Web labels: also donate, withdraw, subscribe, book, reserve, reply, share, authorize, approve. Links count as well as buttons. Enter in a form field asks when the form's own submit button has such a label.
7. Computer use: a system shortcut (Win+R and the like) and a drag into another application keep asking, like protected applications.
8. More network programs for rule 8 (`nslookup`, `dig`, `ping`, `socat`, `sendmail` and a few more), and any argument that holds `://`.

3. TESTS ADDED, AND WHICH I SAW FAIL FIRST

Seen failing before the code existed:
- crates/policy: all 21 tests of `allow_all::tests` failed against stubs, then passed. By rule: `rule1_every_delete_command_still_asks`, `rule1_deleting_a_file_and_leaving_one_empty_still_ask`, `rule2_destroying_commands_still_ask`, `rule2_git_commands_that_discard_work_still_ask`, `rule3_a_command_that_cannot_be_broken_down_asks_when_it_names_a_delete`, the same for a destroy, `rule3_without_such_a_word_it_is_allowed`, `rule3_the_word_format_is_a_hit_only_where_it_can_be_the_program`, `rule3_a_command_that_hides_what_it_runs_still_asks`, `a_delete_word_passed_to_another_program_still_asks`, `rule4_sensitive_writes_still_ask`, `rule4_a_command_that_names_a_sensitive_place_still_asks`, `rule5_web_labels_that_spend_destroy_or_publish_still_ask`, `rule5_publishing_commands_still_ask`, `rule6_input_to_protected_applications_still_asks`, `rule7_external_tools_marked_destructive_or_always_ask_still_ask`, `rule8_commands_that_name_a_network_program_ask_after_untrusted_content`, `rule8_plain_local_commands_are_allowed_also_after_untrusted_content`, `powershell_double_quoted_literals_are_plain_for_allow_all_only`, `the_mode_comes_after_steps_1_and_2_and_changes_nothing_in_ask_mode`, `every_row_of_what_still_asks` (one line per row of the user's list, both directions).
- crates/computer: `an_input_that_allow_all_could_let_through_never_reaches_a_protected_target`. I took the guard change out and it failed; put back, it passed.
- crates/browser: one row of `allow_all_asks_for_labels_that_spend_destroy_or_publish` failed first because my test named the Space key wrongly. I corrected the test, not the rule.

Written with or after their code, not seen failing:
- policy: `engine::allow_all_replaces_steps_3_to_7`, `engine::the_allow_all_facts_default_to_nothing`, `files::what_always_asks_keeps_asking_in_allow_all`, `command::the_lenient_parse_only_adds_plain_double_quoted_strings`, `command::the_verdict_carries_the_allow_all_facts`, and the table rows added after the first pass (bundled flags such as `bash -lc`, code on standard input, quoted words, `forfiles`, `wsl`, git options that run a command, several words in one argument).
- tools: the three tests in tests/allow_all.rs. mcp-host: `allow_all_keeps_asking_for_marked_tools_and_treats_every_call_as_network`.
- browser: `approval::allow_all_releases_only_a_move_to_another_site_before_untrusted_content`, and `allow_all_facts_on_a_real_page` in tests/browser_e2e.rs (real Chrome).
- computer: `allow_all_lets_ordinary_input_to_ordinary_applications_through`, `allow_all_still_asks_for_protected_applications`.
- agent: `a_call_allowed_by_the_mode_runs_without_a_card`.
- store: `the_audit_log_of_a_version_1_database_survives_the_rebuild`, `calls_allowed_by_the_mode_have_their_own_decision`.
- app: seven tests in `approvals::tests`; `turns::tests::a_mode_switch_reaches_a_running_turn_and_its_helpers`; `turns::tests::a_call_allowed_by_the_mode_is_audited_as_allowed_by_allow_all`; `subagent::tests::a_helper_is_in_its_chats_approval_mode`.
- frontend: 8 new tests in ApprovalCard.test.tsx, 8 in AllowAllButton.test.tsx, 4 in PermissionsSettings.test.tsx. The 5 existing tests of ApprovalCard.test.tsx assert what they asserted before; the file now mocks `../lib/ipc` and imports the card after the mock.

4. CHECKS RUN AND THEIR RESULTS

Rust, `CARGO_INCREMENTAL=0`, toolchain 1.99, all from the final run:
- `cargo +1.99 test -p policy`: 73 passed, 0 failed. 46 existed before; one of the 73 is the search feature's test in engine.rs.
- `cargo +1.99 test -p store`: 99 and 4 passed.
- `cargo +1.99 test -p agent`: 144, 30, 12, 1, 1, 1 and 39 (turn_loop) passed.
- `cargo +1.99 test -p mcp-host`: 90, 6 and 1 passed.
- `cargo +1.99 test -p tools`: 108 and 3 passed.
- `cargo +1.99 test -p computer`: 93 passed.
- `cargo +1.99 test -p browser`: 113 unit passed (1 ignored), 30 end-to-end with real Chrome passed.
- `cargo +1.99 test -p app --lib`: 232 passed, 0 failed.
- `cargo +1.99 clippy -p <crate> --all-targets -- -D warnings` for policy, store, agent, mcp-host, tools, browser, computer, app: all clean.
- `cargo fmt --all`: run. `cargo +1.99 build -p app`: finished.
- `bash scripts/check-spawn.sh`: passed.
- `python3 scripts/notices.py --check`: reports THIRD-PARTY-NOTICES.md out of date. I added no dependency and did not regenerate it.

Frontend:
- `npm run lint`: clean. `npm run typecheck`: clean. `npm run build`: built.
- `npm test`: 151 tests passed; 1 file failed to load, /home/user/harnissUI/src/components/ToolCallCard.test.tsx ("No isTauri export is defined on the ../lib/ipc mock"). That comes from /home/user/harnissUI/src/components/sources/state.ts importing `isTauri` while that test's mock does not define it (features B and D), not from this feature.

End to end:
- `flock /tmp/harness-e2e.lock env PROBES=allow-all E2E_OUT=<scratch> timeout 600 xvfb-run -a python3 -I e2e/scripted_agent.py` after `cargo +1.99 build -p app`: PASS three times, 0 console lines. It covers: a write asks in Ask mode; `set_setting` refuses the mode keys; the header button's confirmation (text checked, Cancel changes nothing); a write and an edit run with no card and are audited as `allowed_by_mode`; a delete still shows a once-only card with its reason; Settings, Audit log shows "allowed by Allow all"; switching back makes a write ask again; the card's third choice; "Forget everything I allowed in this chat" puts the chat back to Ask first; the Settings switch (on with confirmation, saved, off at once).
- Screenshots and the dialog text as shown are in /tmp/claude-0/-home-user-harnissUI/23ef2267-cb6f-5325-b082-a4b40b3ccb19/scratchpad/allowall/e2e/ (allow-all-dialog.png, allow-all-dialog.txt, allow-all-on.png, allow-all-delete-card.png).

Not run: the other end-to-end scenarios, and a whole-workspace test run in one command.

5. FOR ARCHITECTURE.md

Approval modes (13.3). A chat is in one of two modes. Ask first is the evaluation order above. In Allow all, steps 1 and 2 hold unchanged (a tool that is off, a hard limit), skills and the app's own data are allowed as before, and steps 3 to 7 are replaced by three: an action that keeps asking gets an approval card with Allow once and Deny only, with the kind of reason on the card; an action that can send data out, once untrusted content has entered the turn, gets a card that says why (D9); anything else is allowed and logged as "allowed by Allow all". Grants are not consulted and a call recovered from text is treated like a native one.

Describers state two facts on every action, which Ask mode does not read: `keep_asking` (deleting, destroying, a sensitive place, publishing, a protected application, a tool marked destructive, a command that hides what it runs) and `network` (the action can send data off the machine). `Risk::AlwaysAsk` keeps its meaning in Ask mode and is not what Allow all looks at: a command run through a wrapper always asks in Ask mode, and in Allow all only when its text names a delete or destroy.

What keeps asking: `delete_file`, a write that leaves a file empty, writes to the sensitive locations; a delete program whatever its flags, `git rm`, `git clean`, `find -delete`; `git reset --hard` and the other git commands that discard work, a forced push, disk and system programs, registry edits, elevation; a command that cannot be fully broken down when its text holds a delete or destroy word or a file-removal call of a common language; an encoded command; `git push` and the `git_push` tool, commands that publish or deploy; a web control (button, submit control or link) whose label says pay, buy, purchase, order, checkout, transfer, delete, remove, send, post or publish, and Enter in a form whose submit button says so; computer input to terminals, system settings, password managers, the system's own shortcuts, a drag into another application; external tools marked destructive or always-ask. After untrusted content: browser navigation to another site, browser typing and submissions, external tool calls, `git_pull`, `git_clone`, and commands that name a network program or cannot be fully broken down.

Who can switch it. The mode is state in the app's Rust code (approvals.rs): a per-chat choice kept in memory only (D12) and one saved setting, "Allow all in every chat". It is changed only by two commands that are granted to the main window alone and check the calling window again; the generic settings command refuses the key (a test reads its allow-list). No tool reaches it and nothing the model writes is read there. The policy gate reads the mode on every decision, so a switch reaches a running turn in both directions, and a helper's gate is built for the same chat and so is in the same mode. "Forget everything I allowed in this chat" also puts the chat back to asking.

Held navigations. A submission announced by the action in progress is released by its intent as before. Of other held requests, Allow all releases only a page moving its tab to another site during a call, while no untrusted content had entered the turn when that call was decided; this is audited as its own row. Held submissions and anything held outside a call keep their always-ask card in every mode.

What runs is what was shown. A browser action described without a keep-asking label is refused at call time if the control, the form's submit button or the focused control has such a label by then. A computer input that always asks counts as approved for the input guard only if it also keeps asking, so nothing Allow all let through can reach a terminal or console window.

Audit (13.7). A new decision, `allowed_by_mode`, decided by "policy", shown as "allowed by Allow all". The `audit_log` table is rebuilt in schema version 2 to accept it: rows, ids, results and parent links are kept, the append-only triggers and indexes are recreated, and a test starts from a version 1 database.

Limits to state plainly (1.4). Allow all is a guard, like the terminal classifier it is built on, not a sandbox. It reads the command line. It cannot see what a script file or a program does once it runs (`python cleanup.py`, `npm run build`, `make`, a script the model wrote a minute earlier), a delete built from variables or string joins, or a file emptied by a redirection (`> file`). In Allow all a terminal command is no longer shown before it runs, and commands are not held to the approved folders (only the built-in read-only programs' path arguments are). Background requests a web page makes by script are not held, as in D8.

6. NOT VERIFIED, DECISIONS FOR YOU, TODOS

Edits outside my file list. All are small and marked; please keep them or tell me where they should live:
- crates/agent/src/turn/mod.rs and runner.rs: the new `Decision` variant does not compile without a `CallDecision` and one arm.
- src-tauri/src/turns.rs: besides the marked point and `decision_name`, I added `ApprovalDto::keep_asking` (the card cannot state its reason without it) and two tests.
- src-tauri/src/browser_cmd.rs: four lines so a held navigation can be released.
- src-tauri/src/mcp_cmd.rs (feature E): one line in `clear_chat_grants`, `crate::approvals::ask_first(chat_id.0);`. If E rewrites that function the line must survive, or "Forget everything I allowed" leaves the chat in Allow all.
- src/components/Composer.tsx (feature H): the hint's last phrase is `<ApprovalHint />`. If H rewrites the file, the phrase and its import must survive, or the hint will claim the model always asks.
- crates/browser/src/page.rs: `formSubmitLabel` in the element script (exercised by the real-browser test).
- crates/store/src/schema.rs: a test in the tests module, next to my block.

One deliberate tightening that also applies in Ask mode: a browser click, type-and-Enter or key press is refused at call time when the control's label became one that always asks after the action was described. It fires only when a page relabels a control between the card and the click. No existing test changed.

Behaviour you may want to change:
- Switching "Allow all in every chat" off also ends Allow all in chats switched on by hand.
- The additions listed in section 2 ask more than the eight rows do.
- After the first tool result of a turn, every command that cannot be fully broken down asks (rule 8 as written). On PowerShell that includes globs (`dir *.txt`), `$env:` variables, `2>&1` and redirections, so such commands ask more often than the owner may expect.
- /home/user/harnissUI/src/components/Welcome.tsx still says the model asks before it changes anything. With the default switch on that is no longer true for new chats. I did not edit it.
- A database created by a 0.0.4 development build before my `V2` block existed is already at schema version 2 and does not get the rebuilt audit table. Audit rows for calls allowed by the mode fail to insert there (logged). Delete such a development database. A 0.0.3 database upgrades correctly (tested).

Not verified:
- Windows and macOS: nothing was run there. The PowerShell rules are tested through the parser only; no command was run in PowerShell. The computer-use rules are tested on the fake desktop.
- What the classifier cannot see is by design and untested: a script file's own code, a program's own behaviour, obfuscation by variables or string joins, emptying a file through a redirection.

TODO: none left in the code.
