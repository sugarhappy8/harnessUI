REPORT: feature F, failed commands counted apart from tool failures

A `run_command` call that ran and exited non-zero (for example failing tests) is now counted as a "failed command", not a tool failure. It has its own "Failed commands" column in the run table and the compare view and in the saved JSON. All checks pass, and so does the `capability-check` scenario.

How it is decided (`src-tauri/src/evals/harness.rs`):
- `CallRec` gains `exit_code: Option<i32>`. It is recorded only for a `run_command` call that was not blocked, and read only from the terminal result's first line in the exact form `Exit code N (<time> s)` (`exit_code_of`).
- A timeout, a signal, Stop, a refusal or any other first line gives no exit code. No other wording is used.
- `CallRec::failed_command()`: a `run_command` call with status `error` (the tool's `is_error`) and an exit code that is neither 0 nor 126/127. 126 and 127 are the shell's own "cannot run" and "not found", so those stay tool failures.
- `failed()`, the tool-failure count, is now "status error and not a failed command". Nothing else changed:
  - timeouts, signals, Stop, programs not found, refused working folders and failed calls of every other tool stay tool failures;
  - blocked calls stay counted only as blocked.
- `Record::failed_commands()` is new.
- The model check has no terminal, so its counts are unchanged.

One thing to know: under PowerShell a program not found exits with 1 rather than 127. It would therefore count as a failed command on Windows. The only way to tell it apart is the error's wording, which you ruled out.

Benchmark (`bench.rs`):
- `TaskResult.failed_commands` and `Totals.failed_commands` are new, both `#[serde(default)]`, so runs saved before this change still load with 0.
- The UI (`src/components/evals/Benchmarks.tsx`, `types.ts`):
  - the run table has a "Failed commands" column after "Tool failures", per task and in the total;
  - the compare view has "Failed commands, first" and "Failed commands, second".

Tests added (I wrote these together with the code, so I did not see them fail first):
- `harness::tests::failed_commands_are_told_from_tool_failures`: a table of exit 1, exit 2, exit 0, timeout, signal, Stop, blocked, exit 127, exit 126, a refused working folder, a failed `edit_file` (even one whose text starts with "Exit code 1") and a failed `write_file`. It also covers the parser's edge cases and the per-record counts.
- `harness::tests::a_failing_test_run_is_a_failed_command_not_a_tool_failure`: three calls through the real terminal.
  - `python3 fails.py` exits 1 and is a failed command.
  - `python3 passes.py` exits 0 and counts as neither.
  - `python3 missing.py`, a script that does not exist, exits 2 from Python itself; the program ran, so it is a failed command. Result: 2 failed commands, 0 tool failures.
- `bench::tests::a_run_saved_before_failed_commands_loads`: old JSON without the field loads, lists, and saves back with 0. `totals_add_up` is extended to the new field.
- Frontend `evals.test.tsx` "failed commands": the column's position and per-task and total values in the run table, and both columns and totals in the compare view.

Checks run, all passing:
- `cargo fmt --all --check`: clean.
- `cargo +1.99 clippy -p app --all-targets -- -D warnings`: clean.
- `cargo +1.99 test -p app --lib evals::`: 78 passed, 0 failed.
- `npm run lint` and `npm run typecheck`: clean.
- `npm test`: 36 files, 248 passed.
- e2e `capability-check`: PASS; the benchmark totals row now shows the new column.
- I did not re-run the real-model script, as you asked.

All changes are in my files only: `src-tauri/src/evals/` and `src/components/evals/`.
