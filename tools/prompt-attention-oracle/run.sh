#!/usr/bin/env bash
# Builds the oracle and the harnesses, then runs them: the two comparisons against the engine's
# own parse_prompt_attention and the benchmark of the child-safety prompt check. Not in CI; run
# it by hand after a change to crates/runtime/src/sd/prompt_view.rs or the engine pin.
#
#   bash tools/prompt-attention-oracle/run.sh            # everything, about ten minutes
#   OUT=/some/dir bash tools/prompt-attention-oracle/run.sh
#
# Needs a C++17 compiler (c++, g++ or clang++) and rustc. Nothing is written in the repository:
# the builds go to $OUT, a temporary folder by default.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
out="${OUT:-$(mktemp -d)}"
cxx="${CXX:-$(command -v c++ || command -v g++ || command -v clang++)}"

# The one constant sd.rs repeats must still be the real one.
want=$(grep -oE 'pub const LOCAL_PROMPT_MAX_CHARS: usize = [0-9_]+' "$root/crates/runtime/src/sd/args.rs" | grep -oE '[0-9_]+$')
have=$(grep -oE 'pub const LOCAL_PROMPT_MAX_CHARS: usize = [0-9_]+' "$here/sd.rs" | grep -oE '[0-9_]+$')
if [ "$want" != "$have" ]; then
  echo "sd.rs says LOCAL_PROMPT_MAX_CHARS is $have; args.rs says $want. Update sd.rs." >&2
  exit 1
fi

echo "Building into $out"
"$cxx" -std=c++17 -O2 -o "$out/oracle" "$here/oracle.cpp"
rustc --edition 2024 -O --crate-type rlib --crate-name runtime -o "$out/libruntime.rlib" "$here/shim.rs"
for bin in fuzz deepfuzz bench checkbench; do
  D="$out" rustc --edition 2024 -O --extern runtime="$out/libruntime.rlib" -o "$out/$bin" "$here/$bin.rs"
done

echo "== parse_prompt_attention and the prompt file's view, against the engine (random fragments)"
"$out/fuzz" 300000 | tail -1
echo "== parse_prompt_attention against the engine (bracket-heavy, weights that underflow and overflow)"
"$out/deepfuzz" 100000 | tail -1
"$out/deepfuzz" 100000 2685821657736338717 | tail -1
"$out/deepfuzz" 20000 15074160668108271509 | tail -1
echo "== the child-safety prompt check at the local bound, release build (best of three)"
"$out/checkbench"
echo "== parse_prompt_attention and attention_text alone, at and past the bound"
"$out/bench"
