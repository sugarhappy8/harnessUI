#!/usr/bin/env bash
# Every child process must be started through the spawn helper in crates/runtime,
# so it lands in a Job Object (Windows) or a registered process group (macOS) and
# cannot outlive the app (ARCHITECTURE.md 4.1). Fails if any other file builds a
# process directly.
set -euo pipefail
cd "$(dirname "$0")/.."
hits=$(grep -rnE '(std|tokio)::process::Command::new|\bCommand::new\(' --include='*.rs' crates src-tauri/src \
  | grep -v '^crates/runtime/src/spawn' \
  | grep -v '// spawn-ok:' || true)
if [ -n "$hits" ]; then
  echo "Direct process creation outside crates/runtime/src/spawn*:"
  echo "$hits"
  exit 1
fi
# The same rule for the operating system's own ways to start a program or load a
# library at run time (no file needs them; the usage readers in crates/usage call
# the system directly and must never start or load anything). No exemption comment.
# Lines that are only a comment are skipped (prose may name these calls); a call
# followed by a comment is still caught.
low=$(grep -rnE 'CreateProcess|ShellExecute|WinExec|posix_spawn|fork\(|execv|execl|popen|libc::system|dlopen|LoadLibrary|libloading' --include='*.rs' crates src-tauri/src \
  | grep -v '^crates/runtime/src/spawn' \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)
if [ -n "$low" ]; then
  echo "Low-level process creation or library loading outside crates/runtime/src/spawn*:"
  echo "$low"
  exit 1
fi
echo "spawn check passed"
