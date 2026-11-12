REVIEW: feature F (tested settings per model, coding benchmarks)

Summary: the file-tool side holds. The capability check uses file tools only, and I could not get it out of its folder. The benchmark terminal does not keep the model in its folder, so the benchmark confirmation text ("commands that delete, go online or cannot be understood are refused") is not true. I confirmed escapes by experiment, both through scripts the model writes and through single plain command lines.

The other findings:
- Verification can be passed without solving the task.
- Verification can hang forever, and Stop does not end it.
- The capability-check text says "Nothing leaves this computer" even when the chosen model is a remote endpoint.
- One evals test is red.

How I tested: I built a throwaway crate in my scratch folder (`/tmp/claude-0/-home-user-harnissUI/23ef2267-cb6f-5325-b082-a4b40b3ccb19/scratchpad/review2/probe`). It contains an unmodified copy of `src-tauri/src/evals/sandbox.rs` and the real `tools::FilesServer` and `TerminalServer` describers from the workspace crates. It runs `Rules::judge` on each call and, where the rule allows it, runs the call for real. The work folder and the "outside" folder are both inside my scratch folder. `git status --short` shows nothing of mine.

FINDINGS, most serious first

1. The benchmark rule does not confine the model. Commands it allows read, write and delete outside the folder, and can go online. CONFIRMED.
   - Where: `src-tauri/src/evals/sandbox.rs` (`judge` and `command_stays_inside`, about lines 125–330), and the promise in `src/components/evals/shared.tsx:24-25` and in the audit preview at `commands.rs` (start_benchmark).
   - Rule broken: 1, the sandbox rule; and 8, the user is told the truth.

   a) Scripts the model writes and then runs. The rule judges the command line, not the code it runs.
   - I did `write_file helper.py`, then `python3 helper.py`. Both were judged Run.
   - The script read `outside/secret.txt` (its content came back to the model) and wrote `outside/written_by_script.txt`.
   - It also deleted `outside/secret.txt` with `os.remove` and made a symlink `link -> outside` in the work folder.
   - The same class covers `python3 -m unittest` (imports model-written `test_*.py`), `node --test`, `node x.js`, `sh x.sh`, `./x.sh`, `perl x.pl` and `make`. All were judged Run.
   - Such code runs with the user's rights. It can reach `~`, delete files and go online.
   - It can also outlive the run. `python3 x.py &` is judged Run, and a process that starts its own session is never killed.

   b) No script needed, one plain command line. Words that hold a path but do not start with one are read as harmless names inside the folder. All of these were judged Run:
   - `sed -n '1w /ABS/outside/sed_w.txt' notes.txt` — ran and wrote outside.
   - `sed -n 1w/ABS/... notes.txt` — ran and wrote outside.
   - `sed '1e touch /ABS/outside/sed_e.txt' notes.txt` — ran: sed's `e` runs any shell command.
   - `awk 'BEGIN{print 1 > "/ABS/outside/awk.txt"}'` — ran and wrote outside.
   - Judged Run, not executed by me: `awk 'BEGIN{system("id")}'`, `tar -cf x.tar notes.txt --checkpoint=1 --checkpoint-action=exec=id`, `zip -r a.zip . -T -TT 'id #'`, `vim -c ':!id' notes.txt`.
   - These were blocked only when the embedded command held a delete or network word.

   c) Globs together with a symlink. Globs are never expanded by the check.
   - `ln -s .*. up` was judged Run, and I ran it. On Linux with dash as `/bin/sh`, `.*.` matches `..`, so it made `up -> ..`.
   - Then `ls u*/` listed the parent folder, `cat li*/secret.txt` printed the outside file, and `cp notes.txt u*/outside/` wrote outside. All judged Run and executed.
   - With the folder in `/tmp`, that reaches `/` and then the home folder.
   - `mv notes.txt li*` was also judged Run (not executed). With `mv` an outside file can be moved into the workspace, and clean-up then deletes it. That breaks "no file is deleted".
   - The existing guards against plain `link`, `link/`, `cd link` and `cp … link` work. Only the glob forms get through.

   d) Verification also runs model-written code (the verifier imports `stats.py`, `cart.js` and so on), with the user's rights. That happens outside the gate and with no audit row.

   Smallest honest fix:
   - First, change `BENCH_CONFIRM` and the run's audit preview to say plainly that the code the model writes and the tests it runs execute on this computer with the user's rights. They are not sandboxed and can reach files outside the folder and the internet. Suggest running benchmarks only with models you trust, or in a VM.
   - Second, as defence in depth, without promising confinement:
     - Refuse any command word with glob characters (`*?[`).
     - Refuse `ln` (or make symlinks unusable).
     - Refuse the `sed` `w`/`W`/`e`/`r` commands, `awk` with `system`, `>` or `|` in its program, `tar --checkpoint-action`/`--to-command`/`--use-compress-program`, `zip -TT`/`--unzip-command`, and editors run with `-c`/`+`.
     - Or reduce the terminal to the task's own fixed command(s), for example `{python} -m unittest -q`.
   - Note that even fixed commands still run model code. Real confinement needs an OS sandbox (bubblewrap, sandbox-exec, AppContainer); that is later work.

