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
echo "spawn check passed"
