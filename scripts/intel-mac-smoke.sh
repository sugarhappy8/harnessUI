#!/usr/bin/env bash
# Runs the Intel half of the universal Mac disk image on a Mac with an Intel processor.
#
# The build machine is an Apple silicon Mac, so nothing there executes the x86_64 code.
# This opens the disk image and checks, on real Intel hardware:
#   1. every program in the app is universal;
#   2. the app's own binary starts (as its PDF worker, which exits before any window);
#   3. llama-server and llama-fit-params start, and llama-server lists its devices;
#   4. llama-server loads a small model on the CPU and generates a few tokens, the way
#      Auto runs models on an Intel Mac (ARCHITECTURE.md 7.2).
#
#   bash scripts/intel-mac-smoke.sh <disk image> <model.gguf>
set -euo pipefail

dmg=${1:?usage: intel-mac-smoke.sh <disk image> <model.gguf>}
model=${2:?usage: intel-mac-smoke.sh <disk image> <model.gguf>}

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "x86_64" ]; then
  echo "This check needs a Mac with an Intel processor (this is $(uname -s) $(uname -m))."
  exit 1
fi
[ -f "$model" ] || { echo "model file not found: $model"; exit 1; }

mount=$(mktemp -d)
work=$(mktemp -d)
server_pid=""
cleanup() {
  if [ -n "$server_pid" ]; then kill "$server_pid" 2>/dev/null || true; fi
  hdiutil detach "$mount" -force >/dev/null 2>&1 || true
}
trap cleanup EXIT

hdiutil attach "$dmg" -nobrowse -readonly -mountpoint "$mount"
app=$(ls -d "$mount"/*.app | head -1)
programs="$app/Contents/MacOS"
main=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist")
echo "App: $app (main program: $main)"

echo "== 1. Every program is universal"
for program in "$programs"/*; do
  archs=$(lipo -archs "$program")
  echo "$(basename "$program"): $archs"
  for arch in arm64 x86_64; do
    case " $archs " in
      *" $arch "*) ;;
      *) echo "::error::$(basename "$program") has no $arch part"; exit 1 ;;
    esac
  done
done

echo "== 2. The app's own binary starts on this processor"
# The PDF worker exits with 2 for a file that does not exist. Any other result (a missing
# library, an instruction this processor lacks) means the Intel half does not start.
code=0
"$programs/$main" --internal-pdf-worker "$work/no-such-file.pdf" || code=$?
if [ "$code" -ne 2 ]; then
  echo "::error::$main exited with $code as the PDF worker, expected 2"
  exit 1
fi
echo "$main started and exited as expected."

echo "== 3. The llama.cpp sidecars start"
"$programs/llama-server" --version
"$programs/llama-fit-params" --version
"$programs/llama-server" --list-devices

echo "== 4. A short generation on the CPU"
port=18439
"$programs/llama-server" --model "$model" --device none -ngl 0 --ctx-size 2048 \
  --host 127.0.0.1 --port "$port" > "$work/server.log" 2>&1 &
server_pid=$!
ready=""
for _ in $(seq 1 180); do
  if curl -fsS "http://127.0.0.1:$port/health" >/dev/null 2>&1; then ready=1; break; fi
  if ! kill -0 "$server_pid" 2>/dev/null; then break; fi
  sleep 1
done
if [ -z "$ready" ]; then
  echo "::error::llama-server did not become ready on the CPU"
  tail -n 80 "$work/server.log" || true
  exit 1
fi
curl -fsS "http://127.0.0.1:$port/completion" -H 'Content-Type: application/json' \
  -d '{"prompt": "The capital of France is", "n_predict": 8, "cache_prompt": false}' > "$work/reply.json"
python3 - "$work/reply.json" <<'PY'
import json, sys

reply = json.load(open(sys.argv[1], encoding="utf-8"))
text = reply.get("content", "")
if not text.strip():
    raise SystemExit(f"::error::llama-server generated nothing: {reply}")
speed = (reply.get("timings") or {}).get("predicted_per_second")
print(f"generated {text!r} at {speed} tokens per second")
PY
grep -iE "system_info|AVX" "$work/server.log" | head -3 || true
echo "The Intel half of the disk image works on this Mac."