2. Verification can be passed without solving the task. CONFIRMED.
   - Where: `bench/tasks/*/verify/verify.py` and `verify.cjs`, and the nonce design in `src-tauri/src/evals/verify.rs`.
   - Method: I ran the shipped verify scripts exactly as `run_script` does (`python3 -I -B script`, cwd set to the workspace, nonce on stdin, minimal env) against the untouched fixtures plus one appended line each.
   - py-fix-failing-test: appending `import unittest as _u; _u.TestCase.assertEqual = lambda *a, **k: None` to the buggy `stats.py` printed `VERIFIED abc123`, exit 0.
   - Also py-fix-failing-test: `import __main__; print("VERIFIED " + __main__.NONCE)` plus flush and `os._exit(0)` printed `VERIFIED n0nce99`, exit 0. The nonce sits in the same process as the model's code, so it protects nothing once that code is imported.
   - js-fix-failing-test: appending `require("node:assert").strictEqual = () => {};` to the buggy `slugify.js` printed `VERIFIED zz77`, exit 0.
   - The "unchanged" checks still pass, because the tests are untouched.
   - Impact: benchmark results only, and it needs a model that cheats on purpose. Medium.
   - Fix: import the model's code only in a child process. That child prints the raw values for the hidden inputs as JSON, and the parent verifier (or Rust) compares them with the expected values. The nonce then becomes unnecessary.

3. Verification can hang forever, and "End the benchmark" does not stop it. CONFIRMED.
   - Where: `src-tauri/src/evals/verify.rs` (`run_program`, about lines 390–455) and `bench.rs` (`run_task` calls `verify` without the cancel token).
   - Method: an exact copy of `run_program` with a 5 s limit, run on a script that does `subprocess.Popen(["sleep","40"], start_new_session=True)` and then prints and exits 0. After 20 s it was still waiting. The same thing without `start_new_session` returned in 25 ms, because the group is killed.
   - Cause: after the child exits, `stdout.await` and `stderr.await` wait for EOF with no time limit. A process in another session that holds the pipe keeps it open.
   - Effect in the app: the model's code is imported by the verifier, so a model can make the benchmark never finish. `RUNNING` stays set, cancel has no effect because verify never sees the token, and no further check or benchmark can start until the app restarts.
   - Related: even without a hang, Stop during verification waits up to 60 s per script and 20 s per parse check, against the report's "Stop works within a second".
   - Fix: bound the reader joins after exit (as `terminal.rs` does with a 2 s timeout), signal the group after a normal exit too, and `select!` on the cancel token in `verify` and `unparsable`.

4. The capability-check confirmation says "Nothing leaves this computer" when the model can be a remote endpoint. PLAUSIBLE from reading.
   - Where: `src/components/evals/shared.tsx:18`. `useModelChoice` offers every `source === "provider"` model, and `commands.rs::model_for` supports `remote:` models.
   - For an endpoint, the prompts, file contents and tool results go to that endpoint's host.
   - Fix: when the chosen ref starts with `remote:`, say "The tasks and the model's replies go to <endpoint host>". The benchmark text should say the same.

5. One evals test is red. CONFIRMED.
   - `cargo +1.99 test -p app --lib evals::`: 61 passed, 1 failed.
   - The failing test is `evals::sandbox::tests::the_node_test_runner_runs_and_nothing_rides_along` (sandbox/tests.rs:601): "cat notes.txt | node --test would have run".
   - Cause: feature A's `takes_no_code` in `crates/policy/src/allow_all.rs` now reads `node --test` as plain itself.
   - This is harmless, since the test runner ignores standard input. The fix is to update the expectation; the `is_node_test_runner` exception in `sandbox.rs` is now redundant.

6. `python3 -m http.server 8765` is judged Run. CONFIRMED by judge only, not executed.
   - It opens a server on all network interfaces, serving the workspace (and, after 1c, anything outside it) for up to the 600 s terminal limit. That is "going online" in the listening sense.
   - Fix: refuse `-m http.server` and similar modules, or allow `-m` only for `unittest`, `pytest` and `json.tool`.

7. Some legitimate commands are refused. PLAUSIBLE from reading.
   - Where: `sandbox.rs` `command_stays_inside`, where `..` is summed over the whole line.
   - `cd src && cat ../notes.txt` or `ls src/..` gives ups=1. `..` is then checked from the workspace root and refused, although the command stays inside.
   - Low impact: it only costs the model steps and counts as "blocked".

8. Clean-up corner cases. PLAUSIBLE; I could not test as root.
   - Where: `workspace.rs` `make_writable`.
   - A folder the model sets to mode 000: `set_readonly(false)` adds write bits only, so it still cannot be listed, and the workspace is left in the temp folder with a warning.
   - On Unix, `set_readonly(false)` makes files writable for everyone just before deletion.
   - A hard link to an outside file (possible only after an escape from 1) would get its permissions changed.
   - Workspaces and detached processes are also left behind when the app exits mid-run.

CHECKED AND SOUND
- Every call goes describe → `sandbox::Gate` → `policy::evaluate` (Allow all, untrusted from the first call) → `execute`, which runs `judge` again.
  - The gate never returns Ask, and `NobodyToAsk` denies.
  - `runner.rs` has no path that runs a tool without describe and decide.
  - Tools that are not offered (`delete_file`) are refused.
  - There is one audit row per call, linked to the run row. The run row's preview repeats the false claim in 1.
- File tools are sound. `..`, absolute paths, and a model-made symlink to outside are all refused (I made `link -> outside`, then tried `read_file link/x` and `write_file link/x.txt`: both blocked). `search_files` and `list_dir` do not follow links (confirmed).
- The capability check offers no terminal, so the model cannot make a symlink, and I found no way out of its folder.
- Workspace: a uuid name under `temp_dir` made with `create_dir` and canonicalized; there is no path argument anywhere. Removal does not follow a symlink to an outside folder (the existing test passes, and std `remove_dir_all` does not follow links).
- Commands: all 8 are registered in `lib.rs`, `commands.list` and `capabilities/main.json`, and that capability covers the main window only.
  - Start, forget and cancel check for the main window.
  - `get_bench_run` and `load` accept only the strict 20-character id, so there is no path traversal.
  - `forget_model_profile` accepts only `modelProfile:` plus 16 lower-case hex digits.
  - One run at a time (static guard), and nothing starts except from the confirm-button handlers. No start on launch, and no tool or chat path reaches these commands.
- Profiles: `apply` touches only thinking and temperature, and the user's choice wins. `load` checks the identity. The profile key is not in `set_setting`'s allow-list.
- No telemetry: the evals code makes no network calls besides the model client. No API key goes into a profile, a benchmark JSON or a log line; profiles store the base URL and model, so a URL with credentials written into it would be stored.
- Commands the rule refuses: redirections and here-docs outside, backticks and `$( )`, `python3 -c`, `python3 -m urllib.request`, `cp -r . ..`, `cat /e*/passwd`, `ln -s /home h`, `ln -s .. up`, `nohup`, `rsync`, `git config`, `truncate`, and absolute paths after `=` (`tar … exec='touch /abs'`, `git -c core.pager=…`).
- Task checks: three different correct roundings of js-add-feature give 22.95. The built-in INI, JSON and Order checks reject the obvious wrong placements.

NOT COVERED
- Nothing was run on Windows or macOS. PowerShell provider paths (`HKCU:\`, `env:`), drive-relative paths and zsh glob behaviour are untested. zsh never matches `..`, so 1c's `.*.` trick is dash-specific, but 1a and 1b apply on every platform.
- The UI was not run. I only read the confirmation texts.
